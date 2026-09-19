use axum::body::Body;
use axum::extract::multipart::MultipartRejection;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use taskharbor_adapters::{
    ArtifactKind, ArtifactRecord, JobRecord, JobSettings, MAX_TOTAL_FILE_BYTES,
};
use taskharbor_core::{JobId, JobStatus, JobType};
use time::OffsetDateTime;
use tokio_util::io::ReaderStream;

use crate::AppState;
use crate::error::ApiError;
use crate::upload;

const MULTIPART_OVERHEAD_BYTES: usize = 1024 * 1024;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/jobs", get(list_jobs).post(create_job))
        .route("/api/v1/jobs/{id}", get(get_job))
        .route("/api/v1/artifacts/{id}/download", get(download_artifact))
        .layer(DefaultBodyLimit::max(
            usize::try_from(MAX_TOTAL_FILE_BYTES).unwrap_or(25 * 1024 * 1024)
                + MULTIPART_OVERHEAD_BYTES,
        ))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    state.jobs.ping().await.map_err(ApiError::repository)?;

    Ok(Json(HealthResponse { status: "ok" }))
}

async fn create_job(
    State(state): State<AppState>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<JobResponse>), ApiError> {
    let job = upload::create_image_job(&state, multipart).await?;

    Ok((StatusCode::CREATED, Json(job.into())))
}

async fn list_jobs(State(state): State<AppState>) -> Result<Json<ListJobsResponse>, ApiError> {
    let jobs = state
        .jobs
        .list()
        .await
        .map_err(ApiError::repository)?
        .into_iter()
        .map(Into::into)
        .collect();

    Ok(Json(ListJobsResponse { jobs }))
}

async fn get_job(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<Json<JobResponse>, ApiError> {
    let id = raw_id
        .parse::<JobId>()
        .map_err(|_| ApiError::job_not_found())?;
    let job = state
        .jobs
        .get(id)
        .await
        .map_err(ApiError::repository)?
        .ok_or_else(ApiError::job_not_found)?;

    Ok(Json(job.into()))
}

async fn download_artifact(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<Response, ApiError> {
    let artifact_id = raw_id
        .parse::<u64>()
        .ok()
        .filter(|id| *id > 0)
        .ok_or_else(ApiError::artifact_not_found)?;
    let artifact = state
        .jobs
        .get_downloadable_artifact(artifact_id)
        .await
        .map_err(ApiError::repository)?
        .ok_or_else(ApiError::artifact_not_found)?;
    let file = state
        .storage
        .open(artifact.storage_key())
        .await
        .map_err(ApiError::artifact_unavailable)?;
    let stream = ReaderStream::new(file);
    let mut headers = HeaderMap::new();
    headers.insert(CONTENT_TYPE, HeaderValue::from_static("image/jpeg"));
    headers.insert(
        CONTENT_LENGTH,
        HeaderValue::from_str(&artifact.byte_size().to_string())
            .map_err(|_| ApiError::artifact_not_found())?,
    );
    headers.insert(
        CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!(
            "attachment; filename=\"taskharbor-artifact-{artifact_id}.jpg\""
        ))
        .map_err(|_| ApiError::artifact_not_found())?,
    );

    Ok((headers, Body::from_stream(stream)).into_response())
}

#[derive(Debug, Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Debug, Serialize)]
struct ListJobsResponse {
    jobs: Vec<JobResponse>,
}

#[derive(Debug, Serialize)]
struct JobResponse {
    id: u64,
    name: String,
    job_type: JobTypeResponse,
    status: JobStatusResponse,
    progress: ProgressResponse,
    delay_ms: Option<u64>,
    image_settings: Option<ImageSettingsResponse>,
    inputs: Vec<ArtifactResponse>,
    outputs: Vec<ArtifactResponse>,
    result: Option<JobResultResponse>,
    failure_message: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    finished_at: Option<OffsetDateTime>,
}

impl From<JobRecord> for JobResponse {
    fn from(record: JobRecord) -> Self {
        let result = record.result_message().map(|message| JobResultResponse {
            message: message.to_owned(),
            duration_ms: record.result_duration_ms(),
        });
        let failure_message = record.failure_message().map(str::to_owned);
        let (delay_ms, image_settings) = match *record.settings() {
            JobSettings::DemoDelay { delay_ms } => (Some(delay_ms), None),
            JobSettings::ImageResize {
                max_width,
                jpeg_quality,
            } => (
                None,
                Some(ImageSettingsResponse {
                    max_width,
                    jpeg_quality,
                    output_media_type: "image/jpeg",
                    transparency_background: "white",
                }),
            ),
        };
        let inputs = record.inputs().map(ArtifactResponse::input).collect();
        let outputs = record.outputs().map(ArtifactResponse::output).collect();

        Self {
            id: record.job().id().get(),
            name: record.job().name().as_str().to_owned(),
            job_type: record.job().job_type().into(),
            status: record.job().status().into(),
            progress: ProgressResponse {
                completed: record.progress_completed(),
                total: record.progress_total(),
            },
            delay_ms,
            image_settings,
            inputs,
            outputs,
            result,
            failure_message,
            created_at: record.created_at(),
            started_at: record.started_at(),
            finished_at: record.finished_at(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ImageSettingsResponse {
    max_width: u32,
    jpeg_quality: u8,
    output_media_type: &'static str,
    transparency_background: &'static str,
}

#[derive(Debug, Serialize)]
struct ArtifactResponse {
    id: u64,
    item_index: u16,
    filename: String,
    media_type: String,
    byte_size: u64,
    width: u32,
    height: u32,
    download_url: Option<String>,
}

impl ArtifactResponse {
    fn input(artifact: &ArtifactRecord) -> Self {
        debug_assert_eq!(artifact.kind(), ArtifactKind::Input);
        Self::new(artifact, None)
    }

    fn output(artifact: &ArtifactRecord) -> Self {
        debug_assert_eq!(artifact.kind(), ArtifactKind::Output);
        Self::new(
            artifact,
            Some(format!("/api/v1/artifacts/{}/download", artifact.id())),
        )
    }

    fn new(artifact: &ArtifactRecord, download_url: Option<String>) -> Self {
        Self {
            id: artifact.id(),
            item_index: artifact.item_index(),
            filename: artifact.display_name().to_owned(),
            media_type: artifact.media_type().to_owned(),
            byte_size: artifact.byte_size(),
            width: artifact.width(),
            height: artifact.height(),
            download_url,
        }
    }
}

#[derive(Debug, Serialize)]
struct ProgressResponse {
    completed: u32,
    total: u32,
}

#[derive(Debug, Serialize)]
struct JobResultResponse {
    message: String,
    duration_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobStatusResponse {
    Queued,
    Running,
    Succeeded,
    Failed,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobTypeResponse {
    DemoDelay,
    ImageResize,
}

impl From<JobType> for JobTypeResponse {
    fn from(job_type: JobType) -> Self {
        match job_type {
            JobType::DemoDelay => Self::DemoDelay,
            JobType::ImageResize => Self::ImageResize,
        }
    }
}

impl From<JobStatus> for JobStatusResponse {
    fn from(status: JobStatus) -> Self {
        match status {
            JobStatus::Queued => Self::Queued,
            JobStatus::Running => Self::Running,
            JobStatus::Succeeded => Self::Succeeded,
            JobStatus::Failed => Self::Failed,
        }
    }
}
