use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;

use crate::{
    auth::{csrf::validate, session::AuthenticatedAdmin},
    error::AppError,
    services::delete_user::{delete_user, DeleteUserResult},
    state::AppState,
    utils::pct_encode,
};

#[derive(Deserialize)]
pub struct DeleteUserForm {
    pub _csrf: String,
}

/// POST /users/{id}/delete
///
/// Deactivates the MAS account then deletes the Keycloak user. MAS is
/// attempted first so that a failure leaves the Keycloak record intact
/// and the admin can retry. Both operations are audit-logged. On success,
/// redirects to the user search page.
pub async fn delete_user_handler(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
    Form(form): Form<DeleteUserForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let result = delete_user(
        &keycloak_id,
        state.keycloak.as_ref(),
        state.mas.as_ref(),
        &state.audit,
        &admin.subject,
        &admin.username,
        &state.config.homeserver_domain,
    )
    .await?;

    let redirect = match result {
        DeleteUserResult::Deleted(outcome) => {
            if outcome.has_warnings() {
                let mut warning = pct_encode(&outcome.warning_summary());
                if warning.len() > 400 {
                    warning.truncate(400);
                    warning.push_str("%E2%80%A6");
                }
                format!("/users/search?warning={warning}")
            } else {
                "/users/search".to_string()
            }
        }
        DeleteUserResult::PartialFailure(outcome) => {
            let mut warning = pct_encode(&outcome.warning_summary());
            if warning.len() > 400 {
                warning.truncate(400);
                warning.push_str("%E2%80%A6");
            }
            format!("/users/{keycloak_id}?warning={warning}")
        }
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
            enabled: true,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    fn test_mas_user() -> MasUser {
        MasUser {
            id: "mas-456".to_string(),
            username: "testuser".to_string(),
            deactivated_at: None,
        }
    }

    async fn post_delete(
        state: crate::state::AppState,
        user_id: &str,
        csrf: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = format!("_csrf={csrf}");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("/users/{user_id}/delete"))
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
    async fn delete_with_mas_account_redirects_to_search() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas {
                user: Some(test_mas_user()),
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_delete(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/users/search");
    }

    #[tokio::test]
    async fn delete_without_mas_account_redirects_to_search() {
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
        let resp = post_delete(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/users/search");
    }

    #[tokio::test]
    async fn delete_keycloak_user_not_found_returns_404() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_delete(state, "nonexistent", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delete_invalid_csrf_returns_400() {
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
        let resp = post_delete(state, "kc-123", "wrong-csrf", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_unauthenticated_redirects_to_login() {
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
        let resp = post_delete(state, "kc-123", TEST_CSRF, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn delete_mas_failure_aborts_before_keycloak_returns_502() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas {
                user: Some(test_mas_user()),
                fail_deactivate_user: true,
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_delete(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn delete_keycloak_failure_without_mas_account_returns_502() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                fail_delete: true,
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_delete(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn delete_keycloak_failure_after_mas_deactivation_redirects_with_warning() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                fail_delete: true,
                ..Default::default()
            },
            MockMas {
                user: Some(test_mas_user()),
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_delete(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("/users/kc-123?warning="));
    }

    #[tokio::test]
    async fn delete_mas_lookup_failure_still_deletes_keycloak_user_with_warning() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas {
                fail_get_user_by_username: true,
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_delete(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("/users/search?warning="));
    }

    #[tokio::test]
    async fn delete_success_writes_audit_logs() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockMas {
                user: Some(test_mas_user()),
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let audit = std::sync::Arc::clone(&state.audit);
        let cookie = make_auth_cookie(TEST_CSRF);
        post_delete(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        let logs = audit.for_user("kc-123", 10).await.unwrap();
        assert_eq!(logs.len(), 2);
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"deactivate_mas_user"));
        assert!(actions.contains(&"delete_keycloak_user"));
        assert!(logs.iter().all(|l| l.result == "success"));
    }
}
