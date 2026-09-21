use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, Rgba, RgbaImage};
use serde_json::{Value, json};
use taskharbor_adapters::{LocalStorage, PgJobRepository};
use tower::ServiceExt;

mod common;
use common::authenticated_app;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn serves_persistent_jobs_and_consistent_errors() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 3)
        .await
        .expect("test database should be reachable");
    repository
        .migrate()
        .await
        .expect("migrations should succeed");
    let storage_root = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(storage_root.path())
        .await
        .expect("temporary storage should initialize");
    let router = authenticated_app(repository, storage.clone()).await;
    let unique_suffix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock should be after the Unix epoch")
        .as_nanos();
    let job_name = format!("api-integration-{unique_suffix}");

    let (health_status, health) = send(
        router.clone(),
        Request::get("/health")
            .body(Body::empty())
            .expect("health request should be valid"),
    )
    .await;
    assert_eq!(health_status, StatusCode::OK);
    assert_eq!(health, json!({ "status": "ok" }));

    let png = test_png();
    let (content_type, body) = multipart_job(&job_name, "picture.bin", &png);
    let create_request = Request::post("/api/v1/jobs")
        .header(CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .expect("create request should be valid");
    let (create_status, created) = send(router.clone(), create_request).await;
    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(created["status"], "queued");
    assert_eq!(created["job_type"], "image_resize");
    assert_eq!(created["progress"], json!({ "completed": 0, "total": 1 }));
    assert_eq!(created["inputs"][0]["media_type"], "image/png");
    let job_id = created["id"]
        .as_u64()
        .expect("created job should contain a numeric ID");

    let (list_status, listed) = send(
        router.clone(),
        Request::get("/api/v1/jobs")
            .body(Body::empty())
            .expect("list request should be valid"),
    )
    .await;
    assert_eq!(list_status, StatusCode::OK);
    assert!(
        listed["jobs"]
            .as_array()
            .expect("list response should contain an array")
            .iter()
            .any(|job| job["id"] == job_id)
    );

    let (detail_status, detail) = send(
        router.clone(),
        Request::get(format!("/api/v1/jobs/{job_id}"))
            .body(Body::empty())
            .expect("detail request should be valid"),
    )
    .await;
    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(detail, created);

    let (early_retry_status, early_retry) = send(
        router.clone(),
        Request::post(format!("/api/v1/jobs/{job_id}/retry"))
            .body(Body::empty())
            .expect("early retry request should be valid"),
    )
    .await;
    assert_eq!(early_retry_status, StatusCode::CONFLICT);
    assert_eq!(early_retry["error"]["code"], "job_state_conflict");

    let (cancel_status, cancelled) = send(
        router.clone(),
        Request::post(format!("/api/v1/jobs/{job_id}/cancel"))
            .body(Body::empty())
            .expect("cancel request should be valid"),
    )
    .await;
    assert_eq!(cancel_status, StatusCode::OK);
    assert_eq!(cancelled["status"], "cancelled");
    assert!(cancelled["cancel_requested_at"].is_string());

    let (retry_status, retry) = send(
        router.clone(),
        Request::post(format!("/api/v1/jobs/{job_id}/retry"))
            .body(Body::empty())
            .expect("manual retry request should be valid"),
    )
    .await;
    assert_eq!(retry_status, StatusCode::CREATED);
    assert_eq!(retry["status"], "queued");
    assert_eq!(retry["retry_of_job_id"], job_id);
    assert_ne!(retry["id"], job_id);
    assert_eq!(
        retry["inputs"][0]["filename"],
        created["inputs"][0]["filename"]
    );
    assert_eq!(
        retry["inputs"][0]["byte_size"],
        created["inputs"][0]["byte_size"]
    );
    assert_ne!(retry["inputs"][0]["id"], created["inputs"][0]["id"]);
    assert_eq!(retry["attempts"], json!([]));

    let (content_type, body) = multipart_job("   ", "picture.png", &png);
    let invalid_request = Request::post("/api/v1/jobs")
        .header(CONTENT_TYPE, content_type)
        .body(Body::from(body))
        .expect("invalid create request should still be valid HTTP");
    let (invalid_status, invalid) = send(router.clone(), invalid_request).await;
    assert_eq!(invalid_status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(invalid["error"]["code"], "validation_error");

    let malformed_request = Request::post("/api/v1/jobs")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":}"#))
        .expect("malformed create request should still be valid HTTP");
    let (malformed_status, malformed) = send(router.clone(), malformed_request).await;
    assert_eq!(malformed_status, StatusCode::BAD_REQUEST);
    assert_eq!(malformed["error"]["code"], "invalid_multipart");

    let (missing_status, missing) = send(
        router,
        Request::get("/api/v1/jobs/not-a-number")
            .body(Body::empty())
            .expect("missing detail request should be valid"),
    )
    .await;
    assert_eq!(missing_status, StatusCode::NOT_FOUND);
    assert_eq!(missing["error"]["code"], "job_not_found");

    let reconnected = PgJobRepository::connect(&database_url, 2)
        .await
        .expect("API should reconnect after a simulated restart");
    let persisted_router = authenticated_app(reconnected, storage).await;
    let (persisted_status, persisted) = send(
        persisted_router,
        Request::get(format!("/api/v1/jobs/{job_id}"))
            .body(Body::empty())
            .expect("persisted detail request should be valid"),
    )
    .await;
    assert_eq!(persisted_status, StatusCode::OK);
    assert_eq!(persisted["id"], job_id);
}

fn test_png() -> Vec<u8> {
    let image = RgbaImage::from_pixel(4, 2, Rgba([30, 120, 220, 255]));
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(image.as_raw(), 4, 2, ColorType::Rgba8.into())
        .expect("test PNG should encode");
    bytes
}

fn multipart_job(name: &str, filename: &str, image: &[u8]) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "taskharbor-test-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"name\"\r\n\r\n{name}\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(
        format!("--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"max_width\"\r\n\r\n2\r\n")
            .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"jpeg_quality\"\r\n\r\n90\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"images\"; filename=\"{filename}\"\r\nContent-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(image);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}

async fn send(app: Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app
        .oneshot(request)
        .await
        .expect("router should return a response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 1_048_576)
        .await
        .expect("response body should be readable");
    let body = serde_json::from_slice(&body).expect("response should contain valid JSON");

    (status, body)
}
