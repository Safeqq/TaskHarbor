mod error;
mod http;

use axum::Router;
use taskharbor_adapters::PgJobRepository;

#[derive(Debug, Clone)]
pub struct AppState {
    jobs: PgJobRepository,
}

pub fn app(jobs: PgJobRepository) -> Router {
    http::router(AppState { jobs })
}
