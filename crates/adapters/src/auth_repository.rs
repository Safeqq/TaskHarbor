use std::fmt::{self, Display, Formatter};
use std::time::Duration;

use sqlx::FromRow;
use time::OffsetDateTime;
use uuid::Uuid;

use crate::postgres::{PgJobRepository, RepositoryError, ensure_one_row};

pub const OWNER_USER_ID: UserId = UserId(1);
pub const MIN_SESSION_TTL: Duration = Duration::from_secs(5 * 60);
pub const MAX_SESSION_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct UserId(i64);

impl UserId {
    pub fn new(value: i64) -> Result<Self, RepositoryError> {
        if value <= 0 {
            return Err(RepositoryError::InvalidData(
                "user ID must be a positive BIGINT".into(),
            ));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> i64 {
        self.0
    }
}

impl Display for UserId {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(&self.0, formatter)
    }
}

#[derive(Debug, Clone)]
pub struct UserCredentials {
    id: UserId,
    username: String,
    password_hash: String,
}

impl UserCredentials {
    pub const fn id(&self) -> UserId {
        self.id
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn password_hash(&self) -> &str {
        &self.password_hash
    }
}

#[derive(Debug, Clone)]
pub struct SessionRecord {
    id: Uuid,
    user_id: UserId,
    username: String,
    csrf_token_hash: Vec<u8>,
    expires_at: OffsetDateTime,
}

impl SessionRecord {
    pub const fn id(&self) -> Uuid {
        self.id
    }

    pub const fn user_id(&self) -> UserId {
        self.user_id
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    pub fn csrf_token_hash(&self) -> &[u8] {
        &self.csrf_token_hash
    }

    pub const fn expires_at(&self) -> OffsetDateTime {
        self.expires_at
    }
}

impl PgJobRepository {
    pub async fn owner_credentials(&self) -> Result<UserCredentials, RepositoryError> {
        self.user_credentials_by_id(OWNER_USER_ID)
            .await?
            .ok_or_else(|| RepositoryError::InvalidData("bootstrap owner is missing".into()))
    }

    pub async fn user_credentials_by_username(
        &self,
        username: &str,
    ) -> Result<Option<UserCredentials>, RepositoryError> {
        let row = sqlx::query_as::<_, UserCredentialsRow>(
            r#"
            SELECT id, username, password_hash
            FROM users
            WHERE username = $1
              AND is_active
            "#,
        )
        .bind(username)
        .fetch_optional(&self.pool)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn configure_owner(
        &self,
        username: &str,
        password_hash: &str,
    ) -> Result<UserCredentials, RepositoryError> {
        validate_username(username)?;
        if password_hash.is_empty() || password_hash.len() > 512 {
            return Err(RepositoryError::InvalidData(
                "password hash length is invalid".into(),
            ));
        }

        let mut transaction = self.pool.begin().await?;
        let previous = sqlx::query_as::<_, UserCredentialsRow>(
            "SELECT id, username, password_hash FROM users WHERE id = $1 FOR UPDATE",
        )
        .bind(OWNER_USER_ID.get())
        .fetch_optional(&mut *transaction)
        .await?
        .ok_or_else(|| RepositoryError::InvalidData("bootstrap owner is missing".into()))?;
        let changed = previous.username != username || previous.password_hash != password_hash;

        let updated = sqlx::query_as::<_, UserCredentialsRow>(
            r#"
            UPDATE users
            SET username = $2,
                password_hash = $3,
                updated_at = CURRENT_TIMESTAMP
            WHERE id = $1
            RETURNING id, username, password_hash
            "#,
        )
        .bind(OWNER_USER_ID.get())
        .bind(username)
        .bind(password_hash)
        .fetch_one(&mut *transaction)
        .await?;

        if changed {
            sqlx::query(
                r#"
                UPDATE sessions
                SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP)
                WHERE user_id = $1
                  AND revoked_at IS NULL
                "#,
            )
            .bind(OWNER_USER_ID.get())
            .execute(&mut *transaction)
            .await?;
        }
        transaction.commit().await?;
        updated.try_into()
    }

    pub async fn create_session(
        &self,
        user_id: UserId,
        token_hash: &[u8],
        csrf_token_hash: &[u8],
        ttl: Duration,
    ) -> Result<SessionRecord, RepositoryError> {
        validate_digest(token_hash, "session token")?;
        validate_digest(csrf_token_hash, "CSRF token")?;
        if !(MIN_SESSION_TTL..=MAX_SESSION_TTL).contains(&ttl) || ttl.subsec_nanos() != 0 {
            return Err(RepositoryError::InvalidData(format!(
                "session TTL must be a whole number of seconds between {} and {}",
                MIN_SESSION_TTL.as_secs(),
                MAX_SESSION_TTL.as_secs()
            )));
        }
        let ttl_seconds = i64::try_from(ttl.as_secs())
            .map_err(|_| RepositoryError::InvalidData("session TTL exceeds BIGINT".into()))?;
        let session_id = Uuid::new_v4();
        let row = sqlx::query_as::<_, SessionRow>(
            r#"
            INSERT INTO sessions (
                id,
                user_id,
                token_hash,
                csrf_token_hash,
                created_at,
                last_seen_at,
                expires_at
            )
            SELECT
                $1,
                account.id,
                $3,
                $4,
                CURRENT_TIMESTAMP,
                CURRENT_TIMESTAMP,
                CURRENT_TIMESTAMP + ($5 * INTERVAL '1 second')
            FROM users AS account
            WHERE account.id = $2
              AND account.is_active
            RETURNING
                id,
                user_id,
                (SELECT username FROM users WHERE id = user_id) AS username,
                csrf_token_hash,
                expires_at
            "#,
        )
        .bind(session_id)
        .bind(user_id.get())
        .bind(token_hash)
        .bind(csrf_token_hash)
        .bind(ttl_seconds)
        .fetch_optional(&self.pool)
        .await?
        .ok_or(RepositoryError::StateConflict(
            "create session for inactive user",
        ))?;
        row.try_into()
    }

