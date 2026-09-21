mod auth;
mod error;
mod http;
mod rate_limit;
mod upload;

use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use taskharbor_adapters::{ImageService, LocalStorage, PgJobRepository};
use tokio::sync::Mutex;

pub use auth::{OwnerSetupError, initialize_owner};
use rate_limit::RateLimiter;

pub const DEFAULT_SESSION_TTL: Duration = Duration::from_secs(12 * 60 * 60);
pub const DEFAULT_STORAGE_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;
pub const DEFAULT_MAX_ACTIVE_JOBS: u32 = 100;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub session_ttl: Duration,
    pub secure_cookies: bool,
    pub storage_budget_bytes: u64,
    pub max_active_jobs: u32,
    pub login_attempts_per_minute: usize,
    pub uploads_per_minute: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            session_ttl: DEFAULT_SESSION_TTL,
            secure_cookies: false,
            storage_budget_bytes: DEFAULT_STORAGE_BUDGET_BYTES,
            max_active_jobs: DEFAULT_MAX_ACTIVE_JOBS,
            login_attempts_per_minute: 5,
            uploads_per_minute: 10,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) jobs: PgJobRepository,
    pub(crate) storage: LocalStorage,
    pub(crate) images: ImageService,
    pub(crate) config: AppConfig,
    pub(crate) login_limiter: RateLimiter,
    pub(crate) upload_limiter: RateLimiter,
    pub(crate) upload_guard: Arc<Mutex<()>>,
}

pub fn app(jobs: PgJobRepository, storage: LocalStorage) -> Router {
    app_with_config(jobs, storage, AppConfig::default())
}

pub fn app_with_config(jobs: PgJobRepository, storage: LocalStorage, config: AppConfig) -> Router {
    let images = ImageService::new(storage.clone(), 2);
    let login_limiter = RateLimiter::new(config.login_attempts_per_minute, Duration::from_secs(60));
    let upload_limiter = RateLimiter::new(config.uploads_per_minute, Duration::from_secs(60));
    http::router(AppState {
        jobs,
        storage,
        images,
        config,
        login_limiter,
        upload_limiter,
        upload_guard: Arc::new(Mutex::new(())),
    })
}
