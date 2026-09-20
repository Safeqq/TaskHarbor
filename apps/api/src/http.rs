use axum::body::Body;
use axum::extract::multipart::MultipartRejection;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::header::{CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use taskharbor_adapters::{
    ArtifactKind, ArtifactRecord, AttemptRecord, AttemptStatus, JobRecord, JobSettings,
    MAX_OUTPUT_WIDTH, MAX_SCHEDULE_INTERVAL_SECONDS, MAX_TOTAL_FILE_BYTES,
    MIN_SCHEDULE_INTERVAL_SECONDS, ScheduleInputRecord, ScheduleOccurrenceRecord, ScheduleRecord,
    UpdateSchedule, WorkerRecord,
};
use taskharbor_core::{JobId, JobName, JobPriority, JobStatus, JobType, ScheduleId};
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
        .route("/api/v1/jobs/{id}/cancel", post(cancel_job))
        .route("/api/v1/jobs/{id}/retry", post(retry_job))
        .route("/api/v1/workers", get(list_workers))
        .route(
            "/api/v1/schedules",
            get(list_schedules).post(create_schedule),
        )
        .route(
            "/api/v1/schedules/{id}",
            get(get_schedule).put(update_schedule),
        )
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

async fn cancel_job(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<Json<JobResponse>, ApiError> {
    let id = parse_job_id(&raw_id)?;
    let job = state
        .jobs
        .request_cancel(id)
        .await
        .map_err(ApiError::repository)?
        .ok_or_else(ApiError::job_not_found)?;

    Ok(Json(job.into()))
}

async fn retry_job(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<(StatusCode, Json<JobResponse>), ApiError> {
    let id = parse_job_id(&raw_id)?;
    let job = state
        .jobs
        .manual_retry(id)
        .await
        .map_err(ApiError::repository)?
        .ok_or_else(ApiError::job_not_found)?;

    Ok((StatusCode::CREATED, Json(job.into())))
}

async fn create_schedule(
    State(state): State<AppState>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<(StatusCode, Json<ScheduleResponse>), ApiError> {
    let schedule = upload::create_schedule(&state, multipart).await?;
    Ok((StatusCode::CREATED, Json(schedule.into())))
}

async fn list_schedules(
    State(state): State<AppState>,
) -> Result<Json<ListSchedulesResponse>, ApiError> {
    let schedules = state
        .jobs
        .list_schedules()
        .await
        .map_err(ApiError::repository)?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(ListSchedulesResponse { schedules }))
}

async fn list_workers(
    State(state): State<AppState>,
) -> Result<Json<ListWorkersResponse>, ApiError> {
    let workers = state
        .jobs
        .list_workers()
        .await
        .map_err(ApiError::repository)?
        .into_iter()
        .map(Into::into)
        .collect();
    Ok(Json(ListWorkersResponse { workers }))
}

async fn get_schedule(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
) -> Result<Json<ScheduleResponse>, ApiError> {
    let id = parse_schedule_id(&raw_id)?;
    let schedule = state
        .jobs
        .get_schedule(id)
        .await
        .map_err(ApiError::repository)?
        .ok_or_else(ApiError::schedule_not_found)?;
    Ok(Json(schedule.into()))
}

async fn update_schedule(
    State(state): State<AppState>,
    Path(raw_id): Path<String>,
    Json(request): Json<UpdateScheduleRequest>,
) -> Result<Json<ScheduleResponse>, ApiError> {
    let id = parse_schedule_id(&raw_id)?;
    let request = request.validate()?;
    let schedule = state
        .jobs
        .update_schedule(id, request)
        .await
        .map_err(ApiError::repository)?
        .ok_or_else(ApiError::schedule_not_found)?;
    Ok(Json(schedule.into()))
}

fn parse_job_id(raw_id: &str) -> Result<JobId, ApiError> {
    raw_id
        .parse::<JobId>()
        .map_err(|_| ApiError::job_not_found())
}

fn parse_schedule_id(raw_id: &str) -> Result<ScheduleId, ApiError> {
    raw_id
        .parse::<ScheduleId>()
        .map_err(|_| ApiError::schedule_not_found())
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
struct ListSchedulesResponse {
    schedules: Vec<ScheduleResponse>,
}

#[derive(Debug, Serialize)]
struct ListWorkersResponse {
    workers: Vec<WorkerResponse>,
}

#[derive(Debug, Serialize)]
struct WorkerResponse {
    id: String,
    name: String,
    status: &'static str,
    concurrency_limit: u16,
    lease_duration_seconds: u32,
    active_attempts: u32,
    #[serde(with = "time::serde::rfc3339")]
    started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    last_heartbeat_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    heartbeat_expires_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    stopped_at: Option<OffsetDateTime>,
}

impl From<WorkerRecord> for WorkerResponse {
    fn from(record: WorkerRecord) -> Self {
        Self {
            id: record.id().to_string(),
            name: record.name().to_owned(),
            status: record.status().as_str(),
            concurrency_limit: record.concurrency_limit(),
            lease_duration_seconds: record.lease_duration_seconds(),
            active_attempts: record.active_attempts(),
            started_at: record.started_at(),
            last_heartbeat_at: record.last_heartbeat_at(),
            heartbeat_expires_at: record.heartbeat_expires_at(),
            stopped_at: record.stopped_at(),
        }
    }
}

#[derive(Debug, Deserialize)]
struct UpdateScheduleRequest {
    name: String,
    enabled: bool,
    interval_seconds: u32,
    #[serde(with = "time::serde::rfc3339")]
    anchor_at: OffsetDateTime,
    priority: String,
    max_width: u32,
    jpeg_quality: u8,
}

impl UpdateScheduleRequest {
    fn validate(self) -> Result<UpdateSchedule, ApiError> {
        let name = JobName::new(self.name).map_err(ApiError::invalid_job_name)?;
        if !(MIN_SCHEDULE_INTERVAL_SECONDS..=MAX_SCHEDULE_INTERVAL_SECONDS)
            .contains(&self.interval_seconds)
        {
            return Err(ApiError::upload_validation(
                "invalid_interval",
                format!(
                    "interval must be between {MIN_SCHEDULE_INTERVAL_SECONDS} and {MAX_SCHEDULE_INTERVAL_SECONDS} seconds"
                ),
                "interval_seconds",
            ));
        }
        let priority = self.priority.parse::<JobPriority>().map_err(|_| {
            ApiError::upload_validation(
                "invalid_priority",
                "priority must be high, normal, or low",
                "priority",
            )
        })?;
        if self.max_width == 0 || self.max_width > MAX_OUTPUT_WIDTH {
            return Err(ApiError::upload_validation(
                "invalid_max_width",
                format!("max width must be between 1 and {MAX_OUTPUT_WIDTH} pixels"),
                "max_width",
            ));
        }
        if !(1..=100).contains(&self.jpeg_quality) {
            return Err(ApiError::upload_validation(
                "invalid_jpeg_quality",
                "JPEG quality must be between 1 and 100",
                "jpeg_quality",
            ));
        }

        Ok(UpdateSchedule {
            name,
            enabled: self.enabled,
            interval_seconds: self.interval_seconds,
            anchor_at: self.anchor_at,
            priority,
            max_width: self.max_width,
            jpeg_quality: self.jpeg_quality,
        })
    }
}

#[derive(Debug, Serialize)]
struct ScheduleResponse {
    id: u64,
    name: String,
    enabled: bool,
    interval_seconds: u32,
    #[serde(with = "time::serde::rfc3339")]
    anchor_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    next_run_at: OffsetDateTime,
    priority: &'static str,
    image_settings: ImageSettingsResponse,
    inputs: Vec<ScheduleInputResponse>,
    occurrences: Vec<ScheduleOccurrenceResponse>,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    updated_at: OffsetDateTime,
}

impl From<ScheduleRecord> for ScheduleResponse {
    fn from(record: ScheduleRecord) -> Self {
        Self {
            id: record.id().get(),
            name: record.name().as_str().to_owned(),
            enabled: record.enabled(),
            interval_seconds: record.interval_seconds(),
            anchor_at: record.anchor_at(),
            next_run_at: record.next_run_at(),
            priority: record.priority().as_str(),
            image_settings: ImageSettingsResponse {
                max_width: record.max_width(),
                jpeg_quality: record.jpeg_quality(),
                output_media_type: "image/jpeg",
                transparency_background: "white",
            },
            inputs: record.inputs().map(Into::into).collect(),
            occurrences: record.occurrences().map(Into::into).collect(),
            created_at: record.created_at(),
            updated_at: record.updated_at(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ScheduleInputResponse {
    id: u64,
    item_index: u16,
    filename: String,
    media_type: String,
    byte_size: u64,
    width: u32,
    height: u32,
}

impl From<&ScheduleInputRecord> for ScheduleInputResponse {
    fn from(input: &ScheduleInputRecord) -> Self {
        Self {
            id: input.id(),
            item_index: input.item_index(),
            filename: input.display_name().to_owned(),
            media_type: input.media_type().to_owned(),
            byte_size: input.byte_size(),
            width: input.width(),
            height: input.height(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ScheduleOccurrenceResponse {
    id: u64,
    #[serde(with = "time::serde::rfc3339")]
    scheduled_for: OffsetDateTime,
    outcome: &'static str,
    job_id: Option<u64>,
    job_status: Option<JobStatusResponse>,
    reason: Option<String>,
    coalesced_slots: u64,
    #[serde(with = "time::serde::rfc3339")]
    created_at: OffsetDateTime,
}

impl From<&ScheduleOccurrenceRecord> for ScheduleOccurrenceResponse {
    fn from(occurrence: &ScheduleOccurrenceRecord) -> Self {
        Self {
            id: occurrence.id(),
            scheduled_for: occurrence.scheduled_for(),
            outcome: occurrence.outcome().as_str(),
            job_id: occurrence.job_id().map(JobId::get),
            job_status: occurrence.job_status().map(Into::into),
            reason: occurrence.reason().map(str::to_owned),
            coalesced_slots: occurrence.coalesced_slots(),
            created_at: occurrence.created_at(),
        }
    }
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
    attempts: Vec<AttemptResponse>,
    #[serde(with = "time::serde::rfc3339")]
    available_at: OffsetDateTime,
    priority: &'static str,
    schedule_id: Option<u64>,
    #[serde(with = "time::serde::rfc3339::option")]
    scheduled_for: Option<OffsetDateTime>,
    max_attempts: u32,
    retry_of_job_id: Option<u64>,
    result: Option<JobResultResponse>,
    failure_message: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    cancel_requested_at: Option<OffsetDateTime>,
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
        let attempts = record.attempts().map(AttemptResponse::from).collect();

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
            attempts,
            available_at: record.available_at(),
            priority: record.priority().as_str(),
            schedule_id: record.schedule_id().map(ScheduleId::get),
            scheduled_for: record.scheduled_for(),
            max_attempts: record.max_attempts(),
            retry_of_job_id: record.retry_of_job_id().map(JobId::get),
            result,
            failure_message,
            cancel_requested_at: record.cancel_requested_at(),
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
struct AttemptResponse {
    id: i64,
    number: u32,
    status: AttemptStatusResponse,
    progress: ProgressResponse,
    #[serde(with = "time::serde::rfc3339")]
    started_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    finished_at: Option<OffsetDateTime>,
    duration_ms: Option<u64>,
    error_kind: Option<String>,
    error_message: Option<String>,
    worker: Option<AttemptWorkerResponse>,
    #[serde(with = "time::serde::rfc3339::option")]
    lease_expires_at: Option<OffsetDateTime>,
}

#[derive(Debug, Serialize)]
struct AttemptWorkerResponse {
    id: String,
    name: String,
}

impl From<&AttemptRecord> for AttemptResponse {
    fn from(attempt: &AttemptRecord) -> Self {
        Self {
            id: attempt.id(),
            number: attempt.attempt_number(),
            status: attempt.status().into(),
            progress: ProgressResponse {
                completed: attempt.progress_completed(),
                total: attempt.progress_total(),
            },
            started_at: attempt.started_at(),
            finished_at: attempt.finished_at(),
            duration_ms: attempt.duration_ms(),
            error_kind: attempt.error_kind().map(str::to_owned),
            error_message: attempt.error_message().map(str::to_owned),
            worker: attempt.worker_id().map(|worker_id| AttemptWorkerResponse {
                id: worker_id.to_string(),
                name: attempt.worker_name().unwrap_or("unknown worker").to_owned(),
            }),
            lease_expires_at: attempt.lease_expires_at(),
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
    RetryWaiting,
    CancelRequested,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum AttemptStatusResponse {
    Running,
    Succeeded,
    Failed,
    Cancelled,
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
            JobStatus::RetryWaiting => Self::RetryWaiting,
            JobStatus::CancelRequested => Self::CancelRequested,
            JobStatus::Succeeded => Self::Succeeded,
            JobStatus::Failed => Self::Failed,
            JobStatus::Cancelled => Self::Cancelled,
        }
    }
}

impl From<AttemptStatus> for AttemptStatusResponse {
    fn from(status: AttemptStatus) -> Self {
        match status {
            AttemptStatus::Running => Self::Running,
            AttemptStatus::Succeeded => Self::Succeeded,
            AttemptStatus::Failed => Self::Failed,
            AttemptStatus::Cancelled => Self::Cancelled,
        }
    }
}
