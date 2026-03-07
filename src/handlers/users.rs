use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::IntoResponse,
};
use serde::Deserialize;

use crate::{
    auth::session::AuthenticatedAdmin,
    error::AppError,
    models::unified::{UnifiedUserDetail, UnifiedUserSummary},
    state::AppState,
};

// ── Search ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
}

#[derive(Template)]
#[template(path = "users_search.html")]
struct SearchTemplate {
    username: String,
    csrf_token: String,
    query: String,
    results: Vec<UnifiedUserSummary>,
}

pub async fn search(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<impl IntoResponse, AppError> {
    let query = params.q.unwrap_or_default();

    let results = if query.is_empty() {
        vec![]
    } else {
        state.users.search(&query).await?
    };

    Ok(SearchTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        query,
        results,
    })
}

// ── Detail ────────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "user_detail.html")]
struct DetailTemplate {
    username: String,
    csrf_token: String,
    user: UnifiedUserDetail,
    audit_logs: Vec<AuditEntry>,
}

struct AuditEntry {
    pub timestamp: String,
    pub action: String,
    pub result: String,
    pub admin_username: String,
}

pub async fn detail(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
) -> Result<impl IntoResponse, AppError> {
    let (user, audit_logs) = tokio::join!(
        state.users.get_detail(&keycloak_id),
        state.audit.for_user(&keycloak_id, 20),
    );

    let user = user?;
    let audit_logs = audit_logs
        .unwrap_or_default()
        .into_iter()
        .map(|l| AuditEntry {
            timestamp: l.timestamp,
            action: l.action,
            result: l.result,
            admin_username: l.admin_username,
        })
        .collect();

    Ok(DetailTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        user,
        audit_logs,
    })
}
