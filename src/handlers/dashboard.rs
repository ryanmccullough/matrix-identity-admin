use askama::Template;
use axum::{
    extract::{Query, State},
    http::header,
    response::{Html, IntoResponse},
};
use serde::Deserialize;

use crate::{auth::session::AuthenticatedAdmin, error::AppError, state::AppState};

const SECS_24H: i64 = 86400;
const SECS_7D: i64 = 86400 * 7;
const SECS_30D: i64 = 86400 * 30;

const INVITE_ACTIONS: &[&str] = &["invite_user"];

const LIFECYCLE_ACTIONS: &[&str] = &[
    "disable_identity_account_on_disable",
    "disable_identity_account_on_offboard",
    "reactivate_auth_account_on_reactivate",
    "delete_keycloak_user",
];

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
    invites_24h: i64,
    invites_7d: i64,
    invites_30d: i64,
    lifecycle_24h: i64,
    lifecycle_7d: i64,
    lifecycle_30d: i64,
    failures_24h: i64,
    failures_7d: i64,
    failures_30d: i64,
    recent_actions: Vec<RecentAction>,
    notice: Option<String>,
    error: Option<String>,
    synapse_enabled: bool,
    templates: Vec<crate::models::onboarding_template::OnboardingTemplate>,
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
    let (
        total_users_res,
        recent_logs_res,
        inv_24h,
        inv_7d,
        inv_30d,
        lc_24h,
        lc_7d,
        lc_30d,
        fail_24h,
        fail_7d,
        fail_30d,
    ) = tokio::join!(
        state.keycloak.count_users(""),
        state.audit.recent(5),
        state.audit.count_actions_since(INVITE_ACTIONS, SECS_24H),
        state.audit.count_actions_since(INVITE_ACTIONS, SECS_7D),
        state.audit.count_actions_since(INVITE_ACTIONS, SECS_30D),
        state.audit.count_actions_since(LIFECYCLE_ACTIONS, SECS_24H),
        state.audit.count_actions_since(LIFECYCLE_ACTIONS, SECS_7D),
        state.audit.count_actions_since(LIFECYCLE_ACTIONS, SECS_30D),
        state.audit.count_failures_since(SECS_24H),
        state.audit.count_failures_since(SECS_7D),
        state.audit.count_failures_since(SECS_30D),
    );

    let total_users = total_users_res.unwrap_or(0);
    let logs = recent_logs_res?;
    let invites_24h = inv_24h.unwrap_or(0);
    let invites_7d = inv_7d.unwrap_or(0);
    let invites_30d = inv_30d.unwrap_or(0);
    let lifecycle_24h = lc_24h.unwrap_or(0);
    let lifecycle_7d = lc_7d.unwrap_or(0);
    let lifecycle_30d = lc_30d.unwrap_or(0);
    let failures_24h = fail_24h.unwrap_or(0);
    let failures_7d = fail_7d.unwrap_or(0);
    let failures_30d = fail_30d.unwrap_or(0);

    let recent_actions = logs
        .into_iter()
        .map(|l| RecentAction {
            timestamp: l.timestamp,
            admin_username: l.admin_username,
            action: l.action,
            result: l.result,
        })
        .collect();

    let templates =
        crate::models::onboarding_template::load_templates(&state.config.templates_path())
            .unwrap_or_default();

    let html = DashboardTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        total_users,
        invites_24h,
        invites_7d,
        invites_30d,
        lifecycle_24h,
        lifecycle_7d,
        lifecycle_30d,
        failures_24h,
        failures_7d,
        failures_30d,
        recent_actions,
        notice: query.notice,
        error: query.error,
        synapse_enabled: state.synapse.is_some(),
        templates,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

    Ok(Html(html))
}

#[derive(Template)]
#[template(path = "status_card.html")]
struct StatusCardTemplate {
    keycloak_ok: bool,
    mas_ok: bool,
    synapse_configured: bool,
    user_count: Option<u32>,
    group_count: Option<usize>,
    role_count: Option<usize>,
    room_count: Option<i64>,
}

