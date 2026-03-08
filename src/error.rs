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

        // Askama validates templates at compile time; render() is effectively
        // infallible for correct templates. The fallback exists as a safety net.
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

#[cfg(test)]
mod tests {
    use axum::{http::StatusCode, response::IntoResponse};

    use super::AppError;

    fn status(err: AppError) -> StatusCode {
        err.into_response().status()
    }

    #[test]
    fn auth_error_returns_401() {
        assert_eq!(
            status(AppError::Auth("not logged in".into())),
            StatusCode::UNAUTHORIZED
        );
    }

    #[test]
    fn not_found_error_returns_404() {
        assert_eq!(
            status(AppError::NotFound("missing".into())),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn validation_error_returns_400() {
        assert_eq!(
            status(AppError::Validation("bad input".into())),
            StatusCode::BAD_REQUEST
        );
    }

    #[test]
    fn upstream_error_returns_502() {
        assert_eq!(
            status(AppError::Upstream {
                service: "keycloak".into(),
                message: "timeout".into(),
            }),
            StatusCode::BAD_GATEWAY,
        );
    }

    #[test]
    fn database_error_returns_500() {
        // Construct a sqlx error via query decoding failure (no DB needed).
        let sqlx_err = sqlx::Error::RowNotFound;
        assert_eq!(
            status(AppError::Database(sqlx_err)),
            StatusCode::INTERNAL_SERVER_ERROR
        );
    }

    #[test]
    fn internal_error_returns_500() {
        assert_eq!(
            status(AppError::Internal(anyhow::anyhow!("oops"))),
            StatusCode::INTERNAL_SERVER_ERROR,
        );
    }

    #[tokio::test]
    async fn upstream_error_fn_without_status_uses_request_failed() {
        // Connect to a port that is not listening to get a connection-refused error.
        let err = reqwest::get("http://127.0.0.1:1").await.unwrap_err();
        let app_err = super::upstream_error("testsvc", err);
        match app_err {
            AppError::Upstream { service, message } => {
                assert_eq!(service, "testsvc");
                assert_eq!(message, "request failed");
            }
            other => panic!("expected Upstream, got {other:?}"),
        }
    }
}
