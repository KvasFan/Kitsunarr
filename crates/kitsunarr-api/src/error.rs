//! API error type and its JSON representation.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use kitsunarr_db::DbError;
use kitsunarr_domain::api::error::{ApiErrorBody, FieldError};

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    BadRequest(String),
    #[error("validation failed")]
    Validation(Vec<FieldError>),
    #[error("authentication required")]
    Unauthorized,
    #[error("{0}")]
    Forbidden(String),
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error("{0}")]
    PayloadTooLarge(String),
    #[error("{0}")]
    BadGateway(String),
    #[error("internal error")]
    Internal(String),
}

pub type ApiResult<T> = Result<T, ApiError>;

impl ApiError {
    pub fn internal(err: impl std::fmt::Display) -> Self {
        ApiError::Internal(err.to_string())
    }

    pub fn field(field: &str, message: impl Into<String>) -> Self {
        ApiError::Validation(vec![FieldError {
            field: field.into(),
            message: message.into(),
        }])
    }

    fn parts(&self) -> (StatusCode, &'static str) {
        match self {
            ApiError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            ApiError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            ApiError::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "validation_failed"),
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized"),
            ApiError::Forbidden(_) => (StatusCode::FORBIDDEN, "forbidden"),
            ApiError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            ApiError::TooManyRequests(_) => (StatusCode::TOO_MANY_REQUESTS, "too_many_requests"),
            ApiError::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "payload_too_large"),
            ApiError::BadGateway(_) => (StatusCode::BAD_GATEWAY, "upstream_error"),
            ApiError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "internal_error"),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.parts();
        let (message, fields) = match self {
            ApiError::Internal(detail) => {
                // Details go to the log only; clients get a generic message.
                tracing::error!(%detail, "internal error");
                (
                    "An internal error occurred. Check the logs for details.".to_string(),
                    Vec::new(),
                )
            }
            ApiError::Validation(fields) => {
                let message = fields
                    .first()
                    .map(|f| f.message.clone())
                    .unwrap_or_else(|| "Validation failed.".into());
                (message, fields)
            }
            ApiError::Unauthorized => ("Authentication required.".to_string(), Vec::new()),
            other => (other.to_string(), Vec::new()),
        };
        (
            status,
            Json(ApiErrorBody {
                code: code.into(),
                message,
                fields,
            }),
        )
            .into_response()
    }
}

impl From<DbError> for ApiError {
    fn from(err: DbError) -> Self {
        match err {
            DbError::NotFound => ApiError::NotFound("The requested record does not exist.".into()),
            DbError::Conflict(msg) => ApiError::Conflict(capitalize(&msg)),
            DbError::Invalid(msg) => ApiError::BadRequest(capitalize(&msg)),
            other => ApiError::Internal(other.to_string()),
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => {
            let mut out = f.to_uppercase().collect::<String>() + c.as_str();
            if !out.ends_with('.') {
                out.push('.');
            }
            out
        }
        None => String::new(),
    }
}
