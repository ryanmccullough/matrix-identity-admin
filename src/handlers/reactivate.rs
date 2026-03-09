use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;

use crate::{
    auth::{csrf::validate, session::AuthenticatedAdmin},
    error::AppError,
    services::reactivate_user::reactivate_user,
    state::AppState,
    utils::pct_encode,
};

#[derive(Deserialize)]
pub struct ReactivateForm {
    pub _csrf: String,
}

/// POST /users/{id}/reactivate
///
/// Enables the Keycloak account and reactivates the MAS account if
/// deactivated. Both operations are audit-logged. On success, redirects
/// back to the user detail page with a notice.
pub async fn reactivate(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
    Form(form): Form<ReactivateForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let outcome = reactivate_user(
        &keycloak_id,
        state.keycloak.as_ref(),
        state.mas.as_ref(),
        &state.audit,
        &admin.subject,
        &admin.username,
        &state.config.homeserver_domain,
    )
    .await?;

    let redirect = if outcome.has_warnings() {
        let mut warning = pct_encode(&outcome.warning_summary());
        if warning.len() > 400 {
            warning.truncate(400);
            warning.push_str("%E2%80%A6");
        }
        format!("/users/{keycloak_id}?warning={warning}")
    } else {
        format!("/users/{keycloak_id}?notice=User+reactivated")
    };

    Ok(Redirect::to(&redirect))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::{
        models::{keycloak::KeycloakUser, mas::MasUser},
        test_helpers::{
            build_test_state_full, make_auth_cookie, mutations_router, MockKeycloak, MockMas,
            TEST_CSRF,
        },
    };

    fn test_kc_user() -> KeycloakUser {
        KeycloakUser {
            id: "kc-123".to_string(),
            username: "testuser".to_string(),
            email: Some("test@example.com".to_string()),
            first_name: None,
            last_name: None,
            enabled: false,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    async fn post_reactivate(
        state: crate::state::AppState,
        user_id: &str,
        csrf: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = format!("_csrf={csrf}");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("/users/{user_id}/reactivate"))
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(cookie) = auth_cookie {
            builder = builder.header("cookie", cookie);
        }
        mutations_router(state)
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reactivate_success_redirects_with_notice() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reactivate(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(
            resp.headers().get("location").unwrap(),
            "/users/kc-123?notice=User+reactivated"
        );
    }

    #[tokio::test]
    async fn reactivate_unauthenticated_redirects_to_login() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let resp = post_reactivate(state, "kc-123", TEST_CSRF, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn reactivate_invalid_csrf_returns_400() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reactivate(state, "kc-123", "wrong-csrf", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn reactivate_keycloak_failure_returns_502() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                fail_enable: true,
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reactivate(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn reactivate_user_not_found_returns_404() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reactivate(state, "nonexistent", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reactivate_with_mas_account_writes_audit_logs() {
        let deactivated_mas_user = MasUser {
            id: "mas-456".to_string(),
            username: "testuser".to_string(),
            deactivated_at: Some("2026-01-01T00:00:00Z".to_string()),
        };
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas {
                user: Some(deactivated_mas_user),
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let audit = std::sync::Arc::clone(&state.audit);
        let cookie = make_auth_cookie(TEST_CSRF);
        post_reactivate(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        let logs = audit.for_user("kc-123", 10).await.unwrap();
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"enable_identity_account_on_reactivate"));
        assert!(actions.contains(&"reactivate_auth_account_on_reactivate"));
        assert!(logs.iter().all(|l| l.result == "success"));
    }
}
