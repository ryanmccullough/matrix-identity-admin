use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::Deserialize;

use crate::{auth::session::AuthenticatedAdmin, error::AppError, state::AppState};

#[derive(Deserialize)]
pub struct DashboardQuery {
    pub notice: Option<String>,
    pub error: Option<String>,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
struct DashboardTemplate {
    username: String,
    csrf_token: String,
    total_users: u32,
    actions_24h: i64,
    recent_actions: Vec<RecentAction>,
    notice: Option<String>,
    error: Option<String>,
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
    Query(query): Query<DashboardQuery>,
) -> Result<Html<String>, AppError> {
    let (total_users_res, recent_logs_res, actions_24h_res) = tokio::join!(
        state.keycloak.count_users(""),
        state.audit.recent(5),
        state.audit.recent_actions_count(86400),
    );

    let total_users = total_users_res.unwrap_or(0);
    let logs = recent_logs_res?;
    let actions_24h = actions_24h_res?;

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
        total_users,
        actions_24h,
        recent_actions,
        notice: query.notice,
        error: query.error,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

    Ok(Html(html))
}
