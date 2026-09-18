use std::time::{Duration, Instant};

use taskharbor_adapters::{PgJobRepository, RepositoryError};
use tokio::sync::watch;
use tokio::time::sleep;

const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub async fn run(
    repository: PgJobRepository,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), RepositoryError> {
    loop {
        let shutdown_requested = *shutdown.borrow();
        if shutdown_requested {
            println!("TaskHarbor worker stopped gracefully");
            return Ok(());
        }

        if process_next(&repository).await? {
            continue;
        }

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    println!("TaskHarbor worker stopped gracefully");
                    return Ok(());
                }
            }
            () = sleep(POLL_INTERVAL) => {}
        }
    }
}

pub async fn process_next(repository: &PgJobRepository) -> Result<bool, RepositoryError> {
    let Some(claimed) = repository.claim_next().await? else {
        return Ok(false);
    };

    println!(
        "Worker claimed job {} with attempt ID {}",
        claimed.job_id(),
        claimed.attempt_id()
    );

    let started_at = Instant::now();
    sleep(claimed.delay()).await;
    repository.complete(&claimed, started_at.elapsed()).await?;

    println!("Worker completed job {}", claimed.job_id());
    Ok(true)
}
