use std::env;
use std::error::Error;
use std::io;
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;

use taskharbor_adapters::{LocalStorage, MAX_SESSION_TTL, MIN_SESSION_TTL, PgJobRepository};
use taskharbor_api::{AppConfig, app_with_config, initialize_owner};
use tokio::{net::TcpListener, signal};
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";
const DEFAULT_STORAGE_DIR: &str = "var/storage";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error + Send + Sync>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .json()
        .try_init()?;
    let database_url = env::var("DATABASE_URL").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "DATABASE_URL must be set before starting the API",
        )
    })?;
    let bind_addr =
        env::var("TASKHARBOR_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());
    let bind_addr = bind_addr.parse::<SocketAddr>()?;
    let secure_cookies = parse_bool("TASKHARBOR_SECURE_COOKIES", false)?;
    let allow_remote = parse_bool("TASKHARBOR_ALLOW_REMOTE", false)?;
    if !(bind_addr.ip().is_loopback() || allow_remote && secure_cookies) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "a non-loopback bind requires TASKHARBOR_ALLOW_REMOTE=true and TASKHARBOR_SECURE_COOKIES=true behind HTTPS",
        )
        .into());
    }
    let owner_username =
        env::var("TASKHARBOR_OWNER_USERNAME").unwrap_or_else(|_| "owner".to_owned());
    let owner_password = env::var("TASKHARBOR_OWNER_PASSWORD").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "TASKHARBOR_OWNER_PASSWORD must be set before starting the API",
        )
    })?;
    let mut config = AppConfig::default();
    config.secure_cookies = secure_cookies;
    config.session_ttl = Duration::from_secs(parse_positive::<u64>(
        "TASKHARBOR_SESSION_TTL_SECONDS",
        config.session_ttl.as_secs(),
    )?);
    if !(MIN_SESSION_TTL..=MAX_SESSION_TTL).contains(&config.session_ttl) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "TASKHARBOR_SESSION_TTL_SECONDS must be between {} and {}",
                MIN_SESSION_TTL.as_secs(),
                MAX_SESSION_TTL.as_secs()
            ),
        )
        .into());
    }
    config.storage_budget_bytes = parse_positive(
        "TASKHARBOR_STORAGE_BUDGET_BYTES",
        config.storage_budget_bytes,
    )?;
    config.max_active_jobs = parse_positive("TASKHARBOR_MAX_ACTIVE_JOBS", config.max_active_jobs)?;
    config.login_attempts_per_minute = parse_positive(
        "TASKHARBOR_LOGIN_ATTEMPTS_PER_MINUTE",
        config.login_attempts_per_minute,
    )?;
    config.uploads_per_minute =
        parse_positive("TASKHARBOR_UPLOADS_PER_MINUTE", config.uploads_per_minute)?;
    let jobs = PgJobRepository::connect(&database_url, 5).await?;
    jobs.migrate().await?;
    initialize_owner(&jobs, &owner_username, &owner_password).await?;
    let storage_dir =
        env::var("TASKHARBOR_STORAGE_DIR").unwrap_or_else(|_| DEFAULT_STORAGE_DIR.to_owned());
    let storage = LocalStorage::initialize(storage_dir).await?;
    let listener = TcpListener::bind(bind_addr).await?;

    info!(%bind_addr, secure_cookies, "TaskHarbor API listening");
    axum::serve(listener, app_with_config(jobs, storage, config))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = signal::ctrl_c().await {
        warn!(%error, "failed to listen for Ctrl+C");
    }
    info!("TaskHarbor API stopped gracefully");
}

fn parse_bool(name: &str, default: bool) -> Result<bool, io::Error> {
    match env::var(name) {
        Ok(value) => value.parse::<bool>().map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be true or false"),
            )
        }),
        Err(env::VarError::NotPresent) => Ok(default),
        Err(error) => Err(io::Error::new(io::ErrorKind::InvalidInput, error)),
    }
}

fn parse_positive<T>(name: &str, default: T) -> Result<T, io::Error>
where
    T: FromStr + PartialEq + Default + Copy + ToString,
{
    let value = env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .parse::<T>()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be a positive integer"),
            )
        })?;
    if value == T::default() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be greater than zero"),
        ));
    }
    Ok(value)
}
