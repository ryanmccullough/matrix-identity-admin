use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    auth::{csrf::validate, session::AuthenticatedAdmin},
    error::AppError,
    models::audit::AuditResult,
    state::AppState,
};

#[derive(Deserialize)]
pub struct RevokeForm {
    pub _csrf: String,
    /// "compat" or "oauth2" — determines which MAS endpoint to call.
    pub session_type: String,
}

pub async fn revoke(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path((keycloak_id, session_id)): Path<(String, String)>,
    Form(form): Form<RevokeForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let result = state
        .mas
        .finish_session(&session_id, &form.session_type)
        .await;

    let audit_result = match &result {
        Ok(_) => AuditResult::Success,
        Err(_) => AuditResult::Failure,
    };

    state
        .audit
        .log(
            &admin.subject,
            &admin.username,
            Some(&keycloak_id),
            None,
            "finish_mas_session",
            audit_result,
            json!({
                "session_id": session_id,
                "session_type": form.session_type,
            }),
        )
        .await?;

    result?;

    Ok(Redirect::to(&format!("/users/{keycloak_id}")))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::test_helpers::{
        build_test_state_full, make_auth_cookie, mutations_router, MockKeycloak, MockMas, TEST_CSRF,
    };

    async fn post_revoke(
        state: crate::state::AppState,
        user_id: &str,
        session_id: &str,
        csrf: &str,
        session_type: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = format!("_csrf={csrf}&session_type={session_type}");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("/users/{user_id}/sessions/{session_id}/revoke"))
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(cookie) = auth_cookie {
            builder = builder.header("cookie", cookie);
        }
        mutations_router(state)
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn revoke_success_redirects_to_user_page() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_revoke(
            state,
            "kc-123",
            "sess-1",
            TEST_CSRF,
            "compat",
            Some(&cookie),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/users/kc-123");
    }

    #[tokio::test]
    async fn revoke_invalid_csrf_returns_400() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_revoke(
            state,
            "kc-123",
            "sess-1",
            "wrong-csrf",
            "compat",
            Some(&cookie),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn revoke_unauthenticated_redirects_to_login() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let resp = post_revoke(state, "kc-123", "sess-1", TEST_CSRF, "compat", None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn revoke_mas_failure_returns_502() {
        let state = build_test_state_full(
            MockKeycloak::default(),
            MockMas {
                fail_finish_session: true,
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_revoke(
            state,
            "kc-123",
            "sess-1",
            TEST_CSRF,
            "compat",
            Some(&cookie),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn revoke_oauth2_session_type_succeeds() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_revoke(
            state,
            "kc-123",
            "sess-1",
            TEST_CSRF,
            "oauth2",
            Some(&cookie),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn revoke_success_writes_audit_log() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let audit = std::sync::Arc::clone(&state.audit);
        let cookie = make_auth_cookie(TEST_CSRF);
        post_revoke(state, "kc-123", "sess-1", TEST_CSRF, "compat", Some(&cookie)).await;
        let logs = audit.for_user("kc-123", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "finish_mas_session");
        assert_eq!(logs[0].result, "success");
    }
}
