use std::time::{Duration, Instant, SystemTime};

use taskharbor_adapters::{
    ClaimedJob, ClaimedWork, FailureDisposition, FailureKind, ImageService, LocalStorage,
    PendingOutputArtifact, PgJobRepository, ReclaimDisposition, RepositoryError, WorkerId,
    WorkerRegistration,
};
use tokio::sync::watch;
use tokio::task::{JoinError, JoinSet};
use tokio::time::{Instant as TokioInstant, MissedTickBehavior, interval, sleep, sleep_until};
use tracing::{debug, error, info, warn};

const POLL_INTERVAL: Duration = Duration::from_millis(250);
pub const DEFAULT_WORKER_CONCURRENCY: usize = 2;
pub const DEFAULT_LEASE_DURATION: Duration = Duration::from_secs(15);
pub const DEFAULT_LEASE_RENEWAL_INTERVAL: Duration = Duration::from_secs(5);
pub const DEFAULT_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(2);
pub const DEFAULT_HEARTBEAT_TTL: Duration = Duration::from_secs(10);
pub const DEFAULT_SHUTDOWN_GRACE: Duration = Duration::from_secs(30);
pub const DEFAULT_MAINTENANCE_INTERVAL: Duration = Duration::from_secs(60 * 60);
pub const DEFAULT_OUTPUT_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
pub const DEFAULT_ORPHAN_GRACE: Duration = Duration::from_secs(60 * 60);
pub const DEFAULT_STORAGE_BUDGET_BYTES: u64 = 1024 * 1024 * 1024;

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
    pub maintenance_interval: Duration,
    pub output_retention: Duration,
    pub orphan_grace: Duration,
    pub storage_budget_bytes: u64,
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
            maintenance_interval: DEFAULT_MAINTENANCE_INTERVAL,
            output_retention: DEFAULT_OUTPUT_RETENTION,
            orphan_grace: DEFAULT_ORPHAN_GRACE,
            storage_budget_bytes: DEFAULT_STORAGE_BUDGET_BYTES,
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
        if self.maintenance_interval.is_zero()
            || self.output_retention.is_zero()
            || self.orphan_grace.is_zero()
            || self.storage_budget_bytes == 0
        {
            return Err(RepositoryError::InvalidData(
                "maintenance intervals and storage budget must be greater than zero".into(),
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
    info!(
        event = "worker_registered",
        worker_id = %config.id,
        worker_name = %config.name,
        concurrency = config.concurrency_limit,
        "worker registered"
    );

    if let Err(error) = run_maintenance(&repository, &storage, &config).await {
        warn!(%error, "initial maintenance failed");
    }

    let mut tasks = JoinSet::new();
    let mut heartbeat = interval(config.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let mut maintenance = interval(config.maintenance_interval);
    maintenance.set_missed_tick_behavior(MissedTickBehavior::Delay);
    maintenance.tick().await;

    let loop_result = worker_loop(
        &repository,
        &images,
        &storage,
        &mut shutdown,
        &config,
        &mut tasks,
        &mut heartbeat,
        &mut maintenance,
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
    info!(event = "worker_stopped", worker_id = %config.id, "worker stopped");

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
    maintenance: &mut tokio::time::Interval,
) -> Result<(), RepositoryError> {
    loop {
        if *shutdown.borrow() {
            info!(worker_id = %config.id, "worker stopped accepting new jobs");
            return Ok(());
        }

        reclaim_expired_attempts(repository, storage).await?;

        if let Some(tick) = repository.materialize_next_schedule().await? {
            match tick.job_id() {
                Some(job_id) => info!(
                    event = "schedule_materialized",
                    %job_id,
                    schedule_id = %tick.schedule_id(),
                    scheduled_for = %tick.scheduled_for(),
                    coalesced_slots = tick.coalesced_slots(),
                    "scheduler created job"
                ),
                None => info!(
                    event = "schedule_overlap_skipped",
                    schedule_id = %tick.schedule_id(),
                    scheduled_for = %tick.scheduled_for(),
                    "scheduler skipped active overlap"
                ),
            }
            continue;
        }

        while tasks.len() < config.concurrency_limit && !*shutdown.borrow() {
            let Some(claimed) = repository.claim_next(config.id).await? else {
                break;
            };
            info!(
                event = "job_claimed",
                worker_id = %config.id,
                job_id = %claimed.job_id(),
                attempt_id = claimed.attempt_id(),
                attempt_number = claimed.attempt_number(),
                queue_wait_ms = claimed.queue_wait().as_millis(),
                lease_expires_at = %claimed.lease_expires_at(),
                "worker claimed job"
            );
            let repository = repository.clone();
            let images = images.clone();
            let storage = storage.clone();
            let renewal_interval = config.lease_renewal_interval;
            let storage_budget_bytes = config.storage_budget_bytes;
            tasks.spawn(async move {
                process_with_lease_renewal(
                    repository,
                    images,
                    storage,
                    claimed,
                    renewal_interval,
                    storage_budget_bytes,
                )
                .await
            });
        }

        tokio::select! {
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    info!(worker_id = %config.id, "worker stopped accepting new jobs");
                    return Ok(());
                }
            }
            task = tasks.join_next(), if !tasks.is_empty() => {
                handle_task_result(task)?;
            }
            _ = heartbeat.tick() => {
                repository.heartbeat_worker(config.id, config.heartbeat_ttl).await?;
            }
            _ = maintenance.tick() => {
                if let Err(error) = run_maintenance(repository, storage, config).await {
                    warn!(%error, worker_id = %config.id, "scheduled maintenance failed");
                }
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
    storage_budget_bytes: u64,
) -> Result<(), RepositoryError> {
    let mut renewal = interval(renewal_interval);
    renewal.set_missed_tick_behavior(MissedTickBehavior::Delay);
    renewal.tick().await;
    let processing = process_claimed_inner(
        &repository,
        &images,
        &storage,
        &claimed,
        storage_budget_bytes,
    );
    tokio::pin!(processing);

    let result = loop {
        tokio::select! {
            result = &mut processing => break result,
            _ = renewal.tick() => {
                match repository.renew_lease(&claimed).await {
                    Ok(lease_expires_at) => debug!(
                        event = "lease_renewed",
                        worker_id = %claimed.worker_id(),
                        job_id = %claimed.job_id(),
                        attempt_id = claimed.attempt_id(),
                        attempt_number = claimed.attempt_number(),
                        lease_expires_at = %lease_expires_at,
                        "attempt lease renewed"
                    ),
                    Err(RepositoryError::ClaimLost) => {
                        let result = processing.await;
                        break result.and(Err(RepositoryError::ClaimLost));
                    }
                    Err(error) => {
                        warn!(
                            %error,
                            worker_id = %claimed.worker_id(),
                            job_id = %claimed.job_id(),
                            attempt_id = claimed.attempt_id(),
                            attempt_number = claimed.attempt_number(),
                            "attempt lease could not be renewed"
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

    info!(
        %worker_id,
        active_jobs = tasks.len(),
        grace_seconds = grace.as_secs(),
        "worker draining active jobs"
    );
    let deadline = TokioInstant::now() + grace;
    loop {
        if tasks.is_empty() {
            info!(%worker_id, "worker drained all active jobs");
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
                warn!(%worker_id, abandoned, "worker ended tasks after shutdown grace period");
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
            error!(
                %error,
                job_id = %reclaimed.job_id(),
                attempt_id = reclaimed.attempt_id(),
                "expired attempt output could not be cleaned"
            );
        }
        match reclaimed.disposition() {
            ReclaimDisposition::RetryScheduled { available_at } => warn!(
                event = "expired_attempt_reclaimed",
                job_id = %reclaimed.job_id(),
                attempt_id = reclaimed.attempt_id(),
                worker_id = %reclaimed.worker_id(),
                disposition = "retry_scheduled",
                available_at = %available_at,
                "expired attempt reclaimed"
            ),
            ReclaimDisposition::Failed => warn!(
                event = "expired_attempt_reclaimed",
                job_id = %reclaimed.job_id(),
                attempt_id = reclaimed.attempt_id(),
                worker_id = %reclaimed.worker_id(),
                disposition = "failed",
                "expired attempt reclaimed"
            ),
            ReclaimDisposition::Cancelled => warn!(
                event = "expired_attempt_reclaimed",
                job_id = %reclaimed.job_id(),
                attempt_id = reclaimed.attempt_id(),
                worker_id = %reclaimed.worker_id(),
                disposition = "cancelled",
                "expired attempt reclaimed"
            ),
        }
    }
    Ok(())
}

async fn run_maintenance(
    repository: &PgJobRepository,
    storage: &LocalStorage,
    config: &WorkerConfig,
) -> Result<(), String> {
    let expired = repository
        .expire_output_artifacts(config.output_retention)
        .await
        .map_err(|error| error.to_string())?;
    for key in &expired {
        if let Err(storage_error) = storage.remove_file(key).await {
            warn!(%storage_error, storage_key = %key, "expired output file could not be removed");
        }
    }

    let references = repository
        .storage_references()
        .await
        .map_err(|error| error.to_string())?;
    let protected_prefixes = repository
        .active_attempt_output_prefixes()
        .await
        .map_err(|error| error.to_string())?;
    let cutoff = SystemTime::now()
        .checked_sub(config.orphan_grace)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let cleaned = storage
        .cleanup_unreferenced(references, protected_prefixes, cutoff)
        .await
        .map_err(|error| error.to_string())?;
    let sessions_purged = repository
        .purge_expired_sessions()
        .await
        .map_err(|error| error.to_string())?;
    let queue = repository
        .queue_metrics()
        .await
        .map_err(|error| error.to_string())?;
    let storage_bytes = storage
        .usage_bytes()
        .await
        .map_err(|error| error.to_string())?;
    info!(
        event = "maintenance_completed",
        worker_id = %config.id,
        queue_depth = queue.depth,
        oldest_queue_wait_ms = queue.oldest_wait_ms,
        expired_outputs = expired.len(),
        orphan_files_removed = cleaned.files_removed,
        orphan_bytes_removed = cleaned.bytes_removed,
        sessions_purged,
        storage_bytes,
        storage_budget_bytes = config.storage_budget_bytes,
        "worker maintenance completed"
    );
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
    let result = process_claimed_inner(repository, images, storage, &claimed, u64::MAX).await;
    finish_claimed_result(storage, &claimed, result).await
}

async fn finish_claimed_result(
    storage: &LocalStorage,
    claimed: &ClaimedJob,
    result: Result<(), RepositoryError>,
) -> Result<(), RepositoryError> {
    if matches!(&result, Err(RepositoryError::ClaimLost)) {
        cleanup_attempt_outputs(storage, claimed).await;
        warn!(
            event = "stale_output_discarded",
            worker_id = %claimed.worker_id(),
            job_id = %claimed.job_id(),
            attempt_id = claimed.attempt_id(),
            attempt_number = claimed.attempt_number(),
            "worker discarded stale output"
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
    storage_budget_bytes: u64,
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
                storage_budget_bytes,
            )
            .await?;
        }
    }

    info!(
        event = "attempt_finished",
        job_id = %claimed.job_id(),
        attempt_id = claimed.attempt_id(),
        attempt_number = claimed.attempt_number(),
        worker_id = %claimed.worker_id(),
        duration_ms = started_at.elapsed().as_millis(),
        "worker finished attempt"
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
    storage_budget_bytes: u64,
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
            Err(image_error) => {
                warn!(
                    event = "image_processing_failed",
                    job_id = %claimed.job_id(),
                    attempt_id = claimed.attempt_id(),
                    item_index = input.item_index(),
                    error = %image_error,
                    "image item failed"
                );
                record_image_failure(
                    repository,
                    storage,
                    claimed,
                    started_at,
                    image_error.failure_kind(),
                    image_error.safe_message(),
                )
                .await?;
                return Ok(());
            }
        };

        match storage.usage_bytes().await {
            Ok(usage) if usage > storage_budget_bytes => {
                record_image_failure(
                    repository,
                    storage,
                    claimed,
                    started_at,
                    FailureKind::Permanent,
                    "storage budget exceeded while writing output",
                )
                .await?;
                return Ok(());
            }
            Ok(_) => {}
            Err(storage_error) => {
                warn!(%storage_error, "could not measure storage usage");
                record_image_failure(
                    repository,
                    storage,
                    claimed,
                    started_at,
                    FailureKind::Transient,
                    "storage usage could not be measured",
                )
                .await?;
                return Ok(());
            }
        }

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

async fn record_image_failure(
    repository: &PgJobRepository,
    storage: &LocalStorage,
    claimed: &ClaimedJob,
    started_at: Instant,
    kind: FailureKind,
    message: &str,
) -> Result<(), RepositoryError> {
    cleanup_attempt_outputs(storage, claimed).await;
    let failure = repository
        .record_failure(claimed, started_at.elapsed(), kind, message)
        .await;
    match failure {
        Ok(FailureDisposition::RetryScheduled { available_at }) => info!(
            event = "attempt_retry_scheduled",
            job_id = %claimed.job_id(),
            attempt_id = claimed.attempt_id(),
            attempt_number = claimed.attempt_number(),
            worker_id = %claimed.worker_id(),
            duration_ms = started_at.elapsed().as_millis(),
            available_at = %available_at,
            failure_kind = kind.as_str(),
            "job retry scheduled"
        ),
        Ok(FailureDisposition::Failed) => warn!(
            event = "attempt_failed",
            job_id = %claimed.job_id(),
            attempt_id = claimed.attempt_id(),
            attempt_number = claimed.attempt_number(),
            worker_id = %claimed.worker_id(),
            duration_ms = started_at.elapsed().as_millis(),
            failure_kind = kind.as_str(),
            "job permanently failed"
        ),
        Err(error @ RepositoryError::StateConflict(_)) => {
            if !finish_if_cancel_requested(repository, storage, claimed, started_at).await? {
                return Err(error);
            }
        }
        Err(error) => return Err(error),
    }
    Ok(())
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
    info!(
        event = "attempt_cancelled",
        job_id = %claimed.job_id(),
        attempt_id = claimed.attempt_id(),
        attempt_number = claimed.attempt_number(),
        worker_id = %claimed.worker_id(),
        duration_ms = started_at.elapsed().as_millis(),
        "worker cancelled job at safe boundary"
    );
    Ok(true)
}

async fn cleanup_attempt_outputs(storage: &LocalStorage, claimed: &ClaimedJob) {
    let prefix = storage.attempt_output_prefix(claimed.job_id().get(), claimed.attempt_id());
    if let Err(error) = storage.remove_tree(&prefix).await {
        error!(
            %error,
            job_id = %claimed.job_id(),
            attempt_id = claimed.attempt_id(),
            "attempt output could not be cleaned"
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
