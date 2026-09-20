use std::time::{Duration, Instant};

use taskharbor_adapters::{
    ClaimedJob, ClaimedWork, FailureDisposition, ImageService, LocalStorage, PendingOutputArtifact,
    PgJobRepository, ReclaimDisposition, RepositoryError, WorkerId, WorkerRegistration,
};
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio::time::{Instant as TokioInstant, MissedTickBehavior, interval, sleep, sleep_until};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
pub const DEFAULT_WORKER_CONCURRENCY: usize = 2;
pub const DEFAULT_LEASE_DURATION: Duration = Duration::from_secs(15);
pub const DEFAULT_LEASE_RENEWAL_INTERVAL: Duration = Duration::from_secs(5);
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
pub const DEFAULT_HEARTBEAT_TTL: Duration = Duration::from_secs(10);
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct WorkerConfig {
    pub id: WorkerId,
    pub name: String,
    pub concurrency_limit: usize,
    pub lease_duration: Duration,
    pub lease_renewal_interval: Duration,
    pub heartbeat_interval: Duration,
    pub heartbeat_ttl: Duration,
    pub shutdown_grace: Duration,
}

impl WorkerConfig {
    pub fn new(name: impl Into<String>, concurrency_limit: usize) -> Self {
        Self {
            id: WorkerId::new(),
            name: name.into(),
            concurrency_limit,
            lease_duration: DEFAULT_LEASE_DURATION,
            lease_renewal_interval: DEFAULT_LEASE_RENEWAL_INTERVAL,
            heartbeat_interval: DEFAULT_HEARTBEAT_INTERVAL,
            heartbeat_ttl: DEFAULT_HEARTBEAT_TTL,
            shutdown_grace: DEFAULT_SHUTDOWN_GRACE,
        }
    }

    fn validate(&self) -> Result<(), RepositoryError> {
        if self.lease_renewal_interval.is_zero()
            || self.lease_renewal_interval >= self.lease_duration
        {
            return Err(RepositoryError::InvalidData(
                "lease renewal interval must be positive and shorter than the lease".into(),
            ));
        }
        if self.heartbeat_interval.is_zero() || self.heartbeat_interval >= self.heartbeat_ttl {
            return Err(RepositoryError::InvalidData(
                "heartbeat interval must be positive and shorter than its TTL".into(),
            ));
        }
        if self.shutdown_grace.is_zero() {
            return Err(RepositoryError::InvalidData(
                "shutdown grace period must be greater than zero".into(),
            ));
        }
        Ok(())
    }

    fn registration(&self) -> WorkerRegistration {
        WorkerRegistration {
            id: self.id,
            name: self.name.clone(),
            concurrency_limit: self.concurrency_limit,
            lease_duration: self.lease_duration,
            heartbeat_ttl: self.heartbeat_ttl,
        }
    }
}

impl Default for WorkerConfig {
    fn default() -> Self {
        let id = WorkerId::new();
        let short_id = id.to_string().chars().take(8).collect::<String>();
        let mut config = Self::new(format!("worker-{short_id}"), DEFAULT_WORKER_CONCURRENCY);
        config.id = id;
        config
    }
}

pub async fn run(
    repository: PgJobRepository,
    images: ImageService,
    storage: LocalStorage,
    shutdown: watch::Receiver<bool>,
) -> Result<(), RepositoryError> {
    run_with_config(
        repository,
        images,
        storage,
        shutdown,
        WorkerConfig::default(),
    )
    .await
}

