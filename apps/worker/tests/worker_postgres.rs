use std::env;
use std::time::Duration;

use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, Rgba, RgbaImage};
use sqlx::PgPool;
use taskharbor_adapters::{
    AttemptStatus, ImageService, JobRecord, LocalStorage, NewImageJob, NewInputArtifact,
    OWNER_USER_ID, PgJobRepository, RepositoryError, WorkerId, WorkerRegistration,
};
use taskharbor_core::{JobName, JobPriority, JobStatus};
use taskharbor_worker::{process_claimed, process_next, run};
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
    sqlx::query(
        "TRUNCATE schedule_occurrences, schedule_inputs, artifacts, job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE",
    )
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
    sqlx::query(
        "TRUNCATE schedule_occurrences, schedule_inputs, artifacts, job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE",
    )
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
            checksum_sha256: vec![0; 32],
        });
    }
    let job = repository
        .create_image_job(NewImageJob {
            owner_user_id: OWNER_USER_ID,
            name: JobName::new("partial output guard").expect("test name should be valid"),
            max_width: 2,
            jpeg_quality: 85,
            available_at: None,
            priority: JobPriority::Normal,
            inputs,
            idempotency: None,
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

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn transient_failure_retries_and_preserves_attempt_history() {
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
    sqlx::query(
        "TRUNCATE schedule_occurrences, schedule_inputs, artifacts, job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE",
    )
        .execute(&setup_pool)
        .await
        .expect("test tables should be reset");

    let storage_root = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(storage_root.path())
        .await
        .expect("temporary storage should initialize");
    let (job, input_key, source) =
        create_image_fixture(&repository, &storage, "transient retry").await;
    let input_path = storage
        .resolve_key(&input_key)
        .expect("input key should resolve safely");
    tokio::fs::remove_file(&input_path)
        .await
        .expect("input should be removed to inject a transient I/O failure");
    let images = ImageService::new(storage.clone(), 1);

    assert!(
        process_next(&repository, &images, &storage)
            .await
            .expect("transient failure should be recorded")
    );
    let waiting = repository
        .get(job.job().id())
        .await
        .expect("waiting job should be readable")
        .expect("waiting job should exist");
    assert_eq!(waiting.job().status(), JobStatus::RetryWaiting);
    assert_eq!(waiting.progress_completed(), 0);
    let first_attempt = waiting
        .attempts()
        .next()
        .expect("first attempt should be preserved");
    assert_eq!(first_attempt.status(), AttemptStatus::Failed);
    assert_eq!(first_attempt.error_kind(), Some("transient"));

    tokio::fs::write(&input_path, &source)
        .await
        .expect("input should be restored before retry");
    sqlx::query("UPDATE jobs SET available_at = CURRENT_TIMESTAMP WHERE id = $1")
        .bind(i64::try_from(job.job().id().get()).expect("job ID should fit BIGINT"))
        .execute(&setup_pool)
        .await
        .expect("retry should be made eligible deterministically");

    assert!(
        process_next(&repository, &images, &storage)
            .await
            .expect("second attempt should run")
    );
    let succeeded = repository
        .get(job.job().id())
        .await
        .expect("succeeded job should be readable")
        .expect("succeeded job should exist");
    assert_eq!(succeeded.job().status(), JobStatus::Succeeded);
    let attempts = succeeded.attempts().collect::<Vec<_>>();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].status(), AttemptStatus::Failed);
    assert_eq!(attempts[1].status(), AttemptStatus::Succeeded);
    assert_eq!(succeeded.outputs().count(), 1);

    let (limited, limited_key, _) =
        create_image_fixture(&repository, &storage, "attempt limit").await;
    sqlx::query("UPDATE jobs SET max_attempts = 1 WHERE id = $1")
        .bind(i64::try_from(limited.job().id().get()).expect("job ID should fit BIGINT"))
        .execute(&setup_pool)
        .await
        .expect("attempt limit should be configurable in the fixture");
    tokio::fs::remove_file(
        storage
            .resolve_key(&limited_key)
            .expect("limited input key should be safe"),
    )
    .await
    .expect("limited input should be removed");
    assert!(
        process_next(&repository, &images, &storage)
            .await
            .expect("limited transient failure should be recorded")
    );
    let exhausted = repository
        .get(limited.job().id())
        .await
        .expect("exhausted job should be readable")
        .expect("exhausted job should exist");
    assert_eq!(exhausted.job().status(), JobStatus::Failed);
    assert_eq!(exhausted.attempts().count(), 1);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn worker_finishes_requested_cancellation_at_a_safe_boundary() {
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
    sqlx::query(
        "TRUNCATE schedule_occurrences, schedule_inputs, artifacts, job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE",
    )
        .execute(&setup_pool)
        .await
        .expect("test tables should be reset");

    let storage_root = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(storage_root.path())
        .await
        .expect("temporary storage should initialize");
    let (job, _, _) = create_image_fixture(&repository, &storage, "cancel boundary").await;
    let worker_id = register_test_worker(&repository, "cancel-worker").await;
    let claimed = repository
        .claim_next(worker_id)
        .await
        .expect("claim should succeed")
        .expect("job should be claimable");
    let requested = repository
        .request_cancel(job.job().id())
        .await
        .expect("cancel request should succeed")
        .expect("job should exist");
    assert_eq!(requested.job().status(), JobStatus::CancelRequested);

    let images = ImageService::new(storage.clone(), 1);
    process_claimed(&repository, &images, &storage, claimed)
        .await
        .expect("worker should finish cancellation cleanly");

    let cancelled = repository
        .get(job.job().id())
        .await
        .expect("cancelled job should be readable")
        .expect("cancelled job should exist");
    assert_eq!(cancelled.job().status(), JobStatus::Cancelled);
    assert_eq!(cancelled.outputs().count(), 0);
    let attempt = cancelled
        .attempts()
        .next()
        .expect("cancelled attempt should be preserved");
    assert_eq!(attempt.status(), AttemptStatus::Cancelled);
    assert_eq!(attempt.error_kind(), Some("cancelled"));
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn cancel_completion_interleavings_have_one_terminal_winner() {
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
    sqlx::query(
        "TRUNCATE schedule_occurrences, schedule_inputs, artifacts, job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE",
    )
        .execute(&setup_pool)
        .await
        .expect("test tables should be reset");

    let cancel_wins = repository
        .create(JobName::new("cancel wins").expect("test name should be valid"))
        .await
        .expect("job should be created");
    let worker_id = register_test_worker(&repository, "race-worker").await;
    let cancel_claim = repository
        .claim_next(worker_id)
        .await
        .expect("claim should succeed")
        .expect("job should be claimable");
    repository
        .request_cancel(cancel_wins.job().id())
        .await
        .expect("cancel request should succeed")
        .expect("cancelled job should exist");
    assert!(matches!(
        repository
            .complete(&cancel_claim, Duration::from_millis(1))
            .await,
        Err(RepositoryError::StateConflict(_))
    ));
    repository
        .finish_cancelled(&cancel_claim, Duration::from_millis(2))
        .await
        .expect("cancel winner should finalize");

    let completion_wins = repository
        .create(JobName::new("completion wins").expect("test name should be valid"))
        .await
        .expect("job should be created");
    let complete_claim = repository
        .claim_next(worker_id)
        .await
        .expect("claim should succeed")
        .expect("job should be claimable");
    repository
        .complete(&complete_claim, Duration::from_millis(1))
        .await
        .expect("completion winner should finalize");
    assert!(matches!(
        repository.request_cancel(completion_wins.job().id()).await,
        Err(RepositoryError::StateConflict(_))
    ));

    let cancelled = repository
        .get(cancel_wins.job().id())
        .await
        .expect("cancel winner should be readable")
        .expect("cancel winner should exist");
    let succeeded = repository
        .get(completion_wins.job().id())
        .await
        .expect("completion winner should be readable")
        .expect("completion winner should exist");
    assert_eq!(cancelled.job().status(), JobStatus::Cancelled);
    assert_eq!(succeeded.job().status(), JobStatus::Succeeded);
}

async fn create_image_fixture(
    repository: &PgJobRepository,
    storage: &LocalStorage,
    name: &str,
) -> (JobRecord, String, Vec<u8>) {
    let batch = storage
        .begin_upload()
        .await
        .expect("upload batch should initialize");
    let source = test_png();
    let (storage_key, mut file) = batch
        .create_file()
        .await
        .expect("input file should be created");
    file.write_all(&source)
        .await
        .expect("input file should be written");
    file.flush().await.expect("input file should flush");
    drop(file);
    let job = repository
        .create_image_job(NewImageJob {
            owner_user_id: OWNER_USER_ID,
            name: JobName::new(name).expect("test name should be valid"),
            max_width: 2,
            jpeg_quality: 85,
            available_at: None,
            priority: JobPriority::Normal,
            inputs: vec![NewInputArtifact {
                storage_key: storage_key.clone(),
                display_name: "input.png".into(),
                media_type: "image/png".into(),
                byte_size: u64::try_from(source.len()).expect("test PNG size should fit u64"),
                width: 4,
                height: 2,
                checksum_sha256: vec![0; 32],
            }],
            idempotency: None,
        })
        .await
        .expect("image job should be created");
    (job, storage_key, source)
}

async fn register_test_worker(repository: &PgJobRepository, name: &str) -> WorkerId {
    let worker_id = WorkerId::new();
    repository
        .register_worker(&WorkerRegistration {
            id: worker_id,
            name: name.into(),
            concurrency_limit: 1,
            lease_duration: Duration::from_secs(30),
            heartbeat_ttl: Duration::from_secs(30),
        })
        .await
        .expect("test worker should register");
    worker_id
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
