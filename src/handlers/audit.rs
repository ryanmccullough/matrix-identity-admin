use askama::Template;
use axum::{
    extract::{Query, State},
    response::Html,
};
use serde::Deserialize;

use crate::{auth::session::AuthenticatedAdmin, error::AppError, state::AppState};

const PAGE_SIZE: i64 = 50;
const AUDIT_ACTION_OPTIONS: &[&str] = &[
    "create_policy_binding",
    "deactivate_auth_account_on_offboard",
    "deactivate_mas_user",
    "delete_keycloak_user",
    "delete_policy_binding",
    "disable_identity_account_on_disable",
    "disable_identity_account_on_offboard",
    "enable_identity_account_on_reactivate",
    "finish_mas_session",
    "force_identity_logout_on_offboard",
    "force_keycloak_logout",
    "invite_user",
    "join_room_on_reconcile",
    "kick_room_on_offboard",
    "kick_room_on_reconcile",
    "reactivate_auth_account_on_reactivate",
    "reactivate_mas_user",
    "revoke_auth_session_on_disable",
    "revoke_auth_session_on_offboard",
    "update_policy_binding",
];
const AUDIT_RESULT_OPTIONS: &[&str] = &["success", "failure"];

#[derive(Deserialize)]
pub struct AuditQuery {
    #[serde(default = "default_page")]
    page: i64,
    action: Option<String>,
    result: Option<String>,
    admin: Option<String>,
    from: Option<String>,
    to: Option<String>,
}

fn default_page() -> i64 {
    1
}

fn validate_optional_filter(
    raw: Option<String>,
    name: &str,
    allowed: &[&str],
) -> Result<String, AppError> {
    let value = raw.unwrap_or_default();
    if value.is_empty() || allowed.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(AppError::Validation(format!(
            "Invalid audit {name} filter."
        )))
    }
}

#[derive(Template)]
#[template(path = "audit.html")]
struct AuditTemplate {
    username: String,
    csrf_token: String,
    logs: Vec<AuditRow>,
    page: i64,
    total_pages: i64,
    action_options: &'static [&'static str],
    action_filter: String,
    result_options: &'static [&'static str],
    result_filter: String,
    admin_filter: String,
    from_filter: String,
    to_filter: String,
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
    let action_filter = validate_optional_filter(query.action, "action", AUDIT_ACTION_OPTIONS)?;
    let result_filter = validate_optional_filter(query.result, "result", AUDIT_RESULT_OPTIONS)?;
    let admin_filter = query.admin.unwrap_or_default();
    let from_filter = query.from.unwrap_or_default();
    let to_filter = query.to.unwrap_or_default();

    if !from_filter.is_empty() {
        validate_date_format(&from_filter)?;
    }
    if !to_filter.is_empty() {
        validate_date_format(&to_filter)?;
    }

    let filter = crate::db::audit::AuditFilter {
        action: non_empty(&action_filter),
        result: non_empty(&result_filter),
        admin_username: non_empty(&admin_filter),
        from: non_empty(&from_filter),
        to: non_empty(&to_filter),
    };

    let total = state.audit.count_with_filter(&filter).await?;
    let total_pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = query.page.max(1).min(total_pages);
    let offset = (page - 1) * PAGE_SIZE;

    let logs = state
        .audit
        .page_with_filter(&filter, PAGE_SIZE, offset)
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
        action_options: AUDIT_ACTION_OPTIONS,
        action_filter,
        result_options: AUDIT_RESULT_OPTIONS,
        result_filter,
        admin_filter,
        from_filter,
        to_filter,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

    Ok(Html(html))
}

fn non_empty(s: &str) -> Option<&str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn validate_date_format(s: &str) -> Result<(), AppError> {
    if s.len() == 10
        && s.as_bytes()[4] == b'-'
        && s.as_bytes()[7] == b'-'
        && s[..4].chars().all(|c| c.is_ascii_digit())
        && s[5..7].chars().all(|c| c.is_ascii_digit())
        && s[8..10].chars().all(|c| c.is_ascii_digit())
    {
        Ok(())
    } else {
        Err(AppError::Validation(
            "Invalid date format, expected YYYY-MM-DD".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::{
        models::audit::AuditResult,
        test_helpers::{audit_router, build_test_state, make_auth_cookie, MockKeycloak, TEST_CSRF},
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
    async fn audit_invalid_action_filter_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(
            state,
            Some(make_auth_cookie(TEST_CSRF)),
            "action=drop_table",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn audit_invalid_result_filter_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(state, Some(make_auth_cookie(TEST_CSRF)), "result=maybe").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn audit_shows_entries_from_db() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        state
            .audit
            .log(
                "sub",
                "admin",
                Some("kc-id"),
                Some("@u:t.com"),
                "revoke_session",
                AuditResult::Success,
                serde_json::json!({}),
            )
            .await
            .unwrap();

        let resp = get_audit(state, Some(make_auth_cookie(TEST_CSRF)), "").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(
            body.contains("revoke_session"),
            "expected 'revoke_session' in audit body"
        );
    }

    // ── Canonical 3-test pattern ──────────────────────────────────────────────

    #[tokio::test]
    async fn audit_list_authenticated_returns_200() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(state, Some(make_auth_cookie(TEST_CSRF)), "").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn audit_list_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(state, None, "").await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn audit_list_shows_log_entries() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        state
            .audit
            .log(
                "sub",
                "testoperator",
                Some("kc-id"),
                Some("@u:t.com"),
                "invite_user",
                AuditResult::Success,
                serde_json::json!({}),
            )
            .await
            .unwrap();

        let resp = get_audit(state, Some(make_auth_cookie(TEST_CSRF)), "").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(
            body.contains("invite_user"),
            "expected 'invite_user' in audit list body"
        );
        assert!(
            body.contains("testoperator"),
            "expected admin username 'testoperator' in audit list body"
        );
    }

    // ── Date and admin filter tests ────────────────────────────────────────

    #[tokio::test]
    async fn audit_date_filter_accepted() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(
            state,
            Some(make_auth_cookie(TEST_CSRF)),
            "from=2024-01-01&to=2024-12-31",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn audit_invalid_date_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(state, Some(make_auth_cookie(TEST_CSRF)), "from=not-a-date").await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn audit_admin_filter_accepted() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_audit(state, Some(make_auth_cookie(TEST_CSRF)), "admin=testadmin").await;
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