    pub async fn active_session(
        &self,
        token_hash: &[u8],
    ) -> Result<Option<SessionRecord>, RepositoryError> {
        validate_digest(token_hash, "session token")?;
        let row = sqlx::query_as::<_, SessionRow>(
            r#"
            UPDATE sessions AS session
            SET last_seen_at = CURRENT_TIMESTAMP
            FROM users AS account
            WHERE session.token_hash = $1
              AND session.user_id = account.id
              AND account.is_active
              AND session.revoked_at IS NULL
              AND session.expires_at > CURRENT_TIMESTAMP
            RETURNING
                session.id,
                session.user_id,
                account.username,
                session.csrf_token_hash,
                session.expires_at
            "#,
        )
        .bind(token_hash)
        .fetch_optional(&self.pool)
        .await?;
        row.map(TryInto::try_into).transpose()
    }

    pub async fn revoke_session(&self, session_id: Uuid) -> Result<(), RepositoryError> {
        let updated = sqlx::query(
            r#"
            UPDATE sessions
            SET revoked_at = COALESCE(revoked_at, CURRENT_TIMESTAMP)
            WHERE id = $1
            "#,
        )
        .bind(session_id)
        .execute(&self.pool)
        .await?;
        ensure_one_row(updated.rows_affected(), "revoke session")
    }

    pub async fn rotate_session_csrf(
        &self,
        session_id: Uuid,
        csrf_token_hash: &[u8],
    ) -> Result<(), RepositoryError> {
        validate_digest(csrf_token_hash, "CSRF token")?;
        let updated = sqlx::query(
            r#"
            UPDATE sessions
            SET csrf_token_hash = $2,
                last_seen_at = CURRENT_TIMESTAMP
            WHERE id = $1
              AND revoked_at IS NULL
              AND expires_at > CURRENT_TIMESTAMP
            "#,
        )
        .bind(session_id)
        .bind(csrf_token_hash)
        .execute(&self.pool)
        .await?;
        ensure_one_row(updated.rows_affected(), "rotate session CSRF token")
    }

    pub async fn purge_expired_sessions(&self) -> Result<u64, RepositoryError> {
        let deleted = sqlx::query(
            r#"
            DELETE FROM sessions
            WHERE expires_at <= CURRENT_TIMESTAMP
               OR revoked_at <= CURRENT_TIMESTAMP - INTERVAL '1 day'
            "#,
        )
        .execute(&self.pool)
        .await?;
        Ok(deleted.rows_affected())
    }

    async fn user_credentials_by_id(
        &self,
        user_id: UserId,
    ) -> Result<Option<UserCredentials>, RepositoryError> {
        let row = sqlx::query_as::<_, UserCredentialsRow>(
            "SELECT id, username, password_hash FROM users WHERE id = $1 AND is_active",
        )
        .bind(user_id.get())
        .fetch_optional(&self.pool)
        .await?;
        row.map(TryInto::try_into).transpose()
    }
}

fn validate_username(username: &str) -> Result<(), RepositoryError> {
    let valid_length = (3..=32).contains(&username.len());
    let valid_characters = username.chars().enumerate().all(|(index, character)| {
        character.is_ascii_lowercase()
            || character.is_ascii_digit()
            || (index > 0 && matches!(character, '_' | '-'))
    });
    if !valid_length || !valid_characters {
        return Err(RepositoryError::InvalidData(
            "username must contain 3-32 lowercase letters, digits, hyphens, or underscores".into(),
        ));
    }
    Ok(())
}

fn validate_digest(value: &[u8], label: &str) -> Result<(), RepositoryError> {
    if value.len() == 32 {
        Ok(())
    } else {
        Err(RepositoryError::InvalidData(format!(
            "{label} hash must contain 32 bytes"
        )))
    }
}

#[derive(Debug, FromRow)]
struct UserCredentialsRow {
    id: i64,
    username: String,
    password_hash: String,
}

impl TryFrom<UserCredentialsRow> for UserCredentials {
    type Error = RepositoryError;

    fn try_from(row: UserCredentialsRow) -> Result<Self, Self::Error> {
        Ok(Self {
            id: UserId::new(row.id)?,
            username: row.username,
            password_hash: row.password_hash,
        })
    }
}

#[derive(Debug, FromRow)]
struct SessionRow {
    id: Uuid,
    user_id: i64,
    username: String,
    csrf_token_hash: Vec<u8>,
    expires_at: OffsetDateTime,
}

impl TryFrom<SessionRow> for SessionRecord {
    type Error = RepositoryError;

    fn try_from(row: SessionRow) -> Result<Self, Self::Error> {
        validate_digest(&row.csrf_token_hash, "CSRF token")?;
        Ok(Self {
            id: row.id,
            user_id: UserId::new(row.user_id)?,
            username: row.username,
            csrf_token_hash: row.csrf_token_hash,
            expires_at: row.expires_at,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::validate_username;

    #[test]
    fn validates_owner_username_shape() {
        assert!(validate_username("owner-1").is_ok());
        assert!(validate_username("Owner").is_err());
        assert!(validate_username("-owner").is_err());
        assert!(validate_username("ab").is_err());
    }
}
