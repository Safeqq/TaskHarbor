use std::time::{Duration, Instant};

use taskharbor_adapters::{
    ClaimedJob, ClaimedWork, FailureDisposition, ImageService, LocalStorage, PendingOutputArtifact,
    PgJobRepository, RepositoryError,
};
use tokio::sync::watch;
use tokio::time::sleep;

const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub async fn run(
    repository: PgJobRepository,
    images: ImageService,
    storage: LocalStorage,
    mut shutdown: watch::Receiver<bool>,
) -> Result<(), RepositoryError> {
    loop {
        let shutdown_requested = *shutdown.borrow();
        if shutdown_requested {
            println!("TaskHarbor worker stopped gracefully");
            return Ok(());
        }

        if process_next(&repository, &images, &storage).await? {
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

pub async fn process_next(
    repository: &PgJobRepository,
    images: &ImageService,
    storage: &LocalStorage,
) -> Result<bool, RepositoryError> {
    let Some(claimed) = repository.claim_next().await? else {
        return Ok(false);
    };

    println!(
        "Worker claimed job {} with attempt ID {}",
        claimed.job_id(),
        claimed.attempt_id()
    );

    process_claimed(repository, images, storage, claimed).await?;
    Ok(true)
}

pub async fn process_claimed(
    repository: &PgJobRepository,
    images: &ImageService,
    storage: &LocalStorage,
    claimed: ClaimedJob,
) -> Result<(), RepositoryError> {
    let started_at = Instant::now();
    match claimed.work().clone() {
        ClaimedWork::DemoDelay { delay } => {
            sleep(delay).await;
            if finish_if_cancel_requested(repository, storage, &claimed, started_at).await? {
                return Ok(());
            }

            match repository.complete(&claimed, started_at.elapsed()).await {
                Ok(()) => {}
                Err(error @ RepositoryError::StateConflict(_)) => {
                    if !finish_if_cancel_requested(repository, storage, &claimed, started_at)
                        .await?
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
                &claimed,
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
