use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use taskharbor_adapters::{ImageError, RepositoryError, StorageError};
use taskharbor_core::JobNameError;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    field: Option<&'static str>,
}

impl ApiError {
    pub fn unauthorized() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "authentication_required",
            message: "sign in to access TaskHarbor".into(),
            field: None,
        }
    }

    pub fn invalid_credentials() -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "invalid_credentials",
            message: "username or password is incorrect".into(),
            field: None,
        }
    }

    pub fn invalid_csrf() -> Self {
        Self {
            status: StatusCode::FORBIDDEN,
            code: "invalid_csrf_token",
            message: "the request is missing a valid CSRF token".into(),
            field: None,
        }
    }

    pub fn rate_limited(retry_after: std::time::Duration) -> Self {
        Self {
            status: StatusCode::TOO_MANY_REQUESTS,
            code: "rate_limited",
            message: format!(
                "too many requests; try again in {} seconds",
                retry_after.as_secs().max(1)
            ),
            field: None,
        }
    }

    pub fn resource_limit(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::INSUFFICIENT_STORAGE,
            code,
            message: message.into(),
            field: None,
        }
    }

    pub fn internal() -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: "the request could not be completed".into(),
            field: None,
        }
    }

    pub fn invalid_job_name(error: JobNameError) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code: "validation_error",
            message: error.to_string(),
            field: Some("name"),
        }
    }

    pub fn invalid_multipart(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_multipart",
            message: message.into(),
            field: None,
        }
    }

    pub fn upload_validation(
        code: &'static str,
        message: impl Into<String>,
        field: &'static str,
    ) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            code,
            message: message.into(),
            field: Some(field),
        }
    }

    pub fn upload_too_large(
        code: &'static str,
        message: impl Into<String>,
        field: &'static str,
    ) -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            code,
            message: message.into(),
            field: Some(field),
        }
    }

    pub fn storage(_error: StorageError) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "storage_error",
            message: "file storage is temporarily unavailable".into(),
            field: None,
        }
    }

    pub fn image_inspection(_error: ImageError) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "image_inspection_error",
            message: "the uploaded image could not be inspected".into(),
            field: Some("images"),
        }
    }

    pub fn artifact_unavailable(_error: StorageError) -> Self {
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "artifact_unavailable",
            message: "the output file is temporarily unavailable".into(),
            field: None,
        }
    }

    pub fn artifact_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "artifact_not_found",
            message: "output artifact was not found".into(),
            field: None,
        }
    }

    pub fn job_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "job_not_found",
            message: "job was not found".into(),
            field: None,
        }
    }

    pub fn schedule_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "schedule_not_found",
            message: "schedule was not found".into(),
            field: None,
        }
    }

    pub fn repository(error: RepositoryError) -> Self {
        let category = match &error {
            RepositoryError::Database(_) => "database",
            RepositoryError::Migration(_) => "migration",
            RepositoryError::InvalidData(_) => "invalid_data",
            RepositoryError::StateConflict(_) => "state_conflict",
            RepositoryError::IdempotencyConflict => "idempotency_conflict",
            RepositoryError::ClaimLost => "claim_lost",
        };
        tracing::error!(
            event = "repository_error",
            category,
            "repository operation failed"
        );
        match error {
            RepositoryError::Database(_) | RepositoryError::Migration(_) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "database_unavailable",
                message: "the database is temporarily unavailable".into(),
                field: None,
            },
            RepositoryError::StateConflict(_) | RepositoryError::ClaimLost => Self {
                status: StatusCode::CONFLICT,
                code: "job_state_conflict",
                message: "the job is no longer in a state that allows this action".into(),
                field: None,
            },
            RepositoryError::IdempotencyConflict => Self {
                status: StatusCode::CONFLICT,
                code: "idempotency_conflict",
                message: "the idempotency key was already used for different job data".into(),
                field: None,
            },
            RepositoryError::InvalidData(_) => Self {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "internal_error",
                message: "the request could not be completed".into(),
                field: None,
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorEnvelope {
            error: ErrorBody {
                code: self.code,
                message: self.message,
                field: self.field,
            },
        };

        (self.status, Json(body)).into_response()
    }
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    field: Option<&'static str>,
}