pub async fn run_with_config(
    repository: PgJobRepository,
    images: ImageService,
    storage: LocalStorage,
    mut shutdown: watch::Receiver<bool>,
    config: WorkerConfig,
) -> Result<(), RepositoryError> {
    config.validate()?;
    repository.register_worker(&config.registration()).await?;
    println!(
        "TaskHarbor worker {} ({}) registered with concurrency {}",
        config.name, config.id, config.concurrency_limit
    );

    let mut tasks = JoinSet::new();
    let mut heartbeat = interval(config.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;

    let loop_result = worker_loop(
        &repository,
        &images,
        &storage,
        &mut shutdown,
        &config,
        &mut tasks,
        &mut heartbeat,
    )
    .await;

    let drain_result = drain_tasks(
        &repository,
        config.id,
        config.heartbeat_ttl,
        config.shutdown_grace,
        &mut tasks,
        &mut heartbeat,
    )
    .await;
    let stop_result = repository.stop_worker(config.id).await;
    println!("TaskHarbor worker {} stopped", config.id);

    loop_result?;
    drain_result?;
    stop_result
}

#[allow(clippy::too_many_arguments)]
async fn worker_loop(
    repository: &PgJobRepository,
    images: &ImageService,
    storage: &LocalStorage,
    shutdown: &mut watch::Receiver<bool>,
    config: &WorkerConfig,
    tasks: &mut JoinSet<Result<(), RepositoryError>>,
    heartbeat: &mut tokio::time::Interval,
) -> Result<(), RepositoryError> {
    loop {
        if *shutdown.borrow() {
            println!("Worker {} stopped accepting new jobs", config.id);
            return Ok(());
        }

        reclaim_expired_attempts(repository, storage).await?;

        if let Some(tick) = repository.materialize_next_schedule().await? {
            match tick.job_id() {
                Some(job_id) => println!(
                    "Scheduler created job {job_id} for schedule {} at {} ({} older slots coalesced)",
                    tick.schedule_id(),
                    tick.scheduled_for(),
                    tick.coalesced_slots()
                ),
                None => println!(
                    "Scheduler skipped schedule {} at {} because an earlier occurrence is active",
                    tick.schedule_id(),
                    tick.scheduled_for()
                ),
            }
            continue;
        }

        while tasks.len() < config.concurrency_limit && !*shutdown.borrow() {
            let Some(claimed) = repository.claim_next(config.id).await? else {
                break;
            };
            println!(
                "Worker {} claimed job {} with attempt ID {} and lease through {}",
                config.id,
                claimed.job_id(),
                claimed.attempt_id(),
                claimed.lease_expires_at()
            );
            let repository = repository.clone();
            let images = images.clone();
            let storage = storage.clone();
            let renewal_interval = config.lease_renewal_interval;
            tasks.spawn(async move {
                process_with_lease_renewal(repository, images, storage, claimed, renewal_interval)
                    .await
            });
        }

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    println!("Worker {} stopped accepting new jobs", config.id);
                    return Ok(());
                }
            }
            task = tasks.join_next(), if !tasks.is_empty() => {
                handle_task_result(task)?;
            }
            _ = heartbeat.tick() => {
                repository.heartbeat_worker(config.id, config.heartbeat_ttl).await?;
            }
            () = sleep(POLL_INTERVAL) => {}
        }
    }
}

async fn process_with_lease_renewal(
    repository: PgJobRepository,
    images: ImageService,
    storage: LocalStorage,
    claimed: ClaimedJob,
    renewal_interval: Duration,
) -> Result<(), RepositoryError> {
    let mut renewal = interval(renewal_interval);
    renewal.set_missed_tick_behavior(MissedTickBehavior::Delay);
    renewal.tick().await;
    let processing = process_claimed_inner(&repository, &images, &storage, &claimed);
    tokio::pin!(processing);

    let result = loop {
        tokio::select! {
            result = &mut processing => break result,
            _ = renewal.tick() => {
                match repository.renew_lease(&claimed).await {
                    Ok(lease_expires_at) => println!(
                        "Worker {} renewed job {} attempt {} through {}",
                        claimed.worker_id(),
                        claimed.job_id(),
                        claimed.attempt_number(),
                        lease_expires_at
                    ),
                    Err(RepositoryError::ClaimLost) => {
                        let result = processing.await;
                        break result.and(Err(RepositoryError::ClaimLost));
                    }
                    Err(error) => {
                        eprintln!(
                            "Worker {} could not renew job {} attempt {}: {error}",
                            claimed.worker_id(),
                            claimed.job_id(),
                            claimed.attempt_number()
                        );
                    }
                }
            }
        }
    };

    finish_claimed_result(&storage, &claimed, result).await
}

async fn drain_tasks(
    repository: &PgJobRepository,
    worker_id: WorkerId,
    heartbeat_ttl: Duration,
    grace: Duration,
    tasks: &mut JoinSet<Result<(), RepositoryError>>,
    heartbeat: &mut tokio::time::Interval,
) -> Result<(), RepositoryError> {
    if tasks.is_empty() {
        return Ok(());
    }

    println!(
        "Worker {worker_id} is draining {} active job(s) for up to {} seconds",
        tasks.len(),
        grace.as_secs()
    );
    let deadline = TokioInstant::now() + grace;
    loop {
        if tasks.is_empty() {
            println!("Worker {worker_id} drained all active jobs");
            return Ok(());
        }

        tokio::select! {
            task = tasks.join_next() => handle_task_result(task)?,
            _ = heartbeat.tick() => {
                repository.heartbeat_worker(worker_id, heartbeat_ttl).await?;
            }
            () = sleep_until(deadline) => {
                let abandoned = tasks.len();
                tasks.abort_all();
                while let Some(task) = tasks.join_next().await {
                    if let Err(error) = task
                        && !error.is_cancelled()
                    {
                        return Err(join_error(error));
                    }
                }
                println!(
                    "Worker {worker_id} ended {abandoned} task(s) after the shutdown grace period"
                );
                return Ok(());
            }
        }
    }
}

