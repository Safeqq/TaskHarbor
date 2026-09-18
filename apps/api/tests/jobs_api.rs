use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use taskharbor_adapters::PgJobRepository;
use taskharbor_api::app;
use tower::ServiceExt;

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
    let router = app(repository);
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

    let create_request = Request::post("/api/v1/jobs")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(json!({ "name": job_name }).to_string()))
        .expect("create request should be valid");
    let (create_status, created) = send(router.clone(), create_request).await;
    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(created["status"], "queued");
    assert_eq!(created["job_type"], "demo_delay");
    assert_eq!(created["progress"], json!({ "completed": 0, "total": 1 }));
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

    let invalid_request = Request::post("/api/v1/jobs")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"   "}"#))
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
    assert_eq!(malformed["error"]["code"], "invalid_json");

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
    let persisted_router = app(reconnected);
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
