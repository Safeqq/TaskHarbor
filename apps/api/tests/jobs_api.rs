use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::CONTENT_TYPE;
use axum::http::{Request, StatusCode};
use serde_json::{Value, json};
use taskharbor_api::app;
use tower::ServiceExt;

#[tokio::test]
async fn reports_that_the_service_is_healthy() {
    let (status, body) = send(
        app(),
        Request::get("/health")
            .body(Body::empty())
            .expect("health request should be valid"),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, json!({ "status": "ok" }));
}

#[tokio::test]
async fn creates_lists_and_reads_a_queued_job() {
    let app = app();
    let create_request = Request::post("/api/v1/jobs")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"resize avatars"}"#))
        .expect("create request should be valid");

    let (create_status, created) = send(app.clone(), create_request).await;

    assert_eq!(create_status, StatusCode::CREATED);
    assert_eq!(
        created,
        json!({ "id": 1, "name": "resize avatars", "status": "queued" })
    );

    let (list_status, listed) = send(
        app.clone(),
        Request::get("/api/v1/jobs")
            .body(Body::empty())
            .expect("list request should be valid"),
    )
    .await;

    assert_eq!(list_status, StatusCode::OK);
    assert_eq!(listed, json!({ "jobs": [created.clone()] }));

    let (detail_status, detail) = send(
        app,
        Request::get("/api/v1/jobs/1")
            .body(Body::empty())
            .expect("detail request should be valid"),
    )
    .await;

    assert_eq!(detail_status, StatusCode::OK);
    assert_eq!(detail, created);
}

#[tokio::test]
async fn returns_a_json_error_for_an_unknown_job() {
    let (status, body) = send(
        app(),
        Request::get("/api/v1/jobs/not-a-number")
            .body(Body::empty())
            .expect("detail request should be valid"),
    )
    .await;

    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        body,
        json!({
            "error": {
                "code": "job_not_found",
                "message": "job was not found"
            }
        })
    );
}

#[tokio::test]
async fn rejects_a_blank_job_name() {
    let request = Request::post("/api/v1/jobs")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":"   "}"#))
        .expect("create request should be valid");

    let (status, body) = send(app(), request).await;

    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(body["error"]["code"], "validation_error");
    assert_eq!(body["error"]["field"], "name");
}

#[tokio::test]
async fn rejects_malformed_json_with_the_standard_error_shape() {
    let request = Request::post("/api/v1/jobs")
        .header(CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"name":}"#))
        .expect("create request should be valid");

    let (status, body) = send(app(), request).await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "invalid_json");
    assert!(body["error"]["message"].is_string());
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
