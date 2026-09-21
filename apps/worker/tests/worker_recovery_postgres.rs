use std::collections::HashSet;
use std::env;
use std::time::Duration;

use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, Rgba, RgbaImage};
use sqlx::PgPool;
use taskharbor_adapters::{
    AttemptStatus, ImageService, LocalStorage, NewImageJob, NewInputArtifact, OWNER_USER_ID,
    PendingOutputArtifact, PgJobRepository, RepositoryError, WorkerId, WorkerRegistration,
    WorkerStatus,
};
use taskharbor_core::{JobName, JobPriority, JobStatus};
use taskharbor_worker::{WorkerConfig, process_claimed, run_with_config};
use time::Duration as TimeDuration;
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn recovers_a_crashed_image_attempt_and_rejects_the_old_owner() {
    let (repository, pool) = setup_repository().await;
    let temporary = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(temporary.path())
        .await
        .expect("temporary storage should initialize");
    let now = sqlx::query_scalar::<_, time::OffsetDateTime>("SELECT CURRENT_TIMESTAMP")
        .fetch_one(&pool)
        .await
        .expect("database time should be readable");
    let crashed_at = now - TimeDuration::seconds(10);
    let old_worker = WorkerId::new();
    repository
        .register_worker_at(
            &WorkerRegistration {
                id: old_worker,
                name: "crashed-worker".into(),
                concurrency_limit: 1,
                lease_duration: Duration::from_secs(2),
                heartbeat_ttl: Duration::from_secs(30),
            },
            crashed_at,
        )
        .await
        .expect("old worker should register");

    let (job, input_id) = create_image_job(&repository, &storage, crashed_at).await;
    let stale_claim = repository
        .claim_next_at(old_worker, crashed_at)
        .await
        .expect("old worker claim should succeed")
        .expect("image job should be claimable");
    let stale_output_key =
        storage.output_key(job.job().id().get(), stale_claim.attempt_id(), input_id);
    write_fake_output(&storage, &stale_output_key).await;

    let recovery_config = fast_config("recovery-worker", 1);
    let recovery_worker_id = recovery_config.id;
    let images = ImageService::new(storage.clone(), 1);
    let (shutdown_sender, shutdown_receiver) = watch::channel(false);
    let worker = tokio::spawn(run_with_config(
        repository.clone(),
        images.clone(),
        storage.clone(),
        shutdown_receiver,
        recovery_config,
    ));

    timeout(Duration::from_secs(8), async {
        loop {
            let current = repository
                .get(job.job().id())
                .await
                .expect("recovering job should be readable")
                .expect("recovering job should exist");
            if current.job().status() == JobStatus::Succeeded {
                break;
            }
            sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("another worker should recover the expired attempt");

    assert!(
        !tokio::fs::try_exists(
            storage
                .resolve_key(&stale_output_key)
                .expect("stale output key should be safe")
        )
        .await
        .expect("stale output existence should be readable")
    );
    let recovered = repository
        .get(job.job().id())
        .await
        .expect("recovered job should be readable")
        .expect("recovered job should exist");
    let attempts = recovered.attempts().collect::<Vec<_>>();
    assert_eq!(attempts.len(), 2);
    assert_eq!(attempts[0].status(), AttemptStatus::Failed);
    assert_eq!(attempts[0].worker_id(), Some(old_worker));
    assert_eq!(attempts[1].status(), AttemptStatus::Succeeded);
    assert_eq!(attempts[1].worker_id(), Some(recovery_worker_id));
    let output = recovered
        .outputs()
        .next()
        .expect("recovered job should publish one output");
    assert_eq!(output.attempt_id(), Some(attempts[1].id()));
    assert_ne!(output.storage_key(), stale_output_key);

    write_fake_output(&storage, &stale_output_key).await;
    let stale_publish = repository
        .complete_image_job(
            &stale_claim,
            Duration::from_millis(20),
            vec![PendingOutputArtifact {
                source_artifact_id: input_id,
                item_index: 0,
                storage_key: stale_output_key.clone(),
                display_name: "stale.jpg".into(),
                media_type: "image/jpeg".into(),
                byte_size: 4,
                width: 1,
                height: 1,
            }],
        )
        .await;
    assert!(matches!(stale_publish, Err(RepositoryError::ClaimLost)));
    process_claimed(&repository, &images, &storage, stale_claim)
        .await
        .expect("stale worker should stop without publishing");
    assert!(
        !tokio::fs::try_exists(
            storage
                .resolve_key(&stale_output_key)
                .expect("stale output key should remain safe")
        )
        .await
        .expect("stale output cleanup should be observable")
    );
    assert!(storage.open(output.storage_key()).await.is_ok());

    shutdown_sender
        .send(true)
        .expect("recovery worker should receive shutdown");
    timeout(Duration::from_secs(5), worker)
        .await
        .expect("recovery worker should stop")
        .expect("recovery worker task should not panic")
        .expect("recovery worker should stop cleanly");
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn two_workers_respect_local_concurrency_and_share_the_queue() {
    let (repository, _) = setup_repository().await;
    let temporary = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(temporary.path())
        .await
        .expect("temporary storage should initialize");
    let images = ImageService::new(storage.clone(), 2);
    let first_config = fast_config("worker-alpha", 2);
    let second_config = fast_config("worker-beta", 2);
    let first_id = first_config.id;
    let second_id = second_config.id;
    let (first_shutdown_sender, first_shutdown_receiver) = watch::channel(false);
    let (second_shutdown_sender, second_shutdown_receiver) = watch::channel(false);
    let first_worker = tokio::spawn(run_with_config(
        repository.clone(),
        images.clone(),
        storage.clone(),
        first_shutdown_receiver,
        first_config,
    ));
    let second_worker = tokio::spawn(run_with_config(
        repository.clone(),
        images,
        storage,
        second_shutdown_receiver,
        second_config,
    ));

    timeout(Duration::from_secs(5), async {
        loop {
            if repository
                .list_workers()
                .await
                .expect("workers should be readable")
                .iter()
                .filter(|worker| worker.status() == WorkerStatus::Online)
                .count()
                == 2
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("both workers should register");

    let mut job_ids = Vec::new();
    for index in 0..8 {
        let job = repository
            .create(JobName::new(format!("parallel job {index}")).expect("name should be valid"))
            .await
            .expect("parallel job should be created");
        job_ids.push(job.job().id());
    }

    let mut saw_both_active = false;
    timeout(Duration::from_secs(8), async {
        loop {
            let workers = repository
                .list_workers()
                .await
                .expect("worker capacity should be readable");
            for worker in &workers {
                assert!(
                    usize::try_from(worker.active_attempts()).expect("count should fit usize")
                        <= usize::from(worker.concurrency_limit())
                );
            }
            saw_both_active |= workers
                .iter()
                .filter(|worker| worker.active_attempts() > 0)
                .count()
                == 2;

            let jobs = repository.list().await.expect("jobs should be readable");
            if jobs
                .iter()
                .filter(|job| job_ids.contains(&job.job().id()))
                .all(|job| job.job().status() == JobStatus::Succeeded)
            {
                break;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("both workers should finish the shared queue");
    assert!(saw_both_active);

    let owner_ids = repository
        .list()
        .await
        .expect("completed jobs should be readable")
        .into_iter()
        .filter(|job| job_ids.contains(&job.job().id()))
        .flat_map(|job| {
            job.attempts()
                .filter_map(|attempt| attempt.worker_id())
                .collect::<Vec<_>>()
        })
        .collect::<HashSet<_>>();
    assert_eq!(owner_ids, HashSet::from([first_id, second_id]));

    first_shutdown_sender
        .send(true)
        .expect("first worker should receive shutdown");
    second_shutdown_sender
        .send(true)
        .expect("second worker should receive shutdown");
    for worker in [first_worker, second_worker] {
        timeout(Duration::from_secs(5), worker)
            .await
            .expect("worker should stop within its grace period")
            .expect("worker task should not panic")
            .expect("worker should stop cleanly");
    }
    let stopped = repository
        .list_workers()
        .await
        .expect("stopped workers should remain visible");
    assert_eq!(
        stopped
            .iter()
            .filter(|worker| worker.status() == WorkerStatus::Stopped)
            .count(),
        2
    );
}

async fn setup_repository() -> (PgJobRepository, PgPool) {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 12)
        .await
        .expect("test database should be reachable");
    repository.migrate().await.expect("migrations should run");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("test setup should connect");
    sqlx::query(
        "TRUNCATE schedule_occurrences, schedule_inputs, artifacts, job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("test tables should reset");
    (repository, pool)
}

fn fast_config(name: &str, concurrency_limit: usize) -> WorkerConfig {
    let mut config = WorkerConfig::new(name, concurrency_limit);
    config.lease_duration = Duration::from_secs(2);
    config.lease_renewal_interval = Duration::from_secs(1);
    config.heartbeat_interval = Duration::from_secs(1);
    config.heartbeat_ttl = Duration::from_secs(4);
    config.shutdown_grace = Duration::from_secs(3);
    config
}

async fn create_image_job(
    repository: &PgJobRepository,
    storage: &LocalStorage,
    available_at: time::OffsetDateTime,
) -> (taskharbor_adapters::JobRecord, u64) {
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
            name: JobName::new("recover image output").expect("test name should be valid"),
            max_width: 2,
            jpeg_quality: 85,
            available_at: Some(available_at),
            priority: JobPriority::Normal,
            inputs: vec![NewInputArtifact {
                storage_key,
                display_name: "input.png".into(),
                media_type: "image/png".into(),
                byte_size: u64::try_from(source.len()).expect("source size should fit u64"),
                width: 4,
                height: 2,
                checksum_sha256: vec![0; 32],
            }],
            idempotency: None,
        })
        .await
        .expect("image job should be created");
    let input_id = job
        .inputs()
        .next()
        .expect("image job should retain its input")
        .id();
    (job, input_id)
}

async fn write_fake_output(storage: &LocalStorage, storage_key: &str) {
    let path = storage
        .resolve_key(storage_key)
        .expect("fake output key should be safe");
    tokio::fs::create_dir_all(path.parent().expect("output path should have a parent"))
        .await
        .expect("fake output directory should be created");
    tokio::fs::write(path, [0xff, 0xd8, 0xff, 0xd9])
        .await
        .expect("fake output should be written");
}

fn test_png() -> Vec<u8> {
    let image = RgbaImage::from_pixel(4, 2, Rgba([40, 140, 220, 255]));
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(image.as_raw(), 4, 2, ColorType::Rgba8.into())
        .expect("test PNG should encode");
    bytes
}
