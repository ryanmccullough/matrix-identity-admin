use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::Deserialize;

use crate::{auth::session::AuthenticatedAdmin, error::AppError, state::AppState};

const PAGE_SIZE: i64 = 50;

#[derive(Deserialize)]
pub struct AuditQuery {
    #[serde(default = "default_page")]
    page: i64,
}

fn default_page() -> i64 {
    1
}

#[derive(Template)]
#[template(path = "audit.html")]
struct AuditTemplate {
    username: String,
    csrf_token: String,
    logs: Vec<AuditRow>,
    page: i64,
    total_pages: i64,
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
    Query(query): Query<AuditQuery>,
) -> Result<Html<String>, AppError> {
    let total = state.audit.count().await?;
    let total_pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = query.page.max(1).min(total_pages);
    let offset = (page - 1) * PAGE_SIZE;

    let logs = state.audit.recent_page(PAGE_SIZE, offset).await?;

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
        page,
        total_pages,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

    Ok(Html(html))
}
