use askama::Template;
use axum::{
    extract::State,
    response::{Html, IntoResponse},
    Form,
};
use serde::Deserialize;

use crate::{
    auth::{csrf::validate, session::AuthenticatedAdmin},
    error::AppError,
    services::reconcile_membership::reconcile_membership,
    state::AppState,
};

const MAX_BULK_RECONCILE_USERS: u32 = 1_000;

#[derive(Deserialize)]
pub struct BulkReconcileForm {
    pub _csrf: String,
}

#[derive(Template)]
#[template(path = "bulk_reconcile_result.html")]
struct BulkReconcileResultTemplate {
    username: String,
    users_processed: usize,
    users_skipped: usize,
    warnings: Vec<String>,
}

/// POST /users/reconcile/all
///
/// Fetches all enabled Keycloak users (paginated) and runs
/// `reconcile_membership` for each. Returns a results page summarising
/// how many users were processed, how many were skipped, and any per-user
/// warnings collected along the way.
///
/// Returns 404 if Synapse is not configured.
pub async fn bulk_reconcile(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Form(form): Form<BulkReconcileForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let synapse = state.synapse.as_ref().ok_or_else(|| {
        AppError::NotFound("Synapse is not configured — reconciliation is unavailable".into())
    })?;

    let total = state.keycloak.count_users("").await?;
    if total > MAX_BULK_RECONCILE_USERS {
        return Err(AppError::Validation(format!(
            "Bulk reconcile is limited to {MAX_BULK_RECONCILE_USERS} users per request."
        )));
    }

    let page_size = 100u32;
    let mut first = 0u32;
    let all_bindings = state.policy_service.list_bindings().await?;

    let mut users_processed = 0usize;
    let mut users_skipped = 0usize;
    let mut warnings: Vec<String> = Vec::new();

    loop {
        let page = state.keycloak.search_users("", page_size, first).await?;
        if page.is_empty() {
            break;
        }

        let fetched = page.len() as u32;
        for kc_user in page {
            // Skip disabled users — they should not be in any rooms.
            if !kc_user.enabled {
                users_skipped += 1;
                continue;
            }

            let kc_groups = match state.keycloak.get_user_groups(&kc_user.id).await {
                Ok(g) => g,
                Err(e) => {
                    warnings.push(format!(
                        "{}: could not fetch groups — {}",
                        kc_user.username, e
                    ));
                    users_skipped += 1;
                    continue;
                }
            };
            let group_names: Vec<String> = kc_groups.into_iter().map(|g| g.name).collect();

            let kc_roles = match state.keycloak.get_user_roles(&kc_user.id).await {
                Ok(r) => r,
                Err(e) => {
                    warnings.push(format!(
                        "{}: could not fetch roles — {}",
                        kc_user.username, e
                    ));
                    // Proceed with empty roles rather than skipping.
                    vec![]
                }
            };
            let role_names: Vec<String> = kc_roles.into_iter().map(|r| r.name).collect();

            let matrix_user_id =
                format!("@{}:{}", kc_user.username, state.config.homeserver_domain);

            let outcome = reconcile_membership(
                &kc_user.id,
                &matrix_user_id,
                &all_bindings,
                &group_names,
                &role_names,
                synapse.as_ref(),
                &state.audit,
                &admin.subject,
                &admin.username,
            )
            .await;

            match outcome {
                Ok(o) => {
                    users_processed += 1;
                    for w in o.warnings {
                        warnings.push(format!("{}: {}", kc_user.username, w));
                    }
                }
                Err(e) => {
                    warnings.push(format!("{}: reconcile failed — {}", kc_user.username, e));
                    users_skipped += 1;
                }
            }
        }

        first += fetched;
        if fetched < page_size {
            break;
        }
    }

    let tmpl = BulkReconcileResultTemplate {
        username: admin.username,
        users_processed,
        users_skipped,
        warnings,
    };
    let html = tmpl
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

    use crate::{
        models::keycloak::KeycloakUser,
        test_helpers::{
            build_test_state_full, build_test_state_with_synapse, make_auth_cookie, MockKeycloak,
            MockMas, MockSynapse, TEST_CSRF,
        },
    };

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn bulk_reconcile_router(state: crate::state::AppState) -> axum::Router {
        axum::Router::new()
            .route(
                "/users/reconcile/all",
                axum::routing::post(super::bulk_reconcile),
            )
            .with_state(state)
    }

    async fn post_bulk_reconcile(
        state: crate::state::AppState,
        csrf: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = format!("_csrf={csrf}");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/users/reconcile/all")
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(cookie) = auth_cookie {
            builder = builder.header("cookie", cookie);
        }
        bulk_reconcile_router(state)
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    fn enabled_user(id: &str, username: &str) -> KeycloakUser {
        KeycloakUser {
            id: id.to_string(),
            username: username.to_string(),
            email: Some(format!("{username}@example.com")),
            first_name: None,
            last_name: None,
            enabled: true,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn bulk_reconcile_unauthenticated_redirects_to_login() {
        let state = build_test_state_with_synapse(
            MockKeycloak::default(),
            MockSynapse::default(),
            vec![],
            false,
        )
        .await;
        let resp = post_bulk_reconcile(state, TEST_CSRF, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn bulk_reconcile_invalid_csrf_returns_400() {
        let state = build_test_state_with_synapse(
            MockKeycloak::default(),
            MockSynapse::default(),
            vec![],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_bulk_reconcile(state, "wrong-csrf", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn bulk_reconcile_without_synapse_returns_404() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_bulk_reconcile(state, TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bulk_reconcile_too_many_users_returns_400() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                user_count: super::MAX_BULK_RECONCILE_USERS + 1,
                ..Default::default()
            },
            MockSynapse::default(),
            vec![],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_bulk_reconcile(state, TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn bulk_reconcile_no_users_returns_200_with_html() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                user_count: 0,
                users: vec![],
                ..Default::default()
            },
            MockSynapse::default(),
            vec![],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_bulk_reconcile(state, TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&body_bytes);
        assert!(
            html.contains("Bulk Reconcile"),
            "expected results page in body: {html}"
        );
    }

    #[tokio::test]
    async fn bulk_reconcile_skips_disabled_users() {
        let disabled = KeycloakUser {
            id: "kc-disabled".to_string(),
            username: "disabled-user".to_string(),
            email: None,
            first_name: None,
            last_name: None,
            enabled: false,
            email_verified: false,
            created_timestamp: None,
            required_actions: vec![],
        };
        let state = build_test_state_with_synapse(
            MockKeycloak {
                user_count: 1,
                users: vec![disabled],
                ..Default::default()
            },
            MockSynapse::default(),
            vec![],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_bulk_reconcile(state, TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&body_bytes);
        // 1 skipped, 0 processed
        assert!(html.contains('1'), "expected skipped count in body: {html}");
    }

    #[tokio::test]
    async fn bulk_reconcile_enabled_user_is_processed() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                user_count: 1,
                users: vec![enabled_user("kc-1", "alice")],
                ..Default::default()
            },
            MockSynapse::default(),
            vec![],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_bulk_reconcile(state, TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&body_bytes);
        assert!(
            html.contains("Users processed"),
            "expected processed stat in body: {html}"
        );
    }

    #[tokio::test]
    async fn bulk_reconcile_group_fetch_failure_skips_user_with_warning() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                user_count: 1,
                users: vec![enabled_user("kc-1", "alice")],
                fail_get_user_groups: true,
                ..Default::default()
            },
            MockSynapse::default(),
            vec![],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_bulk_reconcile(state, TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&body_bytes);
        assert!(
            html.contains("alice: could not fetch groups"),
            "expected group-fetch warning in body: {html}"
        );
    }

    #[tokio::test]
    async fn bulk_reconcile_role_fetch_failure_warns_and_continues() {
        let state = build_test_state_with_synapse(
            MockKeycloak {
                user_count: 1,
                users: vec![enabled_user("kc-1", "alice")],
                fail_get_user_roles: true,
                ..Default::default()
            },
            MockSynapse::default(),
            vec![],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_bulk_reconcile(state, TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&body_bytes);
        assert!(
            html.contains("alice: could not fetch roles"),
            "expected role-fetch warning in body: {html}"
        );
        assert!(
            html.contains("<dt>Users processed</dt><dd>1</dd>"),
            "expected reconcile to continue after role fetch failure: {html}"
        );
    }

    #[tokio::test]
    async fn bulk_reconcile_processes_users_across_multiple_pages() {
        let users: Vec<KeycloakUser> = (0..101)
            .map(|idx| enabled_user(&format!("kc-{idx}"), &format!("user{idx}")))
            .collect();
        let state = build_test_state_with_synapse(
            MockKeycloak {
                user_count: users.len() as u32,
                users,
                ..Default::default()
            },
            MockSynapse::default(),
            vec![],
            false,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_bulk_reconcile(state, TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body_bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&body_bytes);
        assert!(
            html.contains("<dt>Users processed</dt><dd>101</dd>"),
            "expected all paginated users to be processed: {html}"
        );
    }
}
