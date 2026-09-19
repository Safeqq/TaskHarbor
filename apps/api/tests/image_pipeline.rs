use std::env;

use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderValue, Request, StatusCode};
use image::codecs::png::PngEncoder;
use image::{ColorType, GenericImageView, ImageEncoder, Rgba, RgbaImage};
use serde_json::Value;
use sqlx::PgPool;
use taskharbor_adapters::{ImageService, LocalStorage, MAX_FILE_BYTES, PgJobRepository};
use taskharbor_api::app;
use taskharbor_worker::process_next;
use tower::ServiceExt;

#[tokio::test]
#[ignore = "requires TEST_DATABASE_URL pointing to PostgreSQL"]
async fn uploads_processes_and_downloads_a_real_image() {
    let database_url = env::var("TEST_DATABASE_URL")
        .expect("TEST_DATABASE_URL must point to an isolated test database");
    let repository = PgJobRepository::connect(&database_url, 4)
        .await
        .expect("test database should be reachable");
    repository
        .migrate()
        .await
        .expect("migrations should succeed");
    let setup_pool = PgPool::connect(&database_url)
        .await
        .expect("test setup should connect");
    sqlx::query("TRUNCATE artifacts, job_attempts, jobs RESTART IDENTITY CASCADE")
        .execute(&setup_pool)
        .await
        .expect("test tables should be reset");

    let temporary = tempfile::tempdir().expect("temporary storage should be created");
    let storage = LocalStorage::initialize(temporary.path())
        .await
        .expect("temporary storage should initialize");
    let router = app(repository.clone(), storage.clone());
    let source = transparent_png();
    let (content_type, body) = multipart_job(
        "transparent sample",
        "../../client-name.fake",
        &source,
        4,
        92,
    );

    let created = send_json(
        router.clone(),
        Request::post("/api/v1/jobs")
            .header(CONTENT_TYPE, content_type)
            .body(Body::from(body))
            .expect("create request should be valid"),
    )
    .await;
    assert_eq!(created.0, StatusCode::CREATED);
    assert_eq!(created.1["job_type"], "image_resize");
    assert_eq!(created.1["inputs"][0]["media_type"], "image/png");
    let stored_name = created.1["inputs"][0]["filename"]
        .as_str()
        .expect("input should contain a display filename");
    assert!(!stored_name.contains(['/', '\\']));
    let job_id = created.1["id"]
        .as_u64()
        .expect("created job should contain an ID");

    let images = ImageService::new(storage.clone(), 1);
    assert!(
        process_next(&repository, &images, &storage)
            .await
            .expect("worker should process the image job")
    );

    let completed = send_json(
        router.clone(),
        Request::get(format!("/api/v1/jobs/{job_id}"))
            .body(Body::empty())
            .expect("detail request should be valid"),
    )
    .await;
    assert_eq!(completed.0, StatusCode::OK);
    assert_eq!(completed.1["status"], "succeeded");
    assert_eq!(
        completed.1["progress"],
        serde_json::json!({ "completed": 1, "total": 1 })
    );
    assert_eq!(completed.1["outputs"][0]["width"], 4);
    assert_eq!(completed.1["outputs"][0]["height"], 2);
    assert_eq!(completed.1["outputs"][0]["media_type"], "image/jpeg");
    let download_url = completed.1["outputs"][0]["download_url"]
        .as_str()
        .expect("completed output should have a download URL");

    let download = router
        .clone()
        .oneshot(
            Request::get(download_url)
                .body(Body::empty())
                .expect("download request should be valid"),
        )
        .await
        .expect("download route should respond");
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(
        download.headers()[CONTENT_TYPE],
        HeaderValue::from_static("image/jpeg")
    );
    assert!(download.headers().contains_key(CONTENT_DISPOSITION));
    let downloaded = to_bytes(download.into_body(), 10 * 1024 * 1024)
        .await
        .expect("download body should be readable");
    let decoded = image::load_from_memory(&downloaded).expect("download should be a valid JPEG");
    assert_eq!(decoded.dimensions(), (4, 2));
    let rgb = decoded.to_rgb8();
    assert!(rgb.get_pixel(0, 0).0.iter().all(|channel| *channel >= 245));

    let (content_type, corrupt_body) =
        multipart_job("corrupt", "looks-valid.png", b"not an image", 100, 85);
    let corrupt = send_json(
        router.clone(),
        Request::post("/api/v1/jobs")
            .header(CONTENT_TYPE, content_type)
            .body(Body::from(corrupt_body))
            .expect("corrupt upload request should be valid HTTP"),
    )
    .await;
    assert_eq!(corrupt.0, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(corrupt.1["error"]["code"], "invalid_image");

    let oversized =
        vec![0_u8; usize::try_from(MAX_FILE_BYTES).expect("limit should fit usize") + 1];
    let (content_type, oversized_body) =
        multipart_job("oversized", "large.png", &oversized, 100, 85);
    let rejected = send_json(
        router.clone(),
        Request::post("/api/v1/jobs")
            .header(CONTENT_TYPE, content_type)
            .body(Body::from(oversized_body))
            .expect("oversized upload request should be valid HTTP"),
    )
    .await;
    assert_eq!(rejected.0, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(rejected.1["error"]["code"], "file_too_large");

    let listed = send_json(
        router,
        Request::get("/api/v1/jobs")
            .body(Body::empty())
            .expect("list request should be valid"),
    )
    .await;
    assert_eq!(
        listed.1["jobs"]
            .as_array()
            .expect("jobs should be an array")
            .len(),
        1
    );
}

fn transparent_png() -> Vec<u8> {
    let image = RgbaImage::from_pixel(8, 4, Rgba([230, 20, 20, 0]));
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(image.as_raw(), 8, 4, ColorType::Rgba8.into())
        .expect("test PNG should encode");
    bytes
}

fn multipart_job(
    name: &str,
    filename: &str,
    image: &[u8],
    max_width: u32,
    quality: u8,
) -> (String, Vec<u8>) {
    const BOUNDARY: &str = "taskharbor-pipeline-boundary";
    let mut body = Vec::new();
    append_text(&mut body, BOUNDARY, "name", name);
    append_text(&mut body, BOUNDARY, "max_width", &max_width.to_string());
    append_text(&mut body, BOUNDARY, "jpeg_quality", &quality.to_string());
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

fn append_text(body: &mut Vec<u8>, boundary: &str, name: &str, value: &str) {
    body.extend_from_slice(
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        )
        .as_bytes(),
    );
}

async fn send_json(app: Router, request: Request<Body>) -> (StatusCode, Value) {
    let response = app
        .oneshot(request)
        .await
        .expect("router should return a response");
    let status = response.status();
    let body = to_bytes(response.into_body(), 30 * 1024 * 1024)
        .await
        .expect("response body should be readable");
    let body = serde_json::from_slice(&body).expect("response should contain valid JSON");
    (status, body)
}
