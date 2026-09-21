use std::env;
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serde_json::Value;
use sqlx::PgPool;
use taskharbor_adapters::{LocalStorage, PgJobRepository, WorkerId, WorkerRegistration};
use taskharbor_core::JobName;
use tower::ServiceExt;

mod common;
use common::authenticated_app;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn exposes_worker_liveness_capacity_and_attempt_ownership() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 5)
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

    let worker_id = WorkerId::new();
    repository
        .register_worker(&WorkerRegistration {
            id: worker_id,
            name: "api-worker".into(),
            concurrency_limit: 3,
            lease_duration: Duration::from_secs(30),
            heartbeat_ttl: Duration::from_secs(30),
        })
        .await
        .expect("worker should register");
    let job = repository
        .create(JobName::new("owned API job").expect("test name should be valid"))
        .await
        .expect("job should be created");
    let claimed = repository
        .claim_next(worker_id)
        .await
        .expect("claim should succeed")
        .expect("job should be claimable");

    let temporary = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(temporary.path())
        .await
        .expect("temporary storage should initialize");
    let router = authenticated_app(repository.clone(), storage).await;

    let (status, workers) = send_json(
        router.clone(),
        Request::get("/api/v1/workers")
            .body(Body::empty())
            .expect("worker list request should be valid"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(workers["workers"][0]["id"], worker_id.to_string());
    assert_eq!(workers["workers"][0]["name"], "api-worker");
    assert_eq!(workers["workers"][0]["status"], "online");
    assert_eq!(workers["workers"][0]["concurrency_limit"], 3);
    assert_eq!(workers["workers"][0]["active_attempts"], 1);

    let (status, detail) = send_json(
        router,
        Request::get(format!("/api/v1/jobs/{}", job.job().id()))
            .body(Body::empty())
            .expect("job detail request should be valid"),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(detail["attempts"][0]["worker"]["id"], worker_id.to_string());
    assert_eq!(detail["attempts"][0]["worker"]["name"], "api-worker");
    assert!(detail["attempts"][0]["lease_expires_at"].is_string());

    repository
        .complete(&claimed, Duration::from_millis(1))
        .await
        .expect("test job should complete");
    repository
        .stop_worker(worker_id)
        .await
        .expect("test worker should stop");
}

async fn send_json(router: axum::Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = router
        .oneshot(request)
        .await
        .expect("router should return a response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("response body should be readable");
    let body = serde_json::from_slice(&body).expect("response should contain valid JSON");
    (status, body)
}
