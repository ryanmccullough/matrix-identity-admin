use askama::Template;
use axum::{extract::State, response::Html};

use crate::{auth::session::AuthenticatedAdmin, error::AppError, state::AppState};

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    username: String,
    csrf_token: String,
    recent_actions: Vec<RecentAction>,
}

struct RecentAction {
    timestamp: String,
    admin_username: String,
    action: String,
    result: String,
}

pub async fn dashboard(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    let logs = state.audit.recent(10).await?;

    let recent_actions = logs
        .into_iter()
        .map(|l| RecentAction {
            timestamp: l.timestamp,
            admin_username: l.admin_username,
            action: l.action,
            result: l.result,
        })
        .collect();

    let html = DashboardTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        recent_actions,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

    Ok(Html(html))
}
