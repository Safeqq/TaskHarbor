use std::env;
use std::error::Error;
use std::io;

use taskharbor_adapters::{ImageService, LocalStorage, PgJobRepository};
use taskharbor_worker::run;
use tokio::signal;
use tokio::sync::watch;

const DEFAULT_STORAGE_DIR: &str = "var/storage";
const DEFAULT_MAX_BLOCKING_TASKS: usize = 2;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let database_url = env::var("DATABASE_URL").map_err(|_| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "DATABASE_URL must be set before starting the worker",
        )
    })?;
    let repository = PgJobRepository::connect(&database_url, 3).await?;
    repository.migrate().await?;
    let storage_dir =
        env::var("TASKHARBOR_STORAGE_DIR").unwrap_or_else(|_| DEFAULT_STORAGE_DIR.to_owned());
    let storage = LocalStorage::initialize(storage_dir).await?;
    let max_blocking_tasks = env::var("TASKHARBOR_MAX_BLOCKING_TASKS")
        .unwrap_or_else(|_| DEFAULT_MAX_BLOCKING_TASKS.to_string())
        .parse::<usize>()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "TASKHARBOR_MAX_BLOCKING_TASKS must be a positive integer",
            )
        })?;
    if max_blocking_tasks == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "TASKHARBOR_MAX_BLOCKING_TASKS must be greater than zero",
        )
        .into());
    }
    let images = ImageService::new(storage.clone(), max_blocking_tasks);

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let signal_task = tokio::spawn(async move {
        if let Err(error) = signal::ctrl_c().await {
            eprintln!("Failed to listen for Ctrl+C: {error}");
        }
        let _ = shutdown_sender.send(true);
    });

    println!("TaskHarbor worker started");
    let result = run(repository, images, storage, shutdown_receiver).await;
    signal_task.abort();
    result?;

    Ok(())
}