fn handle_task_result(
    task: Option<Result<Result<(), RepositoryError>, JoinError>>,
) -> Result<(), RepositoryError> {
    match task {
        Some(Ok(result)) => result,
        Some(Err(error)) => Err(join_error(error)),
        None => Ok(()),
    }
}

fn join_error(error: JoinError) -> RepositoryError {
    RepositoryError::InvalidData(format!("worker task terminated unexpectedly: {error}"))
}

async fn reclaim_expired_attempts(
    repository: &PgJobRepository,
    storage: &LocalStorage,
) -> Result<(), RepositoryError> {
    while let Some(reclaimed) = repository.reclaim_next_expired_attempt().await? {
        let prefix =
            storage.attempt_output_prefix(reclaimed.job_id().get(), reclaimed.attempt_id());
        if let Err(error) = storage.remove_tree(&prefix).await {
            eprintln!(
                "Could not clean expired output for job {} attempt {}: {error}",
                reclaimed.job_id(),
                reclaimed.attempt_id()
            );
        }
        match reclaimed.disposition() {
            ReclaimDisposition::RetryScheduled { available_at } => println!(
                "Reclaimed job {} attempt {} from worker {}; retry available at {}",
                reclaimed.job_id(),
                reclaimed.attempt_id(),
                reclaimed.worker_id(),
                available_at
            ),
            ReclaimDisposition::Failed => println!(
                "Reclaimed job {} attempt {} from worker {}; retry limit exhausted",
                reclaimed.job_id(),
                reclaimed.attempt_id(),
                reclaimed.worker_id()
            ),
            ReclaimDisposition::Cancelled => println!(
                "Reclaimed and cancelled job {} attempt {} from worker {}",
                reclaimed.job_id(),
                reclaimed.attempt_id(),
                reclaimed.worker_id()
            ),
        }
    }
    Ok(())
}

pub async fn process_next(
    repository: &PgJobRepository,
    images: &ImageService,
    storage: &LocalStorage,
) -> Result<bool, RepositoryError> {
    let config = WorkerConfig::default();
    repository.register_worker(&config.registration()).await?;
    let result = process_next_for_worker(repository, images, storage, config.id).await;
    let stopped = repository.stop_worker(config.id).await;
    match (result, stopped) {
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        (Ok(processed), Ok(())) => Ok(processed),
    }
}

pub async fn process_next_for_worker(
    repository: &PgJobRepository,
    images: &ImageService,
    storage: &LocalStorage,
    worker_id: WorkerId,
) -> Result<bool, RepositoryError> {
    let Some(claimed) = repository.claim_next(worker_id).await? else {
        return Ok(false);
    };
    process_claimed(repository, images, storage, claimed).await?;
    Ok(true)
}

pub async fn process_claimed(
    repository: &PgJobRepository,
    images: &ImageService,
    storage: &LocalStorage,
    claimed: ClaimedJob,
) -> Result<(), RepositoryError> {
    let result = process_claimed_inner(repository, images, storage, &claimed).await;
    finish_claimed_result(storage, &claimed, result).await
}

async fn finish_claimed_result(
    storage: &LocalStorage,
    claimed: &ClaimedJob,
    result: Result<(), RepositoryError>,
) -> Result<(), RepositoryError> {
    if matches!(&result, Err(RepositoryError::ClaimLost)) {
        cleanup_attempt_outputs(storage, claimed).await;
        println!(
            "Worker {} discarded stale output for job {} attempt {}",
            claimed.worker_id(),
            claimed.job_id(),
            claimed.attempt_number()
        );
        return Ok(());
    }
    result
}

