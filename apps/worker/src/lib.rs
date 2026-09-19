use std::time::{Duration, Instant};

use taskharbor_adapters::{
    ClaimedJob, ClaimedWork, ImageService, LocalStorage, PendingOutputArtifact, PgJobRepository,
    RepositoryError,
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

    let started_at = Instant::now();
    match claimed.work().clone() {
        ClaimedWork::DemoDelay { delay } => {
            sleep(delay).await;
            repository.complete(&claimed, started_at.elapsed()).await?;
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

    println!("Worker completed job {}", claimed.job_id());
    Ok(true)
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
                let prefix =
                    storage.attempt_output_prefix(claimed.job_id().get(), claimed.attempt_id());
                if let Err(cleanup_error) = storage.remove_tree(&prefix).await {
                    eprintln!(
                        "Worker could not clean failed output for job {}: {cleanup_error}",
                        claimed.job_id()
                    );
                }
                repository
                    .fail(claimed, started_at.elapsed(), error.safe_message())
                    .await?;
                return Ok(());
            }
        };

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
        repository
            .update_progress(
                claimed,
                u32::try_from(completed + 1).map_err(|_| {
                    RepositoryError::InvalidData("image progress exceeds u32".into())
                })?,
            )
            .await?;
    }

    repository
        .complete_image_job(claimed, started_at.elapsed(), outputs)
        .await
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
