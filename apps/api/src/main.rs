use std::env;
use std::error::Error;
use std::io;
use std::net::SocketAddr;

use taskharbor_adapters::{LocalStorage, PgJobRepository};
use taskharbor_api::app;
use tokio::{net::TcpListener, signal};

const DEFAULT_BIND_ADDR: &str = "127.0.0.1:3000";
const DEFAULT_STORAGE_DIR: &str = "var/storage";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let database_url = env::var("DATABASE_URL").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "DATABASE_URL must be set before starting the API",
        )
    })?;
    let bind_addr =
        env::var("TASKHARBOR_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());
    let bind_addr = bind_addr.parse::<SocketAddr>()?;
    let jobs = PgJobRepository::connect(&database_url, 5).await?;
    jobs.migrate().await?;
    let storage_dir =
        env::var("TASKHARBOR_STORAGE_DIR").unwrap_or_else(|_| DEFAULT_STORAGE_DIR.to_owned());
    let storage = LocalStorage::initialize(storage_dir).await?;
    let listener = TcpListener::bind(bind_addr).await?;

    println!("TaskHarbor API listening on http://{bind_addr}");
    axum::serve(listener, app(jobs, storage))
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    if let Err(error) = signal::ctrl_c().await {
        eprintln!("Failed to listen for Ctrl+C: {error}");
    }
    println!("TaskHarbor API stopped gracefully");
}
