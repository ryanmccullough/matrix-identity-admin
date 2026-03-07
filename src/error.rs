use askama::Template;
use axum::{
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Authentication error: {0}")]
    Auth(String),

    #[error("Upstream API error from {service}: {message}")]
    Upstream { service: String, message: String },

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[derive(Template)]
#[template(path = "error.html")]
struct ErrorTemplate {
    status: u16,
    message: String,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        tracing::error!(error = %self, "Request failed");

        let (status, user_message) = match &self {
            AppError::Auth(_) => (
                StatusCode::UNAUTHORIZED,
                "Authentication required.".to_string(),
            ),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::Validation(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Upstream { service, .. } => (
                StatusCode::BAD_GATEWAY,
                format!("An upstream service ({service}) is unavailable or returned an error."),
            ),
            AppError::Database(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "A database error occurred.".to_string(),
            ),
            AppError::Internal(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "An internal error occurred.".to_string(),
            ),
        };

        let tmpl = ErrorTemplate {
            status: status.as_u16(),
            message: user_message,
        };

        let html = tmpl.render().unwrap_or_else(|_| {
            format!(
                "<html><body><h1>Error {}</h1></body></html>",
                status.as_u16()
            )
        });

        (status, Html(html)).into_response()
    }
}

/// Convert a reqwest error into an upstream AppError.
pub fn upstream_error(service: &str, err: reqwest::Error) -> AppError {
    AppError::Upstream {
        service: service.to_string(),
        // Avoid leaking internal URLs or tokens in the error message
        message: err
            .status()
            .map(|s| format!("HTTP {s}"))
            .unwrap_or_else(|| "request failed".to_string()),
    }
}
