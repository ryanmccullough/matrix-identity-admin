use askama::Template;
use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;

use crate::{
    auth::{csrf::validate, session::AuthenticatedAdmin},
    error::AppError,
    services::reconcile_membership::{preview_membership, reconcile_membership, RoomAction},
    state::AppState,
    utils::pct_encode,
};

#[derive(Deserialize)]
pub struct ReconcileForm {
    pub _csrf: String,
}

/// POST /users/{id}/reconcile
///
/// Compares the user's Keycloak group membership against the configured
/// group → room policy and force-joins them into any rooms they should be in.
/// Optionally kicks them from rooms they should no longer be in
/// (controlled by `RECONCILE_REMOVE_FROM_ROOMS`).
///
/// Returns 404 if Synapse is not configured (the button is hidden in the UI,
/// but guard here in case of direct POST).
pub async fn reconcile(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
    Form(form): Form<ReconcileForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let synapse = state.synapse.as_ref().ok_or_else(|| {
        AppError::NotFound("Synapse is not configured — reconciliation is unavailable".into())
    })?;

    let kc_user = state.keycloak.get_user(&keycloak_id).await?;
    let kc_groups = state.keycloak.get_user_groups(&keycloak_id).await?;
    let group_names: Vec<String> = kc_groups.into_iter().map(|g| g.name).collect();

    let matrix_user_id = format!("@{}:{}", kc_user.username, state.config.homeserver_domain);

    let outcome = reconcile_membership(
        &keycloak_id,
        &matrix_user_id,
        &state.policy,
        &group_names,
        synapse.as_ref(),
        &state.audit,
        &admin.subject,
        &admin.username,
        state.config.reconcile_remove_from_rooms,
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
        format!(
            "/users/{keycloak_id}?notice={}",
            pct_encode("Room membership reconciled")
        )
    };

    Ok(Redirect::to(&redirect))
}

#[derive(Template)]
#[template(path = "reconcile_preview.html")]
struct ReconcilePreviewTemplate {
    keycloak_id: String,
    csrf_token: String,
    joins: Vec<RoomAction>,
    kicks: Vec<RoomAction>,
    already_correct: Vec<RoomAction>,
    warnings: Vec<String>,
}

/// POST /users/{id}/reconcile/preview
///
/// Returns an HTML fragment (HTMX swap target) showing what `reconcile_membership`
/// would do without executing any changes. No audit entries are written.
///
/// Returns 404 if Synapse is not configured.
pub async fn reconcile_preview(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
    Form(form): Form<ReconcileForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let synapse = state.synapse.as_ref().ok_or_else(|| {
        AppError::NotFound("Synapse is not configured — reconciliation is unavailable".into())
    })?;

    let kc_user = state.keycloak.get_user(&keycloak_id).await?;
    let kc_groups = state.keycloak.get_user_groups(&keycloak_id).await?;
    let group_names: Vec<String> = kc_groups.into_iter().map(|g| g.name).collect();

    let matrix_user_id = format!("@{}:{}", kc_user.username, state.config.homeserver_domain);

    let preview = preview_membership(
        &matrix_user_id,
        &state.policy,
        &group_names,
        synapse.as_ref(),
        state.config.reconcile_remove_from_rooms,
    )
    .await?;

    let tmpl = ReconcilePreviewTemplate {
        keycloak_id,
        csrf_token: admin.csrf_token,
        joins: preview.joins,
        kicks: preview.kicks,
        already_correct: preview.already_correct,
        warnings: preview.warnings,
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
    };
    use tower::ServiceExt;

    use crate::{
        models::{group_mapping::GroupMapping, keycloak::KeycloakUser},
        test_helpers::{
            build_test_state_full, build_test_state_with_synapse, make_auth_cookie,
            mutations_router, MockKeycloak, MockMas, MockSynapse, TEST_CSRF,
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

    fn test_mapping() -> GroupMapping {
        GroupMapping {
            keycloak_group: "staff".to_string(),
            matrix_room_id: "!room1:test.com".to_string(),
        }
    }

    async fn post_reconcile(
        state: crate::state::AppState,
        user_id: &str,
        csrf: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = format!("_csrf={csrf}");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("/users/{user_id}/reconcile"))
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
    async fn reconcile_unauthenticated_redirects_to_login() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockSynapse::default(),
            vec![test_mapping()],
            false,
        )
        .await;
        let resp = post_reconcile(state, "kc-123", TEST_CSRF, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn reconcile_invalid_csrf_returns_400() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockSynapse::default(),
            vec![test_mapping()],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reconcile(state, "kc-123", "wrong-csrf", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn reconcile_without_synapse_returns_404() {
        // State with synapse: None
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
        let resp = post_reconcile(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reconcile_user_not_found_returns_404() {
        let state = build_test_state_with_synapse(
            MockKeycloak::default(), // no users
            MockSynapse::default(),
            vec![test_mapping()],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reconcile(state, "nonexistent", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn reconcile_success_redirects_with_notice() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockSynapse::default(), // user not in room → will be joined
            vec![test_mapping()],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reconcile(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("/users/kc-123?notice="));
    }

    #[tokio::test]
    async fn reconcile_with_synapse_failure_redirects_with_warning() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockSynapse {
                fail_get_members: true,
                ..Default::default()
            },
            vec![test_mapping()],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reconcile(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("/users/kc-123?warning="));
    }

    #[tokio::test]
    async fn reconcile_no_mappings_redirects_with_notice() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockSynapse::default(),
            vec![], // no mappings configured
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_reconcile(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(location.starts_with("/users/kc-123?notice="));
    }

    // ── Preview handler tests ──────────────────────────────────────────────────

    async fn post_preview(
        state: crate::state::AppState,
        user_id: &str,
        csrf: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = format!("_csrf={csrf}");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(format!("/users/{user_id}/reconcile/preview"))
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
    async fn preview_unauthenticated_redirects_to_login() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockSynapse::default(),
            vec![test_mapping()],
            false,
        )
        .await;
        let resp = post_preview(state, "kc-123", TEST_CSRF, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn preview_invalid_csrf_returns_400() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockSynapse::default(),
            vec![test_mapping()],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_preview(state, "kc-123", "wrong-csrf", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn preview_without_synapse_returns_404() {
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
        let resp = post_preview(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn preview_user_not_found_returns_404() {
        let state = build_test_state_with_synapse(
            MockKeycloak::default(), // no users
            MockSynapse::default(),
            vec![test_mapping()],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_preview(state, "nonexistent", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn preview_success_returns_html_fragment() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockSynapse::default(), // user not in room → will show as join
            vec![test_mapping()],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_preview(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let content_type = resp
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(content_type.contains("text/html"));
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("class=\"reconcile-preview\""),
            "expected preview div in response: {html}"
        );
    }

    #[tokio::test]
    async fn preview_synapse_failure_returns_html_with_warnings() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                users: vec![test_kc_user()],
                ..Default::default()
            },
            MockSynapse {
                fail_get_members: true,
                ..Default::default()
            },
            vec![test_mapping()],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_preview(state, "kc-123", TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            html.contains("reconcile-preview"),
            "expected preview div in response"
        );
        assert!(
            html.contains("Could not fetch members") || html.contains("preview-warn"),
            "expected warnings in response: {html}"
        );
    }
}
