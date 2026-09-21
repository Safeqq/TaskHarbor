use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use axum::Json;
use axum::body::Body;
use axum::extract::{Extension, State};
use axum::http::{Method, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use taskharbor_adapters::{
    PgJobRepository, RepositoryError, SessionRecord, UserCredentials, UserId,
};
use time::{Duration as CookieDuration, OffsetDateTime};
use uuid::Uuid;

use crate::AppState;
use crate::error::ApiError;

pub const SESSION_COOKIE: &str = "taskharbor_session";
pub const CSRF_COOKIE: &str = "taskharbor_csrf";
pub const CSRF_HEADER: &str = "x-csrf-token";

#[derive(Debug, Clone)]
pub struct AuthSession {
    pub id: Uuid,
    pub user_id: UserId,
    pub username: String,
    pub csrf_token_hash: Vec<u8>,
    pub expires_at: OffsetDateTime,
}

impl From<SessionRecord> for AuthSession {
    fn from(record: SessionRecord) -> Self {
        Self {
            id: record.id(),
            user_id: record.user_id(),
            username: record.username().to_owned(),
            csrf_token_hash: record.csrf_token_hash().to_vec(),
            expires_at: record.expires_at(),
        }
    }
}

#[derive(Debug)]
pub enum OwnerSetupError {
    InvalidUsername,
    InvalidPassword,
    PasswordHash,
    Repository(RepositoryError),
    Task(tokio::task::JoinError),
}

impl Display for OwnerSetupError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUsername => write!(
                formatter,
                "owner username must contain 3-32 lowercase letters, digits, hyphens, or underscores"
            ),
            Self::InvalidPassword => {
                write!(formatter, "owner password must contain 12-128 bytes")
            }
            Self::PasswordHash => write!(formatter, "owner password could not be hashed"),
            Self::Repository(error) => Display::fmt(error, formatter),
            Self::Task(error) => write!(formatter, "password hashing task failed: {error}"),
        }
    }
}

impl Error for OwnerSetupError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Repository(error) => Some(error),
            Self::Task(error) => Some(error),
            Self::InvalidUsername | Self::InvalidPassword | Self::PasswordHash => None,
        }
    }
}

impl From<RepositoryError> for OwnerSetupError {
    fn from(error: RepositoryError) -> Self {
        Self::Repository(error)
    }
}

pub async fn initialize_owner(
    repository: &PgJobRepository,
    username: &str,
    password: &str,
) -> Result<UserCredentials, OwnerSetupError> {
    let username = normalize_username(username).ok_or(OwnerSetupError::InvalidUsername)?;
    validate_password(password).map_err(|_| OwnerSetupError::InvalidPassword)?;
    let current = repository.owner_credentials().await?;
    if current.username() == username
        && verify_password(password.to_owned(), current.password_hash().to_owned()).await?
    {
        return Ok(current);
    }

    let password = password.to_owned();
    let password_hash = tokio::task::spawn_blocking(move || {
        Argon2::default()
            .hash_password(password.as_bytes())
            .map(|hash| hash.to_string())
            .map_err(|_| OwnerSetupError::PasswordHash)
    })
    .await
    .map_err(OwnerSetupError::Task)??;
    repository
        .configure_owner(&username, &password_hash)
        .await
        .map_err(Into::into)
}

pub async fn login(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(request): Json<LoginRequest>,
) -> Result<(CookieJar, (StatusCode, Json<SessionResponse>)), ApiError> {
    let normalized_username = normalize_username(&request.username);
    state
        .login_limiter
        .check("owner-login")
        .await
        .map_err(ApiError::rate_limited)?;
    validate_password(&request.password).map_err(|_| ApiError::invalid_credentials())?;

    let requested_user = match normalized_username {
        Some(username) => state
            .jobs
            .user_credentials_by_username(&username)
            .await
            .map_err(ApiError::repository)?,
        None => None,
    };
    let credentials = match requested_user.as_ref() {
        Some(credentials) => credentials.clone(),
        None => state
            .jobs
            .owner_credentials()
            .await
            .map_err(ApiError::repository)?,
    };
    let password_valid = verify_password(request.password, credentials.password_hash().to_owned())
        .await
        .map_err(|_| ApiError::internal())?;
    if requested_user.is_none() || !password_valid {
        return Err(ApiError::invalid_credentials());
    }

    let session_token = random_token();
    let csrf_token = random_token();
    let session = state
        .jobs
        .create_session(
            credentials.id(),
            &token_hash(&session_token),
            &token_hash(&csrf_token),
            state.config.session_ttl,
        )
        .await
        .map_err(ApiError::repository)?;
    let jar = jar
        .add(session_cookie(
            session_token,
            state.config.session_ttl,
            state.config.secure_cookies,
        ))
        .add(csrf_cookie(
            csrf_token.clone(),
            state.config.session_ttl,
            state.config.secure_cookies,
        ));
    tracing::info!(
        event = "session_login",
        user_id = %credentials.id(),
        username = %credentials.username(),
        session_id = %session.id()
    );

    Ok((
        jar,
        (
            StatusCode::CREATED,
            Json(SessionResponse::new(&session, csrf_token)),
        ),
    ))
}

pub async fn current_session(
    State(state): State<AppState>,
    Extension(session): Extension<AuthSession>,
    jar: CookieJar,
) -> Result<(CookieJar, Json<SessionResponse>), ApiError> {
    let current_csrf = jar.get(CSRF_COOKIE).map(|cookie| cookie.value().to_owned());
    let csrf_token = if current_csrf
        .as_ref()
        .is_some_and(|token| constant_time_eq(&token_hash(token), &session.csrf_token_hash))
    {
        current_csrf.unwrap_or_default()
    } else {
        let token = random_token();
        state
            .jobs
            .rotate_session_csrf(session.id, &token_hash(&token))
            .await
            .map_err(ApiError::repository)?;
        token
    };
    let jar = jar.add(csrf_cookie(
        csrf_token.clone(),
        state.config.session_ttl,
        state.config.secure_cookies,
    ));
    Ok((jar, Json(SessionResponse::from_auth(&session, csrf_token))))
}

