use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),
    #[error("unauthorized")]
    Unauthorized,
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    TooManyRequests(String),
    #[error(transparent)]
    Database(#[from] sqlx::Error),
    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, kind, message) = match self {
            AppError::BadRequest(message) => {
                (StatusCode::BAD_REQUEST, "invalid_request_error", message)
            }
            AppError::Unauthorized => (
                StatusCode::UNAUTHORIZED,
                "authentication_error",
                "unauthorized".to_string(),
            ),
            AppError::NotFound(message) => (StatusCode::NOT_FOUND, "not_found", message),
            AppError::TooManyRequests(message) => {
                (StatusCode::TOO_MANY_REQUESTS, "rate_limit_error", message)
            }
            AppError::Database(error) => {
                tracing::error!(error = %error, "database error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "internal server error".to_string(),
                )
            }
            AppError::Internal(error) => {
                tracing::error!(error = %error, "internal error");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "api_error",
                    "internal server error".to_string(),
                )
            }
        };

        (
            status,
            Json(serde_json::json!({
                "error": {
                    "message": message,
                    "type": kind,
                }
            })),
        )
            .into_response()
    }
}
