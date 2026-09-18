use std::env;
use std::error::Error;
use std::io;

use taskharbor_adapters::PgJobRepository;
use taskharbor_worker::run;
use tokio::signal;
use tokio::sync::watch;

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

    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let signal_task = tokio::spawn(async move {
        if let Err(error) = signal::ctrl_c().await {
            eprintln!("Failed to listen for Ctrl+C: {error}");
        }
        let _ = shutdown_sender.send(true);
    });

    println!("TaskHarbor worker started");
    let result = run(repository, shutdown_receiver).await;
    signal_task.abort();
    result?;

    Ok(())
}
