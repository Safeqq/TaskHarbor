use axum::extract::Multipart;
use axum::extract::multipart::{Field, MultipartRejection};
use taskharbor_adapters::{
    DEFAULT_JPEG_QUALITY, DEFAULT_OUTPUT_WIDTH, ImageError, MAX_FILE_BYTES, MAX_FILES_PER_JOB,
    MAX_OUTPUT_WIDTH, MAX_TOTAL_FILE_BYTES, NewImageJob, NewInputArtifact,
};
use taskharbor_core::JobName;
use tokio::io::AsyncWriteExt;

use crate::AppState;
use crate::error::ApiError;

const MAX_TEXT_FIELD_BYTES: u64 = 512;

pub async fn create_image_job(
    state: &AppState,
    multipart: Result<Multipart, MultipartRejection>,
) -> Result<taskharbor_adapters::JobRecord, ApiError> {
    let multipart = multipart
        .map_err(|_| ApiError::invalid_multipart("request must be valid multipart form data"))?;
    let batch = state
        .storage
        .begin_upload()
        .await
        .map_err(ApiError::storage)?;

    let staged = stage_fields(state, &batch, multipart).await;
    let request = match staged {
        Ok(request) => request,
        Err(error) => {
            let _ = batch.cleanup().await;
            return Err(error);
        }
    };

    // A database commit can have an ambiguous outcome if the connection drops. Keep the staged
    // inputs so a job that did commit never points at files we deleted.
    state
        .jobs
        .create_image_job(request)
        .await
        .map_err(ApiError::repository)
}

async fn stage_fields(
    state: &AppState,
    batch: &taskharbor_adapters::UploadBatch,
    mut multipart: Multipart,
) -> Result<NewImageJob, ApiError> {
    let mut name = None;
    let mut max_width = None;
    let mut jpeg_quality = None;
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
                let byte_size = write_image_field(&mut field, &mut file, &mut total_bytes).await?;
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

    Ok(NewImageJob {
        name,
        max_width: max_width.unwrap_or(DEFAULT_OUTPUT_WIDTH),
        jpeg_quality: jpeg_quality.unwrap_or(DEFAULT_JPEG_QUALITY),
        inputs,
    })
}

async fn write_image_field(
    field: &mut Field<'_>,
    file: &mut tokio::fs::File,
    total_bytes: &mut u64,
) -> Result<u64, ApiError> {
    let mut file_bytes = 0_u64;
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
    }

    if file_bytes == 0 {
        return Err(ApiError::upload_validation(
            "empty_file",
            "uploaded images cannot be empty",
            "images",
        ));
    }
    Ok(file_bytes)
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
