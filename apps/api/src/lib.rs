mod error;
mod http;
mod store;

use axum::Router;

use store::MemoryJobStore;

#[derive(Debug, Clone, Default)]
pub struct AppState {
    jobs: MemoryJobStore,
}

pub fn app() -> Router {
    http::router(AppState::default())
}
