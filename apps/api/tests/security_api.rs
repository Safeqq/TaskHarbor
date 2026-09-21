use std::env;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE, COOKIE, SET_COOKIE};
use axum::http::{Request, StatusCode};
use image::codecs::png::PngEncoder;
use image::{ColorType, ImageEncoder, Rgba, RgbaImage};
use serde_json::{Value, json};
use sqlx::PgPool;
use taskharbor_adapters::{LocalStorage, PgJobRepository};
use taskharbor_api::{AppConfig, app_with_config, initialize_owner};
use tower::ServiceExt;

const USERNAME: &str = "owner";
const PASSWORD: &str = "test-owner-password";

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn enforces_session_csrf_ownership_and_idempotency() {
    let (repository, pool, storage) = setup().await;
    initialize_owner(&repository, USERNAME, PASSWORD)
        .await
        .expect("owner should initialize");
    let config = AppConfig {
        secure_cookies: true,
        max_active_jobs: 1,
        ..AppConfig::default()
    };
    let router = app_with_config(repository.clone(), storage.clone(), config);

    for path in ["/health/live", "/health/ready"] {
        let response = send(
            &router,
            Request::get(path)
                .body(Body::empty())
                .expect("health request should be valid"),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
    }

    let response = send(
        &router,
        Request::get("/api/v1/jobs")
            .body(Body::empty())
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    let response = send(
        &router,
        Request::get("/api/v1/artifacts/1/download")
            .body(Body::empty())
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let response = login_response(&router, USERNAME, "incorrect-password").await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

    let owner = login(&router, USERNAME, PASSWORD).await;
    let session_cookie = owner
        .set_cookies
        .iter()
        .find(|cookie| cookie.starts_with("taskharbor_session="))
        .expect("session cookie should be set");
    assert!(session_cookie.contains("HttpOnly"));
    assert!(session_cookie.contains("SameSite=Strict"));
    assert!(session_cookie.contains("Secure"));
    let csrf_cookie = owner
        .set_cookies
        .iter()
        .find(|cookie| cookie.starts_with("taskharbor_csrf="))
        .expect("CSRF cookie should be set");
    assert!(!csrf_cookie.contains("HttpOnly"));
    assert!(csrf_cookie.contains("SameSite=Strict"));

    let response = send(
        &router,
        Request::post("/api/v1/jobs/1/cancel")
            .header(COOKIE, &owner.cookie)
            .body(Body::empty())
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);

    let (content_type, body) = multipart_image("secure upload");
    let response = send(
        &router,
        Request::post("/api/v1/jobs")
            .header(COOKIE, &owner.cookie)
            .header("x-csrf-token", &owner.csrf)
            .header("idempotency-key", "security-test-job")
            .header(CONTENT_TYPE, &content_type)
            .body(Body::from(body))
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(response.status(), StatusCode::CREATED);
    let created = json_body(response).await;
    let job_id = created["id"].as_u64().expect("job should have an ID");

    let (content_type, body) = multipart_image("secure upload");
    let replay = send(
        &router,
        Request::post("/api/v1/jobs")
            .header(COOKIE, &owner.cookie)
            .header("x-csrf-token", &owner.csrf)
            .header("idempotency-key", "security-test-job")
            .header(CONTENT_TYPE, &content_type)
            .body(Body::from(body))
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::CREATED);
    assert_eq!(json_body(replay).await["id"], job_id);
    let job_count = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM jobs")
        .fetch_one(&pool)
        .await
        .expect("job count should be readable");
    assert_eq!(job_count, 1);

    let (content_type, body) = multipart_image("different upload");
    let conflict = send(
        &router,
        Request::post("/api/v1/jobs")
            .header(COOKIE, &owner.cookie)
            .header("x-csrf-token", &owner.csrf)
            .header("idempotency-key", "security-test-job")
            .header(CONTENT_TYPE, &content_type)
            .body(Body::from(body))
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);

    let (content_type, body) = multipart_image("second active job");
    let limited = send(
        &router,
        Request::post("/api/v1/jobs")
            .header(COOKIE, &owner.cookie)
            .header("x-csrf-token", &owner.csrf)
            .header(CONTENT_TYPE, &content_type)
            .body(Body::from(body))
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(limited.status(), StatusCode::INSUFFICIENT_STORAGE);
    let referenced_bytes =
        sqlx::query_scalar::<_, i64>("SELECT COALESCE(SUM(byte_size), 0)::BIGINT FROM artifacts")
            .fetch_one(&pool)
            .await
            .expect("artifact size should be readable");
    assert_eq!(
        storage
            .usage_bytes()
            .await
            .expect("storage usage should be readable"),
        u64::try_from(referenced_bytes).expect("artifact size should be non-negative")
    );

    sqlx::query("DELETE FROM users WHERE username = 'guest'")
        .execute(&pool)
        .await
        .expect("old guest should be removed");
    sqlx::query(
        r#"
        INSERT INTO users (username, password_hash)
        SELECT 'guest', password_hash FROM users WHERE id = 1
        "#,
    )
    .execute(&pool)
    .await
    .expect("guest should be inserted");
    let guest = login(&router, "guest", PASSWORD).await;
    let hidden = send(
        &router,
        Request::get(format!("/api/v1/jobs/{job_id}"))
            .header(COOKIE, &guest.cookie)
            .body(Body::empty())
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(hidden.status(), StatusCode::NOT_FOUND);
    sqlx::query(
        r#"
        UPDATE sessions
        SET created_at = CURRENT_TIMESTAMP - INTERVAL '2 minutes',
            last_seen_at = CURRENT_TIMESTAMP - INTERVAL '2 minutes',
            expires_at = CURRENT_TIMESTAMP - INTERVAL '1 minute'
        WHERE user_id = (SELECT id FROM users WHERE username = 'guest')
        "#,
    )
    .execute(&pool)
    .await
    .expect("guest session should expire");
    let expired = send(
        &router,
        Request::get("/api/v1/session")
            .header(COOKIE, guest.cookie)
            .body(Body::empty())
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(expired.status(), StatusCode::UNAUTHORIZED);

    let logout = send(
        &router,
        Request::delete("/api/v1/session")
            .header(COOKIE, &owner.cookie)
            .header("x-csrf-token", &owner.csrf)
            .body(Body::empty())
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(logout.status(), StatusCode::NO_CONTENT);
    let revoked = send(
        &router,
        Request::get("/api/v1/session")
            .header(COOKIE, owner.cookie)
            .body(Body::empty())
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn rate_limits_login_and_upload_and_enforces_storage_budget() {
    let (repository, _pool, storage) = setup().await;
    initialize_owner(&repository, USERNAME, PASSWORD)
        .await
        .expect("owner should initialize");
    let config = AppConfig {
        login_attempts_per_minute: 2,
        uploads_per_minute: 1,
        storage_budget_bytes: 1,
        ..AppConfig::default()
    };
    let router = app_with_config(repository, storage, config);
    for expected in [
        StatusCode::UNAUTHORIZED,
        StatusCode::UNAUTHORIZED,
        StatusCode::TOO_MANY_REQUESTS,
    ] {
        assert_eq!(
            login_response(&router, USERNAME, "incorrect-password")
                .await
                .status(),
            expected
        );
    }

    // The login key above is saturated, so use the same valid owner through a fresh router state.
    let database_url = env::var("TEST_DATABASE_URL").expect("test URL should remain available");
    let repository = PgJobRepository::connect(&database_url, 5)
        .await
        .expect("database should reconnect");
    let storage = LocalStorage::initialize(
        tempfile::tempdir()
            .expect("temporary storage should be created")
            .keep(),
    )
    .await
    .expect("storage should initialize");
    let router = app_with_config(
        repository,
        storage,
        AppConfig {
            uploads_per_minute: 1,
            storage_budget_bytes: 1,
            ..AppConfig::default()
        },
    );
    let owner = login(&router, USERNAME, PASSWORD).await;
    let (content_type, body) = multipart_image("budget test");
    let first = send(
        &router,
        Request::post("/api/v1/jobs")
            .header(COOKIE, &owner.cookie)
            .header("x-csrf-token", &owner.csrf)
            .header(CONTENT_TYPE, &content_type)
            .body(Body::from(body))
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(first.status(), StatusCode::INSUFFICIENT_STORAGE);
    let second = send(
        &router,
        Request::post("/api/v1/jobs")
            .header(COOKIE, owner.cookie)
            .header("x-csrf-token", owner.csrf)
            .header(CONTENT_TYPE, content_type)
            .body(Body::empty())
            .expect("request should be valid"),
    )
    .await;
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

struct TestSession {
    cookie: String,
    csrf: String,
    set_cookies: Vec<String>,
}

async fn login(router: &Router, username: &str, password: &str) -> TestSession {
    let response = login_response(router, username, password).await;
    assert_eq!(response.status(), StatusCode::CREATED);
    assert_eq!(
        response
            .headers()
            .get(CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store")
    );
    assert_eq!(
        response
            .headers()
            .get("x-content-type-options")
            .and_then(|value| value.to_str().ok()),
        Some("nosniff")
    );
    let set_cookies = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|value| value.to_str().expect("cookie should be text").to_owned())
        .collect::<Vec<_>>();
    let cookie = set_cookies
        .iter()
        .map(|value| value.split(';').next().expect("cookie should have a pair"))
        .collect::<Vec<_>>()
        .join("; ");
    let body = json_body(response).await;
    TestSession {
        cookie,
        csrf: body["csrf_token"]
            .as_str()
            .expect("login should return a CSRF token")
            .to_owned(),
        set_cookies,
    }
}

async fn login_response(
    router: &Router,
    username: &str,
    password: &str,
) -> axum::response::Response {
    send(
        router,
        Request::post("/api/v1/session/login")
            .header(CONTENT_TYPE, "application/json")
            .body(Body::from(
                json!({ "username": username, "password": password }).to_string(),
            ))
            .expect("login request should be valid"),
    )
    .await
}

async fn send(router: &Router, request: Request<Body>) -> axum::response::Response {
    router
        .clone()
        .oneshot(request)
        .await
        .expect("router should return a response")
}

async fn json_body(response: axum::response::Response) -> Value {
    let body = to_bytes(response.into_body(), 30 * 1024 * 1024)
        .await
        .expect("response body should be readable");
    serde_json::from_slice(&body).expect("response should be JSON")
}

async fn setup() -> (PgJobRepository, PgPool, LocalStorage) {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 8)
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
    sqlx::query("DELETE FROM users WHERE id <> 1")
        .execute(&pool)
        .await
        .expect("extra test users should reset");
    let temporary = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(temporary.keep())
        .await
        .expect("temporary storage should initialize");
    (repository, pool, storage)
}

fn multipart_image(name: &str) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "taskharbor-security-boundary";
    let image = RgbaImage::from_pixel(2, 2, Rgba([20, 120, 220, 255]));
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(image.as_raw(), 2, 2, ColorType::Rgba8.into())
        .expect("test PNG should encode");
    let mut body = Vec::new();
    for (field, value) in [("name", name), ("max_width", "2"), ("jpeg_quality", "85")] {
        body.extend_from_slice(
            format!(
                "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n"
            )
            .as_bytes(),
        );
    }
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"images\"; filename=\"source.png\"\r\nContent-Type: image/png\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(&png);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={BOUNDARY}"), body)
}
