use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use taskharbor_core::{Job, JobId, JobName, JobStatus};

use crate::AppState;
use crate::error::ApiError;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/jobs", get(list_jobs).post(create_job))
        .route("/api/v1/jobs/{id}", get(get_job))
        .with_state(state)
}

async fn health() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn create_job(
    State(state): State<AppState>,
    payload: Result<Json<CreateJobRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<JobResponse>), ApiError> {
    let Json(payload) = payload.map_err(|_| ApiError::invalid_json())?;
    let name = JobName::new(payload.name).map_err(ApiError::invalid_job_name)?;
    let job = state.jobs.create(name).await.map_err(ApiError::store)?;

    Ok((StatusCode::CREATED, Json(job.into())))
}

async fn list_jobs(State(state): State<AppState>) -> Json<ListJobsResponse> {
    let jobs = state
        .jobs
        .list()
        .await
        .into_iter()
        .map(Into::into)
        .collect();

    Json(ListJobsResponse { jobs })
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
    status: JobStatusResponse,
}

impl From<Job> for JobResponse {
    fn from(job: Job) -> Self {
        Self {
            id: job.id().get(),
            name: job.name().as_str().to_owned(),
            status: job.status().into(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum JobStatusResponse {
    Queued,
}

impl From<JobStatus> for JobStatusResponse {
    fn from(status: JobStatus) -> Self {
        match status {
            JobStatus::Queued => Self::Queued,
        }
    }
}
