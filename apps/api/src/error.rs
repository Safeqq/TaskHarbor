use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use taskharbor_adapters::RepositoryError;
use taskharbor_core::JobNameError;

#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
    field: Option<&'static str>,
}

impl ApiError {
    pub fn invalid_json() -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "invalid_json",
            message: "request body must be valid JSON with a string field named 'name'".into(),
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

    pub fn job_not_found() -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "job_not_found",
            message: "job was not found".into(),
            field: None,
        }
    }

    pub fn repository(error: RepositoryError) -> Self {
        match error {
            RepositoryError::Database(_) | RepositoryError::Migration(_) => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "database_unavailable",
                message: "the database is temporarily unavailable".into(),
                field: None,
            },
            RepositoryError::InvalidData(_) | RepositoryError::StateConflict(_) => Self {
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
