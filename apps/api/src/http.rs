use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use taskharbor_adapters::JobRecord;
use taskharbor_core::{JobId, JobName, JobStatus};
use time::OffsetDateTime;

use crate::AppState;
use crate::error::ApiError;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/jobs", get(list_jobs).post(create_job))
        .route("/api/v1/jobs/{id}", get(get_job))
        .with_state(state)
}

async fn health(State(state): State<AppState>) -> Result<Json<HealthResponse>, ApiError> {
    state.jobs.ping().await.map_err(ApiError::repository)?;

    Ok(Json(HealthResponse { status: "ok" }))
}

async fn create_job(
    State(state): State<AppState>,
    payload: Result<Json<CreateJobRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<JobResponse>), ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::invalid_json())?;
    let name = JobName::new(payload.name).map_err(ApiError::invalid_job_name)?;
    let job = state
        .jobs
        .create(name)
        .await
        .map_err(ApiError::repository)?;

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

#[derive(Debug, Deserialize)]
struct CreateJobRequest {
    name: String,
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
    job_type: &'static str,
    status: JobStatusResponse,
    progress: ProgressResponse,
    delay_ms: u64,
    result: Option<JobResultResponse>,
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

        Self {
            id: record.job().id().get(),
            name: record.job().name().as_str().to_owned(),
            job_type: "demo_delay",
            status: record.job().status().into(),
            progress: ProgressResponse {
                completed: record.progress_completed(),
                total: record.progress_total(),
            },
            delay_ms: record.delay_ms(),
            result,
            created_at: record.created_at(),
            started_at: record.started_at(),
            finished_at: record.finished_at(),
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
