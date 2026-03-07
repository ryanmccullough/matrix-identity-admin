use askama::Template;
use axum::{extract::State, response::Html};

use crate::{auth::session::AuthenticatedAdmin, error::AppError, state::AppState};

#[derive(Template)]
#[template(path = "audit.html")]
struct AuditTemplate {
    username: String,
    csrf_token: String,
    logs: Vec<AuditRow>,
}

struct AuditRow {
    pub timestamp: String,
    pub admin_username: String,
    pub action: String,
    pub result: String,
    pub target_keycloak_user_id: Option<String>,
    pub target_matrix_user_id: Option<String>,
}

pub async fn list(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    let logs = state.audit.recent(100).await?;

    let rows = logs
        .into_iter()
        .map(|l| AuditRow {
            timestamp: l.timestamp,
            admin_username: l.admin_username,
            action: l.action,
            result: l.result,
            target_keycloak_user_id: l.target_keycloak_user_id,
            target_matrix_user_id: l.target_matrix_user_id,
        })
        .collect();

    let html = AuditTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        logs: rows,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

    Ok(Html(html))
}
