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

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::test_helpers::{
        build_test_state, dashboard_router, make_auth_cookie, MockKeycloak, TEST_CSRF,
    };

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
        assert_eq!(
            resp.headers().get("location").unwrap(),
            "/auth/login"
        );
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
        let resp =
            get_dashboard(state, Some(make_auth_cookie(TEST_CSRF)), "notice=Invite+sent").await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = body_text(resp).await;
        assert!(
            body.contains("Invite sent"),
            "expected notice message in body"
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
}