pub async fn logout(
    State(state): State<AppState>,
    Extension(session): Extension<AuthSession>,
    jar: CookieJar,
) -> Result<(CookieJar, StatusCode), ApiError> {
    state
        .jobs
        .revoke_session(session.id)
        .await
        .map_err(ApiError::repository)?;
    tracing::info!(
        event = "session_logout",
        user_id = %session.user_id,
        session_id = %session.id
    );
    let jar = jar
        .remove(removal_cookie(
            SESSION_COOKIE,
            true,
            state.config.secure_cookies,
        ))
        .remove(removal_cookie(
            CSRF_COOKIE,
            false,
            state.config.secure_cookies,
        ));
    Ok((jar, StatusCode::NO_CONTENT))
}

pub async fn require_auth(
    State(state): State<AppState>,
    mut request: Request<Body>,
    next: Next,
) -> Result<Response, ApiError> {
    let jar = CookieJar::from_headers(request.headers());
    let raw_token = jar
        .get(SESSION_COOKIE)
        .map(|cookie| cookie.value())
        .ok_or_else(ApiError::unauthorized)?;
    let session = state
        .jobs
        .active_session(&token_hash(raw_token))
        .await
        .map_err(ApiError::repository)?
        .ok_or_else(ApiError::unauthorized)?;

    if requires_csrf(request.method()) {
        let supplied = request
            .headers()
            .get(CSRF_HEADER)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(ApiError::invalid_csrf)?;
        if !constant_time_eq(&token_hash(supplied), session.csrf_token_hash()) {
            return Err(ApiError::invalid_csrf());
        }
    }

    request.extensions_mut().insert(AuthSession::from(session));
    Ok(next.run(request).await)
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    username: String,
    password: String,
}

#[derive(Debug, Serialize)]
pub struct SessionResponse {
    user: SessionUserResponse,
    #[serde(with = "time::serde::rfc3339")]
    expires_at: OffsetDateTime,
    csrf_token: String,
}

impl SessionResponse {
    fn new(session: &SessionRecord, csrf_token: String) -> Self {
        Self {
            user: SessionUserResponse {
                id: session.user_id().get(),
                username: session.username().to_owned(),
            },
            expires_at: session.expires_at(),
            csrf_token,
        }
    }

    fn from_auth(session: &AuthSession, csrf_token: String) -> Self {
        Self {
            user: SessionUserResponse {
                id: session.user_id.get(),
                username: session.username.clone(),
            },
            expires_at: session.expires_at,
            csrf_token,
        }
    }
}

#[derive(Debug, Serialize)]
struct SessionUserResponse {
    id: i64,
    username: String,
}

async fn verify_password(password: String, password_hash: String) -> Result<bool, OwnerSetupError> {
    tokio::task::spawn_blocking(move || {
        let parsed = match PasswordHash::new(&password_hash) {
            Ok(parsed) => parsed,
            Err(_) => return Ok(false),
        };
        Ok(Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok())
    })
    .await
    .map_err(OwnerSetupError::Task)?
}

fn normalize_username(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    let valid_length = (3..=32).contains(&value.len());
    let valid_characters = value.chars().enumerate().all(|(index, character)| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || (index > 0 && matches!(character, '_' | '-'))
    });
    (valid_length && valid_characters).then_some(value)
}

fn validate_password(password: &str) -> Result<(), ()> {
    if (12..=128).contains(&password.len()) {
        Ok(())
    } else {
        Err(())
    }
}

fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

pub(crate) fn token_hash(value: &str) -> Vec<u8> {
    Sha256::digest(value.as_bytes()).to_vec()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn requires_csrf(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

fn session_cookie(value: String, ttl: Duration, secure: bool) -> Cookie<'static> {
    Cookie::build((SESSION_COOKIE, value))
        .path("/")
        .http_only(true)
        .same_site(SameSite::Strict)
        .secure(secure)
        .max_age(cookie_duration(ttl))
        .build()
}

fn csrf_cookie(value: String, ttl: Duration, secure: bool) -> Cookie<'static> {
    Cookie::build((CSRF_COOKIE, value))
        .path("/")
        .http_only(false)
        .same_site(SameSite::Strict)
        .secure(secure)
        .max_age(cookie_duration(ttl))
        .build()
}

fn removal_cookie(name: &'static str, http_only: bool, secure: bool) -> Cookie<'static> {
    Cookie::build(name)
        .path("/")
        .http_only(http_only)
        .same_site(SameSite::Strict)
        .secure(secure)
        .max_age(CookieDuration::ZERO)
        .build()
}

fn cookie_duration(value: Duration) -> CookieDuration {
    let seconds = i64::try_from(value.as_secs()).unwrap_or(i64::MAX);
    CookieDuration::seconds(seconds)
}

impl IntoResponse for OwnerSetupError {
    fn into_response(self) -> Response {
        ApiError::internal().into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::{constant_time_eq, normalize_username, token_hash, validate_password};

    #[test]
    fn validates_credentials_and_token_comparisons() {
        assert_eq!(normalize_username(" Owner-1 ").as_deref(), Some("owner-1"));
        assert!(normalize_username("-owner").is_none());
        assert!(validate_password("long-enough-password").is_ok());
        assert!(validate_password("short").is_err());
        assert!(constant_time_eq(&token_hash("a"), &token_hash("a")));
        assert!(!constant_time_eq(&token_hash("a"), &token_hash("b")));
    }
}
