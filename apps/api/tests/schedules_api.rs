use std::env;
use std::time::Duration as StdDuration;

use axum::body::{Body, to_bytes};
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, Rgba, RgbaImage};
use serde_json::{Value, json};
use sqlx::PgPool;
use taskharbor_adapters::{
    ImageService, LocalStorage, PgJobRepository, WorkerId, WorkerRegistration,
};
use taskharbor_api::app;
use taskharbor_worker::process_claimed;
use time::Duration;
use time::format_description::well_known::Rfc3339;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn serves_one_off_and_recurring_schedule_contracts() {
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
        "TRUNCATE schedule_occurrences, schedule_inputs, artifacts, job_attempts, jobs, schedules, workers RESTART IDENTITY CASCADE",
    )
    .execute(&pool)
    .await
    .expect("test tables should reset");
    let base = sqlx::query_scalar::<_, time::OffsetDateTime>("SELECT CURRENT_TIMESTAMP")
        .fetch_one(&pool)
        .await
        .expect("database time should be readable")
        + Duration::minutes(10);
    let base_text = base.format(&Rfc3339).expect("test time should format");
    let worker_id = WorkerId::new();
    repository
        .register_worker_at(
            &WorkerRegistration {
                id: worker_id,
                name: "api-schedule-worker".into(),
                concurrency_limit: 1,
                lease_duration: StdDuration::from_secs(3_600),
                heartbeat_ttl: StdDuration::from_secs(3_600),
            },
            base - Duration::minutes(1),
        )
        .await
        .expect("test worker should register");

    let temporary = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(temporary.path())
        .await
        .expect("temporary storage should initialize");
    let router = app(repository.clone(), storage.clone());
    let source = test_png();

    let (content_type, one_off_body) = multipart_image(
        "future high job",
        &source,
        &[("priority", "high"), ("available_at", base_text.as_str())],
    );
    let (status, one_off) = send_json(
        router.clone(),
        Request::post("/api/v1/jobs")
            .header(CONTENT_TYPE, content_type)
            .body(Body::from(one_off_body))
            .expect("one-off request should be valid"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(one_off["priority"], "high");
    assert_eq!(one_off["available_at"], base_text);
    assert!(
        repository
            .claim_next_at(worker_id, base - Duration::nanoseconds(1))
            .await
            .expect("early eligibility check should succeed")
            .is_none()
    );
    let claimed = repository
        .claim_next_at(worker_id, base)
        .await
        .expect("boundary eligibility check should succeed")
        .expect("one-off job should be eligible at its timestamp");
    repository
        .request_cancel(claimed.job_id())
        .await
        .expect("one-off cancellation should succeed")
        .expect("one-off job should exist");
    repository
        .finish_cancelled(&claimed, StdDuration::from_millis(1))
        .await
        .expect("one-off job should finish cancellation");

    let (content_type, schedule_body) = multipart_image(
        "hourly catalog",
        &source,
        &[
            ("priority", "low"),
            ("anchor_at", base_text.as_str()),
            ("interval_seconds", "3600"),
        ],
    );
    let (status, created) = send_json(
        router.clone(),
        Request::post("/api/v1/schedules")
            .header(CONTENT_TYPE, content_type)
            .body(Body::from(schedule_body))
            .expect("schedule request should be valid"),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(created["enabled"], true);
    assert_eq!(created["priority"], "low");
    assert_eq!(created["inputs"][0]["media_type"], "image/png");
    let schedule_id = created["id"]
        .as_u64()
        .expect("schedule response should include an ID");

    let (status, paused) = send_json(
        router.clone(),
        Request::put(format!("/api/v1/schedules/{schedule_id}"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "name": "hourly catalog edited",
                    "enabled": false,
                    "interval_seconds": 3600,
                    "anchor_at": base_text,
                    "priority": "low",
                    "max_width": 2,
                    "jpeg_quality": 90
                })
                .to_string(),
            ))
            .expect("pause request should be valid"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(paused["enabled"], false);
    assert!(
        repository
            .materialize_next_schedule_at(base)
            .await
            .expect("paused schedule check should succeed")
            .is_none()
    );

    let (status, enabled) = send_json(
        router.clone(),
        Request::put(format!("/api/v1/schedules/{schedule_id}"))
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({
                    "name": "hourly catalog edited",
                    "enabled": true,
                    "interval_seconds": 3600,
                    "anchor_at": base_text,
                    "priority": "low",
                    "max_width": 2,
                    "jpeg_quality": 90
                })
                .to_string(),
            ))
            .expect("enable request should be valid"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(enabled["enabled"], true);

    let tick = repository
        .materialize_next_schedule_at(base)
        .await
        .expect("due schedule should run")
        .expect("due schedule should create an occurrence");
    let job_id = tick.job_id().expect("occurrence should create a job");
    let claimed = repository
        .claim_next_at(worker_id, base)
        .await
        .expect("occurrence claim should succeed")
        .expect("occurrence job should be eligible");
    assert_eq!(claimed.job_id(), job_id);
    let images = ImageService::new(storage.clone(), 1);
    process_claimed(&repository, &images, &storage, claimed)
        .await
        .expect("occurrence image job should complete");

    let (status, detail) = send_json(
        router.clone(),
        Request::get(format!("/api/v1/schedules/{schedule_id}"))
            .body(Body::empty())
            .expect("schedule detail request should be valid"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["occurrences"][0]["outcome"], "created");
    assert_eq!(detail["occurrences"][0]["job_status"], "succeeded");

    let (status, generated) = send_json(
        router,
        Request::get(format!("/api/v1/jobs/{job_id}"))
            .body(Body::empty())
            .expect("generated job request should be valid"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(generated["priority"], "low");
    assert_eq!(generated["schedule_id"], schedule_id);
    assert_eq!(generated["scheduled_for"], base_text);
}

fn test_png() -> Vec<u8> {
    let image = RgbaImage::from_pixel(4, 2, Rgba([20, 120, 220, 255]));
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(image.as_raw(), 4, 2, ColorType::Rgba8.into())
        .expect("test PNG should encode");
    bytes
}

fn multipart_image(name: &str, image: &[u8], extra: &[(&str, &str)]) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "taskharbor-schedule-boundary";
    let mut body = Vec::new();
    append_text(&mut body, BOUNDARY, "name", name);
    append_text(&mut body, BOUNDARY, "max_width", "2");
    append_text(&mut body, BOUNDARY, "jpeg_quality", "90");
    for (field, value) in extra {
        append_text(&mut body, BOUNDARY, field, value);
    }
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"images\"; filename=\"source.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(image);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

fn append_text(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

async fn send_json(router: axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router
        .oneshot(request)
        .await
        .expect("router should return a response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 30 * 1024 * 1024)
        .await
        .expect("response body should be readable");
    let body = serde_json::from_slice(&body).expect("response should be valid JSON");
    (status, body)
}
