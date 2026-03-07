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
pub struct ForceLogoutForm {
    pub _csrf: String,
}

pub async fn force_keycloak_logout(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
    Form(form): Form<ForceLogoutForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let result = state.keycloak.logout_user(&keycloak_id).await;

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
            "force_keycloak_logout",
            audit_result,
            json!({ "keycloak_user_id": keycloak_id }),
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

    async fn post_force_logout(
        state: crate::state::AppState,
        user_id: &str,
        csrf: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = format!("_csrf={csrf}");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("/users/{user_id}/keycloak/logout"))
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
    async fn force_logout_success_redirects_to_user_page() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_force_logout(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/users/kc-123");
    }

    #[tokio::test]
    async fn force_logout_invalid_csrf_returns_400() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_force_logout(state, "kc-123", "wrong-csrf", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn force_logout_unauthenticated_redirects_to_login() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let resp = post_force_logout(state, "kc-123", TEST_CSRF, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn force_logout_keycloak_failure_returns_502() {
        let state = build_test_state_full(
            MockKeycloak {
                fail_logout: true,
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_force_logout(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }
}