/// GET /status
///
/// Returns an HTML fragment with the current system status.
/// Intended to be loaded via HTMX on the dashboard — runs health checks
/// against each configured upstream.
pub async fn status(
    AuthenticatedAdmin(_admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<impl IntoResponse, AppError> {
    let synapse_configured = state.synapse.is_some();

    let (kc_result, mas_result, groups_result, roles_result) = tokio::join!(
        state.keycloak.count_users(""),
        state.mas.get_user_by_username("__status_check__"),
        state.keycloak.list_groups(),
        state.keycloak.list_realm_roles(),
    );

    let (keycloak_ok, user_count) = match kc_result {
        Ok(n) => (true, Some(n)),
        Err(_) => (false, None),
    };
    let mas_ok = mas_result.is_ok();
    let group_count = groups_result.ok().map(|g| g.len());
    let role_count = roles_result.ok().map(|r| r.len());

    let room_count = if let Some(ref synapse) = state.synapse {
        synapse
            .list_rooms(1, None)
            .await
            .ok()
            .and_then(|r| r.total_rooms)
    } else {
        None
    };

    let tmpl = StatusCardTemplate {
        keycloak_ok,
        mas_ok,
        synapse_configured,
        user_count,
        group_count,
        role_count,
        room_count,
    };
    let html = tmpl
        .render()
        .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;
    Ok(([(header::CONTENT_TYPE, "text/html; charset=utf-8")], html))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
        routing::get,
        Router,
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::{
        models::audit::AuditResult,
        test_helpers::{
            build_test_state, dashboard_router, make_auth_cookie, MockKeycloak, TEST_CSRF,
        },
    };

    fn status_router(state: crate::state::AppState) -> Router {
        Router::new()
            .route("/status", get(super::status))
            .with_state(state)
    }

    async fn get_dashboard(
        state: crate::state::AppState,
        cookie: Option<String>,
        query: &str,
    ) -> axum::response::Response {
        let uri = if query.is_empty() {
            "/".to_string()
        } else {
            format!("/?{query}")
        };
        let mut builder = Request::builder().method(Method::GET).uri(uri);
        if let Some(c) = cookie {
            builder = builder.header("cookie", c);
        }
        let req = builder.body(Body::empty()).unwrap();
        dashboard_router(state).oneshot(req).await.unwrap()
    }

    async fn body_text(resp: axum::response::Response) -> String {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    #[tokio::test]
    async fn dashboard_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_dashboard(state, None, "").await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn dashboard_authenticated_returns_200() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_dashboard(state, Some(make_auth_cookie(TEST_CSRF)), "").await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn dashboard_shows_user_count() {
        let state = build_test_state(
            MockKeycloak {
                user_count: 42,
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let resp = get_dashboard(state, Some(make_auth_cookie(TEST_CSRF)), "").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(body.contains("42"), "expected user count '42' in body");
    }

    #[tokio::test]
    async fn dashboard_notice_query_param_appears_in_body() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_dashboard(
            state,
            Some(make_auth_cookie(TEST_CSRF)),
            "notice=Invite+sent",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(
            body.contains("Invite sent"),
            "expected notice message in body"
        );
    }

    #[tokio::test]
    async fn dashboard_shows_recent_audit_actions() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        state
            .audit
            .log(
                "sub",
                "admin",
                Some("kc-id"),
                Some("@u:t.com"),
                "test_action",
                AuditResult::Success,
                serde_json::json!({}),
            )
            .await
            .unwrap();

        let resp = get_dashboard(state, Some(make_auth_cookie(TEST_CSRF)), "").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(
            body.contains("test_action"),
            "expected 'test_action' in dashboard body"
        );
    }

    #[tokio::test]
    async fn dashboard_error_query_param_appears_in_body() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_dashboard(
            state,
            Some(make_auth_cookie(TEST_CSRF)),
            "error=Something+went+wrong",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(
            body.contains("Something went wrong"),
            "expected error message in body"
        );
    }

    // ── Status handler tests ──────────────────────────────────────────────────

    #[tokio::test]
    async fn status_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = status_router(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/status")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn status_authenticated_returns_html_fragment() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = status_router(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/status")
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(ct.contains("text/html"));
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(html.contains("status-grid"));
    }

    #[tokio::test]
    async fn status_shows_keycloak_user_count() {
        let state = build_test_state(
            MockKeycloak {
                user_count: 7,
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = status_router(state)
            .oneshot(
                Request::builder()
                    .method(Method::GET)
                    .uri("/status")
                    .header("cookie", cookie)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("7"),
            "expected user count '7' in status fragment"
        );
    }
}
