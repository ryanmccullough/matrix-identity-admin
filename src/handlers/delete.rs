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
pub struct DeleteUserForm {
    pub _csrf: String,
}

/// POST /users/{id}/delete
///
/// Deletes the user from both Keycloak and MAS (if a MAS account exists).
/// MAS is attempted first so that if it fails the Keycloak record is preserved
/// and the admin can retry. Both deletions are audit-logged.
pub async fn delete_user(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
    Form(form): Form<DeleteUserForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    // Resolve username and MAS ID before deleting anything.
    let kc_user = state.keycloak.get_user(&keycloak_id).await?;
    let username = kc_user.username.clone();
    let matrix_user_id = format!("@{}:{}", username, state.config.homeserver_domain);

    let mas_user = state
        .mas
        .get_user_by_username(&username)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "MAS user lookup failed during delete");
            None
        });

    // ── Deactivate MAS user first (if present) ───────────────────────────────
    // Note: MAS deactivation revokes sessions but does not free the email address.
    // See the TODO in MasClient::delete_user for the permanent solution.
    if let Some(ref mas) = mas_user {
        let mas_result = state.mas.delete_user(&mas.id).await;
        let audit_result = if mas_result.is_ok() {
            AuditResult::Success
        } else {
            AuditResult::Failure
        };

        state
            .audit
            .log(
                &admin.subject,
                &admin.username,
                Some(&keycloak_id),
                Some(&matrix_user_id),
                "deactivate_mas_user",
                audit_result,
                json!({
                    "keycloak_user_id": keycloak_id,
                    "mas_user_id": mas.id,
                    "username": username,
                }),
            )
            .await?;

        mas_result?;
    }

    // ── Delete Keycloak user ──────────────────────────────────────────────────
    let kc_result = state.keycloak.delete_user(&keycloak_id).await;
    let audit_result = if kc_result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    state
        .audit
        .log(
            &admin.subject,
            &admin.username,
            Some(&keycloak_id),
            Some(&matrix_user_id),
            "delete_keycloak_user",
            audit_result,
            json!({
                "keycloak_user_id": keycloak_id,
                "username": username,
                "mas_deleted": mas_user.is_some(),
            }),
        )
        .await?;

    kc_result?;

    Ok(Redirect::to("/users/search"))
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
        let state = build_test_state_full(
            MockKeycloak::default(), // no users → get_user returns NotFound
            MockMas::default(),
            "secret",
            None,
        )
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
        // No cookie → AuthenticatedAdmin redirects to /auth/login
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
                fail_delete_user: true,
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
    async fn delete_keycloak_failure_returns_502() {
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
    async fn delete_mas_lookup_failure_still_deletes_keycloak_user() {
        // When MAS lookup fails, the handler logs a warning and treats it as no
        // MAS account — skips MAS deletion and proceeds to delete the Keycloak user.
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
        // Should succeed (redirect to /users/search), Keycloak user deleted.
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/users/search");
    }

    #[tokio::test]
    async fn delete_success_writes_audit_logs() {
        // With a MAS account the handler writes two entries: deactivate_mas_user
        // and delete_keycloak_user. Both should be recorded against kc-123.
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
