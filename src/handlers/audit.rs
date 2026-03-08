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
    action: Option<String>,
    result: Option<String>,
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
    action_filter: String,
    result_filter: String,
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
    let action_filter = query.action.unwrap_or_default();
    let result_filter = query.result.unwrap_or_default();

    let action_opt = if action_filter.is_empty() {
        None
    } else {
        Some(action_filter.as_str())
    };
    let result_opt = if result_filter.is_empty() {
        None
    } else {
        Some(result_filter.as_str())
    };

    let total = state.audit.count_filtered(action_opt, result_opt).await?;
    let total_pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = query.page.max(1).min(total_pages);
    let offset = (page - 1) * PAGE_SIZE;

    let logs = state
        .audit
        .recent_page_filtered(PAGE_SIZE, offset, action_opt, result_opt)
        .await?;

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
        action_filter,
        result_filter,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

    Ok(Html(html))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::test_helpers::{
        audit_router, build_test_state, make_auth_cookie, MockKeycloak, TEST_CSRF,
    };

    async fn get_audit(
        state: crate::state::AppState,
        cookie: Option<String>,
        query: &str,
    ) -> axum::response::Response {
        let uri = if query.is_empty() {
            "/audit".to_string()
        } else {
            format!("/audit?{query}")
        };
        let mut builder = Request::builder().method(Method::GET).uri(uri);
        if let Some(c) = cookie {
            builder = builder.header("cookie", c);
        }
        let req = builder.body(Body::empty()).unwrap();
        audit_router(state).oneshot(req).await.unwrap()
    }

    async fn body_text(resp: axum::response::Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn audit_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(state, None, "").await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn audit_authenticated_empty_db_returns_200() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(state, Some(make_auth_cookie(TEST_CSRF)), "").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn audit_filter_params_accepted() {
        // Verify filter query params don't cause a crash or non-200 on an empty DB.
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(
            state,
            Some(make_auth_cookie(TEST_CSRF)),
            "action=invite_user&result=success",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn audit_page_out_of_range_clamped_to_one() {
        // With an empty DB total_pages=1; page=999 should clamp to 1 and still return 200.
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(state, Some(make_auth_cookie(TEST_CSRF)), "page=999").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        // Template renders current page — should show "1" not "999".
        assert!(
            body.contains("Page 1"),
            "expected page to be clamped to 1 in body"
        );
    }
}
