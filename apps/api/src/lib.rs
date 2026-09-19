mod error;
mod http;
mod upload;

use axum::Router;
use taskharbor_adapters::{ImageService, LocalStorage, PgJobRepository};

#[derive(Debug, Clone)]
pub struct AppState {
    pub(crate) jobs: PgJobRepository,
    pub(crate) storage: LocalStorage,
    pub(crate) images: ImageService,
}

pub fn app(jobs: PgJobRepository, storage: LocalStorage) -> Router {
    let images = ImageService::new(storage.clone(), 2);
    http::router(AppState {
        jobs,
        storage,
        images,
    })
}
