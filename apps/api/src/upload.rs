use axum::extract::Multipart;
use axum::extract::multipart::{Field, MultipartRejection};
use sha2::{Digest, Sha256};
use taskharbor_adapters::{
    DEFAULT_JPEG_QUALITY, DEFAULT_OUTPUT_WIDTH, IdempotencyRequest, ImageError, MAX_FILE_BYTES,
    MAX_FILES_PER_JOB, MAX_OUTPUT_WIDTH, MAX_SCHEDULE_INTERVAL_SECONDS, MAX_TOTAL_FILE_BYTES,
    MIN_SCHEDULE_INTERVAL_SECONDS, NewImageJob, NewInputArtifact, NewSchedule, UserId,
};
use taskharbor_core::{JobName, JobPriority};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::io::AsyncWriteExt;

use crate::AppState;
use crate::error::ApiError;

const MAX_TEXT_FIELD_BYTES: u64 = 512;

pub async fn create_image_job(
    state: &AppState,
    owner_user_id: UserId,
    idempotency_key: Option<String>,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<taskharbor_adapters::JobRecord, ApiError> {
    let _upload_guard = state.upload_guard.lock().await;
    let storage_usage = storage_usage(state).await?;
    let multipart = multipart
        .map_err(|_| ApiError::invalid_multipart("request must be valid multipart form data"))?;
    let batch = state
        .storage
        .begin_upload()
        .await
        .map_err(ApiError::storage)?;

    let staged = stage_fields(state, &batch, multipart, UploadTarget::Job, storage_usage).await;
    let request = match staged {
        Ok(request) => request,
        Err(error) => {
            let _ = batch.cleanup().await;
            return Err(error);
        }
    };

    // A database commit can have an ambiguous outcome if the connection drops. Keep the staged
    // inputs so a job that did commit never points at files we deleted.
    let idempotency = idempotency_key.map(|key| IdempotencyRequest {
        key,
        fingerprint: request_fingerprint(&request),
    });
    if let Some(idempotency) = &idempotency {
        match state
            .jobs
            .find_idempotent_image_job(owner_user_id, idempotency)
            .await
        {
            Ok(Some(job)) => {
                let _ = batch.cleanup().await;
                return Ok(job);
            }
            Ok(None) => {}
            Err(error) => {
                let _ = batch.cleanup().await;
                return Err(ApiError::repository(error));
            }
        }
    }
    let active_jobs = match state.jobs.active_job_count_for_owner(owner_user_id).await {
        Ok(active_jobs) => active_jobs,
        Err(error) => {
            let _ = batch.cleanup().await;
            return Err(ApiError::repository(error));
        }
    };
    if active_jobs >= u64::from(state.config.max_active_jobs) {
        let _ = batch.cleanup().await;
        return Err(ApiError::resource_limit(
            "active_job_limit",
            "finish or cancel an active job before creating another",
        ));
    }
    state
        .jobs
        .create_image_job(NewImageJob {
            owner_user_id,
            name: request.name,
            max_width: request.max_width,
            jpeg_quality: request.jpeg_quality,
            available_at: request.available_at,
            priority: request.priority,
            inputs: request.inputs,
            idempotency,
        })
        .await
        .map_err(ApiError::repository)
}

pub async fn create_schedule(
    state: &AppState,
    owner_user_id: UserId,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<taskharbor_adapters::ScheduleRecord, ApiError> {
    let _upload_guard = state.upload_guard.lock().await;
    let storage_usage = storage_usage(state).await?;
    let multipart = multipart
        .map_err(|_| ApiError::invalid_multipart("request must be valid multipart form data"))?;
    let batch = state
        .storage
        .begin_upload()
        .await
        .map_err(ApiError::storage)?;

    let staged = stage_fields(
        state,
        &batch,
        multipart,
        UploadTarget::Schedule,
        storage_usage,
    )
    .await;
    let request = match staged {
        Ok(request) => request,
        Err(error) => {
            let _ = batch.cleanup().await;
            return Err(error);
        }
    };

    state
        .jobs
        .create_schedule(NewSchedule {
            owner_user_id,
            name: request.name,
            interval_seconds: request.interval_seconds.ok_or_else(|| {
                ApiError::upload_validation(
                    "missing_interval",
                    "schedule interval is required",
                    "interval_seconds",
                )
            })?,
            anchor_at: request.anchor_at.ok_or_else(|| {
                ApiError::upload_validation(
                    "missing_anchor",
                    "schedule anchor time is required",
                    "anchor_at",
                )
            })?,
            priority: request.priority,
            max_width: request.max_width,
            jpeg_quality: request.jpeg_quality,
            inputs: request.inputs,
        })
        .await
        .map_err(ApiError::repository)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UploadTarget {
    Job,
    Schedule,
}

struct StagedImageFields {
    name: JobName,
    max_width: u32,
    jpeg_quality: u8,
    available_at: Option<OffsetDateTime>,
    priority: JobPriority,
    interval_seconds: Option<u32>,
    anchor_at: Option<OffsetDateTime>,
    inputs: Vec<NewInputArtifact>,
}

async fn stage_fields(
    state: &AppState,
    batch: &taskharbor_adapters::UploadBatch,
    mut multipart: Multipart,
    target: UploadTarget,
    storage_usage: u64,
) -> Result<StagedImageFields, ApiError> {
    let mut name = None;
    let mut max_width = None;
    let mut jpeg_quality = None;
    let mut available_at = None;
    let mut priority = None;
    let mut interval_seconds = None;
    let mut anchor_at = None;
    let mut inputs = Vec::new();
    let mut total_bytes = 0_u64;

    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::invalid_multipart("multipart body could not be read"))?
    {
        let field_name = field.name().unwrap_or_default().to_owned();
        match field_name.as_str() {
            "name" => {
                set_once(&mut name, read_small_text(field, "name").await?, "name")?;
            }
            "max_width" => {
                let value = read_small_text(field, "max_width").await?;
                let value = value.parse::<u32>().map_err(|_| {
                    ApiError::upload_validation(
                        "invalid_max_width",
                        format!("max width must be between 1 and {MAX_OUTPUT_WIDTH} pixels"),
                        "max_width",
                    )
                })?;
                if value == 0 || value > MAX_OUTPUT_WIDTH {
                    return Err(ApiError::upload_validation(
                        "invalid_max_width",
                        format!("max width must be between 1 and {MAX_OUTPUT_WIDTH} pixels"),
                        "max_width",
                    ));
                }
                set_once(&mut max_width, value, "max_width")?;
            }
            "jpeg_quality" => {
                let value = read_small_text(field, "jpeg_quality").await?;
                let value = value.parse::<u8>().map_err(|_| {
                    ApiError::upload_validation(
                        "invalid_jpeg_quality",
                        "JPEG quality must be between 1 and 100",
                        "jpeg_quality",
                    )
                })?;
                if !(1..=100).contains(&value) {
                    return Err(ApiError::upload_validation(
                        "invalid_jpeg_quality",
                        "JPEG quality must be between 1 and 100",
                        "jpeg_quality",
                    ));
                }
                set_once(&mut jpeg_quality, value, "jpeg_quality")?;
            }
            "priority" => {
                let value = read_small_text(field, "priority").await?;
                let value = value.parse::<JobPriority>().map_err(|_| {
                    ApiError::upload_validation(
                        "invalid_priority",
                        "priority must be high, normal, or low",
                        "priority",
                    )
                })?;
                set_once(&mut priority, value, "priority")?;
            }
            "available_at" if target == UploadTarget::Job => {
                let value = read_small_text(field, "available_at").await?;
                let value = parse_timestamp(&value, "available_at")?;
                set_once(&mut available_at, value, "available_at")?;
            }
            "interval_seconds" if target == UploadTarget::Schedule => {
                let value = read_small_text(field, "interval_seconds").await?;
                let value = value.parse::<u32>().map_err(|_| invalid_interval())?;
                if !(MIN_SCHEDULE_INTERVAL_SECONDS..=MAX_SCHEDULE_INTERVAL_SECONDS).contains(&value)
                {
                    return Err(invalid_interval());
                }
                set_once(&mut interval_seconds, value, "interval_seconds")?;
            }
            "anchor_at" if target == UploadTarget::Schedule => {
                let value = read_small_text(field, "anchor_at").await?;
                let value = parse_timestamp(&value, "anchor_at")?;
                set_once(&mut anchor_at, value, "anchor_at")?;
            }
            "images" => {
                if inputs.len() >= MAX_FILES_PER_JOB {
                    return Err(ApiError::upload_validation(
                        "too_many_files",
                        format!("a job can contain at most {MAX_FILES_PER_JOB} images"),
                        "images",
                    ));
                }
                let raw_name = field.file_name().ok_or_else(|| {
                    ApiError::upload_validation(
                        "missing_filename",
                        "each image field must include a filename",
                        "images",
                    )
                })?;
                let display_name = safe_display_name(raw_name, inputs.len());
                let (storage_key, mut file) =
                    batch.create_file().await.map_err(ApiError::storage)?;
                let (byte_size, checksum_sha256) = write_image_field(
                    &mut field,
                    &mut file,
                    &mut total_bytes,
                    storage_usage,
                    state.config.storage_budget_bytes,
                )
                .await?;
                file.flush().await.map_err(|error| {
                    ApiError::storage(taskharbor_adapters::StorageError::from(error))
                })?;
                drop(file);

                let metadata = state
                    .images
                    .inspect(&storage_key)
                    .await
                    .map_err(map_image_error)?;
                inputs.push(NewInputArtifact {
                    storage_key,
                    display_name,
                    media_type: metadata.media_type.to_owned(),
                    byte_size,
                    width: metadata.width,
                    height: metadata.height,
                    checksum_sha256,
                });
            }
            _ => {
                return Err(ApiError::upload_validation(
                    "unexpected_field",
                    format!("multipart field '{field_name}' is not supported"),
                    "images",
                ));
            }
        }
    }

    let name = name.ok_or_else(|| {
        ApiError::upload_validation("missing_name", "job name is required", "name")
    })?;
    let name = JobName::new(name).map_err(ApiError::invalid_job_name)?;
    if inputs.is_empty() {
        return Err(ApiError::upload_validation(
            "missing_images",
            "select at least one JPEG or PNG image",
            "images",
        ));
    }

    Ok(StagedImageFields {
        name,
        max_width: max_width.unwrap_or(DEFAULT_OUTPUT_WIDTH),
        jpeg_quality: jpeg_quality.unwrap_or(DEFAULT_JPEG_QUALITY),
        available_at,
        priority: priority.unwrap_or_default(),
        interval_seconds,
        anchor_at,
        inputs,
    })
}

fn parse_timestamp(value: &str, field: &'static str) -> Result<OffsetDateTime, ApiError> {
    OffsetDateTime::parse(value.trim(), &Rfc3339).map_err(|_| {
        ApiError::upload_validation(
            "invalid_timestamp",
            "timestamp must be an RFC 3339 value with a UTC offset",
            field,
        )
    })
}

fn invalid_interval() -> ApiError {
    ApiError::upload_validation(
        "invalid_interval",
        format!(
            "interval must be between {MIN_SCHEDULE_INTERVAL_SECONDS} and {MAX_SCHEDULE_INTERVAL_SECONDS} seconds"
        ),
        "interval_seconds",
    )
}

async fn write_image_field(
    field: &mut Field<'_>,
    file: &mut tokio::fs::File,
    total_bytes: &mut u64,
    storage_usage: u64,
    storage_budget: u64,
) -> Result<(u64, Vec<u8>), ApiError> {
    let mut file_bytes = 0_u64;
    let mut checksum = Sha256::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| ApiError::invalid_multipart("uploaded image could not be read"))?
    {
        let chunk_bytes = u64::try_from(chunk.len()).map_err(|_| {
            ApiError::upload_too_large(
                "file_too_large",
                "uploaded image exceeds the supported size",
                "images",
            )
        })?;
        file_bytes = file_bytes.checked_add(chunk_bytes).ok_or_else(|| {
            ApiError::upload_too_large(
                "file_too_large",
                "uploaded image exceeds the supported size",
                "images",
            )
        })?;
        if file_bytes > MAX_FILE_BYTES {
            return Err(ApiError::upload_too_large(
                "file_too_large",
                "each image must be 5 MiB or smaller",
                "images",
            ));
        }

        *total_bytes = total_bytes.checked_add(chunk_bytes).ok_or_else(|| {
            ApiError::upload_too_large(
                "upload_too_large",
                "combined image size exceeds the supported limit",
                "images",
            )
        })?;
        if storage_usage
            .checked_add(*total_bytes)
            .is_none_or(|projected| projected > storage_budget)
        {
            return Err(ApiError::resource_limit(
                "storage_budget_exceeded",
                "the storage budget does not have enough room for this upload",
            ));
        }
        if *total_bytes > MAX_TOTAL_FILE_BYTES {
            return Err(ApiError::upload_too_large(
                "upload_too_large",
                "combined image size must be 25 MiB or smaller",
                "images",
            ));
        }
        file.write_all(&chunk)
            .await
            .map_err(|error| ApiError::storage(taskharbor_adapters::StorageError::from(error)))?;
        checksum.update(&chunk);
    }

    if file_bytes == 0 {
        return Err(ApiError::upload_validation(
            "empty_file",
            "uploaded images cannot be empty",
            "images",
        ));
    }
    Ok((file_bytes, checksum.finalize().to_vec()))
}

async fn storage_usage(state: &AppState) -> Result<u64, ApiError> {
    let usage = state
        .storage
        .usage_bytes()
        .await
        .map_err(ApiError::storage)?;
    if usage >= state.config.storage_budget_bytes {
        return Err(ApiError::resource_limit(
            "storage_budget_exceeded",
            "the storage budget is full; wait for retention cleanup before uploading",
        ));
    }
    Ok(usage)
}

fn request_fingerprint(request: &StagedImageFields) -> Vec<u8> {
    let mut digest = Sha256::new();
    fingerprint_part(&mut digest, request.name.as_str().as_bytes());
    fingerprint_part(&mut digest, &request.max_width.to_be_bytes());
    fingerprint_part(&mut digest, &[request.jpeg_quality]);
    fingerprint_part(&mut digest, request.priority.as_str().as_bytes());
    match request.available_at {
        Some(available_at) => fingerprint_part(
            &mut digest,
            &available_at.unix_timestamp_nanos().to_be_bytes(),
        ),
        None => fingerprint_part(&mut digest, &[]),
    }
    for input in &request.inputs {
        fingerprint_part(&mut digest, input.display_name.as_bytes());
        fingerprint_part(&mut digest, input.media_type.as_bytes());
        fingerprint_part(&mut digest, &input.byte_size.to_be_bytes());
        fingerprint_part(&mut digest, &input.width.to_be_bytes());
        fingerprint_part(&mut digest, &input.height.to_be_bytes());
        fingerprint_part(&mut digest, &input.checksum_sha256);
    }
    digest.finalize().to_vec()
}

fn fingerprint_part(digest: &mut Sha256, value: &[u8]) {
    digest.update(u64::try_from(value.len()).unwrap_or(u64::MAX).to_be_bytes());
    digest.update(value);
}

async fn read_small_text(
    mut field: Field<'_>,
    field_name: &'static str,
) -> Result<String, ApiError> {
    let mut bytes = Vec::new();
    while let Some(chunk) = field
        .chunk()
        .await
        .map_err(|_| ApiError::invalid_multipart("multipart text field could not be read"))?
    {
        let next_len = bytes.len().checked_add(chunk.len()).ok_or_else(|| {
            ApiError::upload_validation(
                "invalid_field",
                "multipart text field is too long",
                field_name,
            )
        })?;
        if u64::try_from(next_len).unwrap_or(u64::MAX) > MAX_TEXT_FIELD_BYTES {
            return Err(ApiError::upload_validation(
                "invalid_field",
                "multipart text field is too long",
                field_name,
            ));
        }
        bytes.extend_from_slice(&chunk);
    }

    String::from_utf8(bytes).map_err(|_| {
        ApiError::upload_validation(
            "invalid_field",
            "multipart text field must be UTF-8",
            field_name,
        )
    })
}

fn set_once<T>(slot: &mut Option<T>, value: T, field: &'static str) -> Result<(), ApiError> {
    if slot.replace(value).is_some() {
        return Err(ApiError::upload_validation(
            "duplicate_field",
            format!("multipart field '{field}' can only appear once"),
            field,
        ));
    }
    Ok(())
}

fn safe_display_name(raw_name: &str, item_index: usize) -> String {
    let basename = raw_name
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or_default()
        .trim();
    let cleaned = basename
        .chars()
        .filter(|character| !character.is_control())
        .take(255)
        .collect::<String>();
    if cleaned.is_empty() {
        format!("image-{}", item_index + 1)
    } else {
        cleaned
    }
}

fn map_image_error(error: ImageError) -> ApiError {
    if error.is_invalid_input() {
        ApiError::upload_validation("invalid_image", error.safe_message(), "images")
    } else {
        ApiError::image_inspection(error)
    }
}
