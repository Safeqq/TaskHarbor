use std::env;
use std::error::Error;
use std::io;

use taskharbor_adapters::{ImageService, LocalStorage, PgJobRepository};
use taskharbor_worker::{
    DEFAULT_HEARTBEAT_INTERVAL, DEFAULT_HEARTBEAT_TTL, DEFAULT_LEASE_DURATION,
    DEFAULT_LEASE_RENEWAL_INTERVAL, DEFAULT_SHUTDOWN_GRACE, DEFAULT_WORKER_CONCURRENCY,
    WorkerConfig, run_with_config,
};
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
    let concurrency =
        parse_positive_usize("TASKHARBOR_WORKER_CONCURRENCY", DEFAULT_WORKER_CONCURRENCY)?;
    let max_connections = u32::try_from(concurrency.saturating_add(4)).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "TASKHARBOR_WORKER_CONCURRENCY is too large",
        )
    })?;
    let repository = PgJobRepository::connect(&database_url, max_connections).await?;
    repository.migrate().await?;
    let storage_dir =
        env::var("TASKHARBOR_STORAGE_DIR").unwrap_or_else(|_| DEFAULT_STORAGE_DIR.to_owned());
    let storage = LocalStorage::initialize(storage_dir).await?;
    let max_blocking_tasks =
        parse_positive_usize("TASKHARBOR_MAX_BLOCKING_TASKS", DEFAULT_MAX_BLOCKING_TASKS)?;
    let images = ImageService::new(storage.clone(), max_blocking_tasks);
    let default_config = WorkerConfig::default();
    let config = WorkerConfig {
        id: default_config.id,
        name: env::var("TASKHARBOR_WORKER_NAME").unwrap_or(default_config.name),
        concurrency_limit: concurrency,
        lease_duration: parse_duration("TASKHARBOR_LEASE_SECONDS", DEFAULT_LEASE_DURATION)?,
        lease_renewal_interval: parse_duration(
            "TASKHARBOR_LEASE_RENEWAL_SECONDS",
            DEFAULT_LEASE_RENEWAL_INTERVAL,
        )?,
        heartbeat_interval: parse_duration(
            "TASKHARBOR_HEARTBEAT_SECONDS",
            DEFAULT_HEARTBEAT_INTERVAL,
        )?,
        heartbeat_ttl: parse_duration("TASKHARBOR_HEARTBEAT_TTL_SECONDS", DEFAULT_HEARTBEAT_TTL)?,
        shutdown_grace: parse_duration(
            "TASKHARBOR_SHUTDOWN_GRACE_SECONDS",
            DEFAULT_SHUTDOWN_GRACE,
        )?,
    };

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let signal_task = tokio::spawn(async move {
        if let Err(error) = signal::ctrl_c().await {
            eprintln!("Failed to listen for Ctrl+C: {error}");
        }
        let _ = shutdown_sender.send(true);
    });

    let result = run_with_config(repository, images, storage, shutdown_receiver, config).await;
    signal_task.abort();
    result?;

    Ok(())
}

fn parse_positive_usize(name: &str, default: usize) -> Result<usize, io::Error> {
    let value = env::var(name)
        .unwrap_or_else(|_| default.to_string())
        .parse::<usize>()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be a positive integer"),
            )
        })?;
    if value == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be greater than zero"),
        ));
    }
    Ok(value)
}

fn parse_duration(
    name: &str,
    default: std::time::Duration,
) -> Result<std::time::Duration, io::Error> {
    let seconds = env::var(name)
        .unwrap_or_else(|_| default.as_secs().to_string())
        .parse::<u64>()
        .map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{name} must be a positive number of seconds"),
            )
        })?;
    if seconds == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be greater than zero"),
        ));
    }
    Ok(std::time::Duration::from_secs(seconds))
}
