use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderValue, Method, Request, StatusCode};
use axum::middleware::{self, Next};
use taskharbor_adapters::{LocalStorage, PgJobRepository};
use taskharbor_api::{app, initialize_owner};
use tower::ServiceExt;

const TEST_USERNAME: &str = "owner";
const TEST_PASSWORD: &str = "test-owner-password";

pub async fn authenticated_app(repository: PgJobRepository, storage: LocalStorage) -> Router {
    let _ = tracing_subscriber::fmt().with_test_writer().try_init();
    initialize_owner(&repository, TEST_USERNAME, TEST_PASSWORD)
        .await
        .expect("test owner should initialize");
    let router = app(repository, storage);
    let response = router
        .clone()
        .oneshot(
            Request::post("/api/v1/session/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "username": TEST_USERNAME,
                        "password": TEST_PASSWORD
                    })
                    .to_string(),
                ))
                .expect("login request should be valid"),
        )
        .await
        .expect("login should return a response");
    assert_eq!(response.status(), StatusCode::CREATED);
    let cookies = response
        .headers()
        .get_all(SET_COOKIE)
        .iter()
        .map(|value| {
            value
                .to_str()
                .expect("test cookie should be text")
                .split(';')
                .next()
                .expect("test cookie should have a value")
                .to_owned()
        })
        .collect::<Vec<_>>();
    let cookie = HeaderValue::from_str(&cookies.join("; ")).expect("cookie header should be valid");
    let body = to_bytes(response.into_body(), 1024 * 1024)
        .await
        .expect("login body should be readable");
    let session: serde_json::Value =
        serde_json::from_slice(&body).expect("login response should be JSON");
    let csrf = HeaderValue::from_str(
        session["csrf_token"]
            .as_str()
            .expect("login response should include CSRF token"),
    )
    .expect("CSRF header should be valid");

    router.layer(middleware::from_fn(
        move |mut request: Request<Body>, next: Next| {
            let cookie = cookie.clone();
            let csrf = csrf.clone();
            async move {
                request.headers_mut().insert(COOKIE, cookie);
                if !matches!(
                    *request.method(),
                    Method::GET | Method::HEAD | Method::OPTIONS
                ) {
                    request.headers_mut().insert("x-csrf-token", csrf);
                }
                next.run(request).await
            }
        },
    ))
}
