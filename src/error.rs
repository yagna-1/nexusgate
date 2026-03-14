use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Budget exceeded: {0}")]
    BudgetExceeded(String),

    #[error("No providers available: {0}")]
    NoProviders(String),

    #[error("Provider error ({provider}): {message}")]
    ProviderError { provider: String, message: String },

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Rate limited by all providers")]
    AllProvidersRateLimited,

    #[error("Internal error: {0}")]
    Internal(String),

    #[error(transparent)]
    Anyhow(#[from] anyhow::Error),

    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),

    #[error(transparent)]
    Redis(#[from] redis::RedisError),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::Unauthorized(msg) => {
                (StatusCode::UNAUTHORIZED, "invalid_api_key", msg.clone())
            }
            AppError::BudgetExceeded(msg) => {
                (StatusCode::PAYMENT_REQUIRED, "budget_exceeded", msg.clone())
            }
            AppError::NoProviders(msg) => (
                StatusCode::SERVICE_UNAVAILABLE,
                "no_providers_available",
                msg.clone(),
            ),
            AppError::ProviderError { message, .. } => {
                (StatusCode::BAD_GATEWAY, "provider_error", message.clone())
            }
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, "invalid_request", msg.clone()),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, "not_found", msg.clone()),
            AppError::AllProvidersRateLimited => (
                StatusCode::TOO_MANY_REQUESTS,
                "rate_limited",
                "All providers are currently rate limited. Retry after a moment.".into(),
            ),
            AppError::Internal(msg) => {
                tracing::error!("Internal error: {}", msg);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "An internal error occurred".into(),
                )
            }
            other => {
                tracing::error!("Unhandled error: {:?}", other);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal_error",
                    "An internal error occurred".into(),
                )
            }
        };

        // OpenAI-compatible error format
        (
            status,
            Json(json!({
                "error": {
                    "message": message,
                    "type": code,
                    "code": code,
                }
            })),
        )
            .into_response()
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
