use std::collections::HashSet;
use std::env;
use std::time::{Duration, SystemTime};

use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, Rgba, RgbaImage};
use sqlx::PgPool;
use taskharbor_adapters::{
    ImageService, LocalStorage, NewImageJob, NewInputArtifact, NewSchedule, OWNER_USER_ID,
    PendingOutputArtifact, PgJobRepository, WorkerId, WorkerRegistration,
};
use taskharbor_core::{JobName, JobPriority};
use time::OffsetDateTime;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn expires_old_outputs_without_removing_references_or_active_attempt_files() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 6)
        .await
        .expect("test database should be reachable");
    repository.migrate().await.expect("migrations should run");
    let pool = PgPool::connect(&database_url)
        .await
        .expect("test setup should connect");
    sqlx::query(
        r#"
        TRUNCATE sessions, schedule_occurrences, schedule_inputs, artifacts,
            job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE
        "#,
    )
    .execute(&pool)
    .await
    .expect("test tables should reset");

    let temporary = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(temporary.path())
        .await
        .expect("storage should initialize");
    let source = test_png();
    write_key(&storage, "inputs/finished.png", &source).await;
    let finished = repository
        .create_image_job(image_job("finished job", "inputs/finished.png", &source))
        .await
        .expect("finished job should be created");
    let worker_id = WorkerId::new();
    repository
        .register_worker(&WorkerRegistration {
            id: worker_id,
            name: "maintenance-worker".into(),
            concurrency_limit: 1,
            lease_duration: Duration::from_secs(3_600),
            heartbeat_ttl: Duration::from_secs(3_600),
        })
        .await
        .expect("worker should register");
    let claimed = repository
        .claim_next(worker_id)
        .await
        .expect("claim should succeed")
        .expect("finished job should be claimable");
    let input = finished.inputs().next().expect("job should retain input");
    let output_key = storage.output_key(claimed.job_id().get(), claimed.attempt_id(), input.id());
    let processed = ImageService::new(storage.clone(), 1)
        .resize_to_jpeg(input.storage_key(), output_key.clone(), 2, 85)
        .await
        .expect("test output should process");
    repository
        .complete_image_job(
            &claimed,
            Duration::from_millis(1),
            vec![PendingOutputArtifact {
                source_artifact_id: input.id(),
                item_index: 0,
                storage_key: processed.storage_key,
                display_name: "finished.jpg".into(),
                media_type: "image/jpeg".into(),
                byte_size: processed.byte_size,
                width: processed.width,
                height: processed.height,
            }],
        )
        .await
        .expect("job should complete");
    sqlx::query(
        "UPDATE jobs SET finished_at = CURRENT_TIMESTAMP - INTERVAL '2 hours' WHERE id = $1",
    )
    .bind(i64::try_from(finished.job().id().get()).expect("job ID should fit BIGINT"))
    .execute(&pool)
    .await
    .expect("finish time should age");
    let expired = repository
        .expire_output_artifacts(Duration::from_secs(60 * 60))
        .await
        .expect("old output should expire");
    assert_eq!(expired, vec![output_key.clone()]);

    write_key(&storage, "inputs/active.png", &source).await;
    repository
        .create_image_job(image_job("active job", "inputs/active.png", &source))
        .await
        .expect("active job should be created");
    let active = repository
        .claim_next(worker_id)
        .await
        .expect("active claim should succeed")
        .expect("active job should be claimable");
    let active_prefix = storage.attempt_output_prefix(active.job_id().get(), active.attempt_id());
    write_key(
        &storage,
        &format!("{active_prefix}/partial.jpg"),
        b"partial",
    )
    .await;

    write_key(&storage, "inputs/schedule.png", &source).await;
    repository
        .create_schedule(NewSchedule {
            owner_user_id: OWNER_USER_ID,
            name: JobName::new("protected schedule").expect("schedule name should be valid"),
            interval_seconds: 60,
            anchor_at: OffsetDateTime::now_utc() + time::Duration::hours(1),
            priority: JobPriority::Normal,
            max_width: 2,
            jpeg_quality: 85,
            inputs: vec![input_artifact("inputs/schedule.png", &source)],
        })
        .await
        .expect("schedule should be created");
    write_key(&storage, "inputs/orphan.png", b"orphan").await;

    let references = repository
        .storage_references()
        .await
        .expect("references should load");
    assert!(references.contains("inputs/active.png"));
    assert!(references.contains("inputs/schedule.png"));
    let prefixes = repository
        .active_attempt_output_prefixes()
        .await
        .expect("active prefixes should load");
    assert!(prefixes.contains(&active_prefix));
    let cleaned = storage
        .cleanup_unreferenced(
            references,
            prefixes,
            SystemTime::now() + Duration::from_secs(1),
        )
        .await
        .expect("cleanup should succeed");
    assert!(cleaned.files_removed >= 2);
    assert!(!path_exists(&storage, "inputs/orphan.png"));
    assert!(!path_exists(&storage, &output_key));
    assert!(path_exists(&storage, "inputs/active.png"));
    assert!(path_exists(&storage, "inputs/schedule.png"));
    assert!(path_exists(
        &storage,
        &format!("{active_prefix}/partial.jpg")
    ));

    let remaining: HashSet<_> = repository
        .storage_references()
        .await
        .expect("remaining references should load");
    assert!(!remaining.contains(&output_key));
}

fn image_job(name: &str, storage_key: &str, source: &[u8]) -> NewImageJob {
    NewImageJob {
        owner_user_id: OWNER_USER_ID,
        name: JobName::new(name).expect("job name should be valid"),
        max_width: 2,
        jpeg_quality: 85,
        available_at: None,
        priority: JobPriority::Normal,
        inputs: vec![input_artifact(storage_key, source)],
        idempotency: None,
    }
}

fn input_artifact(storage_key: &str, source: &[u8]) -> NewInputArtifact {
    NewInputArtifact {
        storage_key: storage_key.into(),
        display_name: "source.png".into(),
        media_type: "image/png".into(),
        byte_size: u64::try_from(source.len()).expect("source size should fit u64"),
        width: 4,
        height: 2,
        checksum_sha256: vec![0; 32],
    }
}

async fn write_key(storage: &LocalStorage, key: &str, contents: &[u8]) {
    let path = storage.resolve_key(key).expect("test key should be safe");
    tokio::fs::create_dir_all(path.parent().expect("test file should have a parent"))
        .await
        .expect("test directory should be created");
    tokio::fs::write(path, contents)
        .await
        .expect("test file should be written");
}

fn path_exists(storage: &LocalStorage, key: &str) -> bool {
    storage
        .resolve_key(key)
        .expect("test key should be safe")
        .exists()
}

fn test_png() -> Vec<u8> {
    let image = RgbaImage::from_pixel(4, 2, Rgba([20, 120, 220, 255]));
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(image.as_raw(), 4, 2, ColorType::Rgba8.into())
        .expect("test PNG should encode");
    bytes
}
