use std::env;
use std::time::Duration;

use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, Rgba, RgbaImage};
use sqlx::PgPool;
use taskharbor_adapters::{
    ImageService, LocalStorage, NewImageJob, NewInputArtifact, PgJobRepository,
};
use taskharbor_core::{JobName, JobStatus};
use taskharbor_worker::{process_next, run};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn finishes_the_active_job_before_graceful_shutdown() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 3)
        .await
        .expect("test database should be reachable");
    repository
        .migrate()
        .await
        .expect("migrations should succeed");
    let setup_pool = PgPool::connect(&database_url)
        .await
        .expect("test setup should connect");
    sqlx::query("TRUNCATE job_attempts, jobs RESTART IDENTITY CASCADE")
        .execute(&setup_pool)
        .await
        .expect("test tables should be reset");

    let job = repository
        .create(JobName::new("graceful shutdown").expect("test name should be valid"))
        .await
        .expect("test job should be created");
    let job_id = job.job().id();
    let storage_root = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(storage_root.path())
        .await
        .expect("temporary storage should initialize");
    let images = ImageService::new(storage.clone(), 1);
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let worker = tokio::spawn(run(repository.clone(), images, storage, shutdown_receiver));

    timeout(Duration::from_secs(5), async {
        loop {
            let current = repository
                .get(job_id)
                .await
                .expect("job state should be readable")
                .expect("test job should exist");
            if current.job().status() == JobStatus::Running {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("worker should claim the job");

    shutdown_sender
        .send(true)
        .expect("worker should still receive shutdown");
    timeout(Duration::from_secs(5), worker)
        .await
        .expect("worker should stop after the active job")
        .expect("worker task should not panic")
        .expect("worker should stop without a repository error");

    let completed = repository
        .get(job_id)
        .await
        .expect("completed state should be readable")
        .expect("completed job should exist");
    assert_eq!(completed.job().status(), JobStatus::Succeeded);
    assert_eq!(completed.progress_completed(), completed.progress_total());

    let attempt_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM job_attempts WHERE job_id = $1 AND state = 'succeeded'",
    )
    .bind(i64::try_from(job_id.get()).expect("test ID should fit BIGINT"))
    .fetch_one(&setup_pool)
    .await
    .expect("attempt count should be readable");
    assert_eq!(attempt_count, 1);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn fails_the_whole_job_without_publishing_partial_outputs() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 3)
        .await
        .expect("test database should be reachable");
    repository
        .migrate()
        .await
        .expect("migrations should succeed");
    let setup_pool = PgPool::connect(&database_url)
        .await
        .expect("test setup should connect");
    sqlx::query("TRUNCATE artifacts, job_attempts, jobs RESTART IDENTITY CASCADE")
        .execute(&setup_pool)
        .await
        .expect("test tables should be reset");

    let storage_root = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(storage_root.path())
        .await
        .expect("temporary storage should initialize");
    let batch = storage
        .begin_upload()
        .await
        .expect("upload batch should initialize");
    let source = test_png();
    let mut inputs = Vec::new();
    let mut keys = Vec::new();
    for index in 0..2 {
        let (storage_key, mut file) = batch
            .create_file()
            .await
            .expect("input file should be created");
        file.write_all(&source)
            .await
            .expect("input file should be written");
        file.flush().await.expect("input file should flush");
        keys.push(storage_key.clone());
        inputs.push(NewInputArtifact {
            storage_key,
            display_name: format!("input-{index}.png"),
            media_type: "image/png".into(),
            byte_size: u64::try_from(source.len()).expect("test PNG size should fit u64"),
            width: 4,
            height: 2,
        });
    }
    let job = repository
        .create_image_job(NewImageJob {
            name: JobName::new("partial output guard").expect("test name should be valid"),
            max_width: 2,
            jpeg_quality: 85,
            inputs,
        })
        .await
        .expect("image job should be created");
    tokio::fs::write(
        storage
            .resolve_key(&keys[1])
            .expect("second input key should be safe"),
        b"corrupted after upload validation",
    )
    .await
    .expect("second input should be corrupted for the test");

    let images = ImageService::new(storage.clone(), 1);
    assert!(
        process_next(&repository, &images, &storage)
            .await
            .expect("processing failure should be recorded, not crash the worker")
    );

    let failed = repository
        .get(job.job().id())
        .await
        .expect("failed job should be readable")
        .expect("failed job should still exist");
    assert_eq!(failed.job().status(), JobStatus::Failed);
    assert_eq!(failed.progress_completed(), 1);
    assert_eq!(failed.progress_total(), 2);
    assert!(failed.failure_message().is_some());
    assert_eq!(failed.outputs().count(), 0);

    let output_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM artifacts WHERE job_id = $1 AND kind = 'output'",
    )
    .bind(i64::try_from(job.job().id().get()).expect("job ID should fit BIGINT"))
    .fetch_one(&setup_pool)
    .await
    .expect("output count should be readable");
    assert_eq!(output_count, 0);
    let attempt = sqlx::query_as::<_, AttemptFailure>(
        "SELECT state, progress_completed, progress_total FROM job_attempts WHERE job_id = $1",
    )
    .bind(i64::try_from(job.job().id().get()).expect("job ID should fit BIGINT"))
    .fetch_one(&setup_pool)
    .await
    .expect("failed attempt should be readable");
    assert_eq!(attempt.state, "failed");
    assert_eq!(attempt.progress_completed, 1);
    assert_eq!(attempt.progress_total, 2);
}

#[derive(Debug, sqlx::FromRow)]
struct AttemptFailure {
    state: String,
    progress_completed: i32,
    progress_total: i32,
}

fn test_png() -> Vec<u8> {
    let image = RgbaImage::from_pixel(4, 2, Rgba([40, 140, 220, 255]));
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(image.as_raw(), 4, 2, ColorType::Rgba8.into())
        .expect("test PNG should encode");
    bytes
}