async fn process_claimed_inner(
    repository: &PgJobRepository,
    images: &ImageService,
    storage: &LocalStorage,
    claimed: &ClaimedJob,
) -> Result<(), RepositoryError> {
    let started_at = Instant::now();
    match claimed.work().clone() {
        ClaimedWork::DemoDelay { delay } => {
            sleep(delay).await;
            if finish_if_cancel_requested(repository, storage, claimed, started_at).await? {
                return Ok(());
            }

            match repository.complete(claimed, started_at.elapsed()).await {
                Ok(()) => {}
                Err(error @ RepositoryError::StateConflict(_)) => {
                    if !finish_if_cancel_requested(repository, storage, claimed, started_at).await?
                    {
                        return Err(error);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        ClaimedWork::ImageResize {
            max_width,
            jpeg_quality,
            inputs,
        } => {
            process_images(
                repository,
                images,
                storage,
                claimed,
                inputs,
                max_width,
                jpeg_quality,
                started_at,
            )
            .await?;
        }
    }

    println!(
        "Worker finished attempt {} for job {}",
        claimed.attempt_number(),
        claimed.job_id()
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn process_images(
    repository: &PgJobRepository,
    images: &ImageService,
    storage: &LocalStorage,
    claimed: &ClaimedJob,
    inputs: Vec<taskharbor_adapters::ArtifactRecord>,
    max_width: u32,
    jpeg_quality: u8,
    started_at: Instant,
) -> Result<(), RepositoryError> {
    let mut outputs = Vec::with_capacity(inputs.len());

    for (completed, input) in inputs.iter().enumerate() {
        if finish_if_cancel_requested(repository, storage, claimed, started_at).await? {
            return Ok(());
        }

        let output_key =
            storage.output_key(claimed.job_id().get(), claimed.attempt_id(), input.id());
        let processed = match images
            .resize_to_jpeg(input.storage_key(), output_key, max_width, jpeg_quality)
            .await
        {
            Ok(processed) => processed,
            Err(error) => {
                eprintln!(
                    "Worker failed image job {} on item {}: {error}",
                    claimed.job_id(),
                    input.item_index()
                );
                cleanup_attempt_outputs(storage, claimed).await;
                let failure = repository
                    .record_failure(
                        claimed,
                        started_at.elapsed(),
                        error.failure_kind(),
                        error.safe_message(),
                    )
                    .await;
                match failure {
                    Ok(FailureDisposition::RetryScheduled { available_at }) => println!(
                        "Worker scheduled attempt {} for job {} after {}",
                        claimed.attempt_number() + 1,
                        claimed.job_id(),
                        available_at
                    ),
                    Ok(FailureDisposition::Failed) => println!(
                        "Worker permanently failed job {} on attempt {}",
                        claimed.job_id(),
                        claimed.attempt_number()
                    ),
                    Err(error @ RepositoryError::StateConflict(_)) => {
                        if !finish_if_cancel_requested(repository, storage, claimed, started_at)
                            .await?
                        {
                            return Err(error);
                        }
                    }
                    Err(error) => return Err(error),
                }
                return Ok(());
            }
        };

        if finish_if_cancel_requested(repository, storage, claimed, started_at).await? {
            return Ok(());
        }

        outputs.push(PendingOutputArtifact {
            source_artifact_id: input.id(),
            item_index: input.item_index(),
            storage_key: processed.storage_key,
            display_name: output_name(input.display_name()),
            media_type: processed.media_type.to_owned(),
            byte_size: processed.byte_size,
            width: processed.width,
            height: processed.height,
        });
        let progress = repository
            .update_progress(
                claimed,
                u32::try_from(completed + 1).map_err(|_| {
                    RepositoryError::InvalidData("image progress exceeds u32".into())
                })?,
            )
            .await;
        if let Err(error) = progress {
            if matches!(&error, RepositoryError::StateConflict(_))
                && finish_if_cancel_requested(repository, storage, claimed, started_at).await?
            {
                return Ok(());
            }
            return Err(error);
        }
    }

    if finish_if_cancel_requested(repository, storage, claimed, started_at).await? {
        return Ok(());
    }

    match repository
        .complete_image_job(claimed, started_at.elapsed(), outputs)
        .await
    {
        Ok(()) => Ok(()),
        Err(error @ RepositoryError::StateConflict(_)) => {
            if finish_if_cancel_requested(repository, storage, claimed, started_at).await? {
                Ok(())
            } else {
                Err(error)
            }
        }
        Err(error) => Err(error),
    }
}

async fn finish_if_cancel_requested(
    repository: &PgJobRepository,
    storage: &LocalStorage,
    claimed: &ClaimedJob,
    started_at: Instant,
) -> Result<bool, RepositoryError> {
    if !repository.cancellation_requested(claimed).await? {
        return Ok(false);
    }

    cleanup_attempt_outputs(storage, claimed).await;
    repository
        .finish_cancelled(claimed, started_at.elapsed())
        .await?;
    println!(
        "Worker cancelled job {} after attempt {} reached a safe boundary",
        claimed.job_id(),
        claimed.attempt_number()
    );
    Ok(true)
}

async fn cleanup_attempt_outputs(storage: &LocalStorage, claimed: &ClaimedJob) {
    let prefix = storage.attempt_output_prefix(claimed.job_id().get(), claimed.attempt_id());
    if let Err(error) = storage.remove_tree(&prefix).await {
        eprintln!(
            "Worker could not clean attempt output for job {}: {error}",
            claimed.job_id()
        );
    }
}

fn output_name(input_name: &str) -> String {
    let stem = input_name
        .rsplit_once('.')
        .map_or(input_name, |(stem, _)| stem)
        .trim();
    let stem = if stem.is_empty() { "image" } else { stem };
    let stem = stem.chars().take(251).collect::<String>();
    format!("{stem}.jpg")
}

#[cfg(test)]
mod tests {
    use super::output_name;

    #[test]
    fn output_filename_stays_within_the_database_limit() {
        let input = "a".repeat(255);
        let output = output_name(&input);

        assert_eq!(output.chars().count(), 255);
        assert!(output.ends_with(".jpg"));
    }
}
