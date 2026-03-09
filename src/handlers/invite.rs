use axum::{
    extract::{Form, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect},
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::{
    auth::{csrf::validate, session::AuthenticatedAdmin},
    error::AppError,
    services::invite_user::invite_user,
    state::AppState,
};

#[derive(Deserialize)]
pub struct InviteRequest {
    pub email: String,
    /// Bot-provided attribution string (stored as metadata only).
    pub invited_by: String,
}

#[derive(Deserialize)]
pub struct AdminInviteForm {
    pub email: String,
    pub _csrf: String,
}

pub async fn create_invite(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<InviteRequest>,
) -> impl IntoResponse {
    match handle_invite(&state, &headers, body).await {
        Ok(msg) => (
            StatusCode::CREATED,
            Json(json!({"ok": true, "message": msg})),
        )
            .into_response(),
        Err(e) => {
            let (status, msg) = match &e {
                AppError::Auth(_) => (StatusCode::UNAUTHORIZED, e.to_string()),
                AppError::Validation(m) => (StatusCode::UNPROCESSABLE_ENTITY, m.clone()),
                AppError::Upstream { service, message } => (
                    StatusCode::BAD_GATEWAY,
                    format!("Upstream error ({service}): {message}"),
                ),
                _ => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Internal error".to_string(),
                ),
            };
            tracing::warn!(error = %e, "Invite request failed");
            (status, Json(json!({"ok": false, "message": msg}))).into_response()
        }
    }
}

/// POST /users/invite — admin UI invite (OIDC session + CSRF, not bearer token).
pub async fn admin_invite(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Form(form): Form<AdminInviteForm>,
) -> impl IntoResponse {
    if let Err(e) = validate(&admin.csrf_token, &form._csrf) {
        return Redirect::to(&format!("/?error={}", pct_encode(&e.to_string()))).into_response();
    }

    match invite_user(
        &form.email,
        state.config.invite_allowed_domains.as_deref(),
        state.keycloak.as_ref(),
        state.mas.as_ref(),
        &state.audit,
        &admin.subject,
        &admin.username,
        &state.config.homeserver_domain,
        None,
    )
    .await
    {
        Ok(email) => Redirect::to(&format!(
            "/?notice={}",
            pct_encode(&format!("Invite sent to {email}"))
        ))
        .into_response(),
        Err(e) => {
            tracing::warn!(error = %e, "Admin invite failed");
            Redirect::to(&format!("/?error={}", pct_encode(&e.to_string()))).into_response()
        }
    }
}

/// Minimal percent-encoder for use in redirect query params.
fn pct_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b' ' => {
                if b == b' ' {
                    out.push('+');
                } else {
                    out.push(b as char);
                }
            }
            b => {
                out.push('%');
                out.push(
                    char::from_digit((b >> 4) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
                out.push(
                    char::from_digit((b & 0xf) as u32, 16)
                        .unwrap()
                        .to_ascii_uppercase(),
                );
            }
        }
    }
    out
}

async fn handle_invite(
    state: &AppState,
    headers: &HeaderMap,
    body: InviteRequest,
) -> Result<String, AppError> {
    let expected = format!("Bearer {}", state.config.bot_api_secret);
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided != expected {
        return Err(AppError::Auth("Invalid bot API secret".to_string()));
    }

    // Do not trust caller-provided attribution for audit actor identity.
    invite_user(
        &body.email,
        state.config.invite_allowed_domains.as_deref(),
        state.keycloak.as_ref(),
        state.mas.as_ref(),
        &state.audit,
        "bot-api",
        "bot-api",
        &state.config.homeserver_domain,
        Some(&body.invited_by),
    )
    .await
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
        models::{keycloak::KeycloakUser, mas::MasUser},
        test_helpers::{
            admin_invite_router, build_test_state, build_test_state_full, invite_router,
            make_auth_cookie, MockKeycloak, MockMas, TEST_CSRF,
        },
    };

    // ── Helpers ───────────────────────────────────────────────────────────────

    const SECRET: &str = "test-bot-secret";

    fn invite_body(email: &str) -> Body {
        Body::from(format!(
            r#"{{"email":"{email}","invited_by":"@bot:test.com"}}"#
        ))
    }

    async fn post_invite(
        state: crate::state::AppState,
        auth_header: Option<&str>,
        body: Body,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/invites")
            .header("content-type", "application/json");

        if let Some(auth) = auth_header {
            builder = builder.header("authorization", auth);
        }

        let req: Request<Body> = builder.body(body).unwrap();
        invite_router(state).oneshot(req).await.unwrap()
    }

    async fn json_body(resp: axum::response::Response) -> serde_json::Value {
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn existing_user(email: &str) -> KeycloakUser {
        KeycloakUser {
            id: "existing-id".to_string(),
            username: "existing".to_string(),
            email: Some(email.to_string()),
            first_name: None,
            last_name: None,
            enabled: true,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    // ── Auth ──────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn missing_auth_header_returns_401() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_invite(state, None, invite_body("user@test.com")).await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let json = json_body(resp).await;
        assert_eq!(json["ok"], false);
    }

    #[tokio::test]
    async fn wrong_bearer_secret_returns_401() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_invite(
            state,
            Some("Bearer wrong-secret"),
            invite_body("user@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let json = json_body(resp).await;
        assert_eq!(json["ok"], false);
    }

    // ── Email validation ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn invalid_email_no_at_sign_returns_422() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("notanemail"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json = json_body(resp).await;
        assert_eq!(json["ok"], false);
    }

    #[tokio::test]
    async fn invalid_email_empty_local_part_returns_422() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    #[tokio::test]
    async fn invalid_email_no_dot_in_domain_returns_422() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@localhost"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }

    // ── Domain allowlist ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn domain_not_on_allowlist_returns_422() {
        let state = build_test_state(
            MockKeycloak::default(),
            SECRET,
            Some(vec!["allowed.com".to_string()]),
        )
        .await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@other.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json = json_body(resp).await;
        assert!(
            json["message"]
                .as_str()
                .unwrap_or("")
                .contains("not permitted"),
            "expected 'not permitted' in message, got: {json}"
        );
    }

    #[tokio::test]
    async fn allowed_domain_passes_allowlist_check() {
        let state = build_test_state(
            MockKeycloak::default(),
            SECRET,
            Some(vec!["allowed.com".to_string()]),
        )
        .await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@allowed.com"),
        )
        .await;
        // Should not be blocked by domain check — reaches invite creation
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn no_allowlist_permits_any_domain() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@any-domain-at-all.io"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // ── Duplicate email ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn duplicate_email_returns_422() {
        let state = build_test_state(
            MockKeycloak {
                user_by_email: Some(existing_user("user@test.com")),
                ..Default::default()
            },
            SECRET,
            None,
        )
        .await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let json = json_body(resp).await;
        assert!(
            json["message"]
                .as_str()
                .unwrap_or("")
                .contains("already exists"),
            "expected 'already exists' in message, got: {json}"
        );
    }

    // ── Upstream failures ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn keycloak_create_failure_returns_502() {
        let state = build_test_state(
            MockKeycloak {
                fail_create: true,
                ..Default::default()
            },
            SECRET,
            None,
        )
        .await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let json = json_body(resp).await;
        assert_eq!(json["ok"], false);
    }

    #[tokio::test]
    async fn send_invite_email_failure_returns_502() {
        let state = build_test_state(
            MockKeycloak {
                fail_send_invite: true,
                ..Default::default()
            },
            SECRET,
            None,
        )
        .await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    }

    // ── Happy path ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn successful_invite_returns_201_with_ok_true() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("newuser@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let json = json_body(resp).await;
        assert_eq!(json["ok"], true);
        assert!(
            json["message"]
                .as_str()
                .unwrap_or("")
                .contains("newuser@test.com"),
            "expected email in message, got: {json}"
        );
    }

    #[tokio::test]
    async fn email_is_lowercased_before_processing() {
        // The handler normalises the email to lowercase. This test verifies that
        // an uppercase email is processed without error (not treated as invalid).
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            Body::from(r#"{"email":"User@Test.COM","invited_by":"@bot:test.com"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // ── MAS reactivation ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn deactivated_mas_user_is_reactivated_on_invite() {
        // If a deactivated MAS account exists for the username derived from the
        // invite email, the handler should reactivate it (preserving the MXID)
        // rather than letting it sit deactivated.
        let state = build_test_state_full(
            MockKeycloak::default(),
            MockMas {
                user: Some(MasUser {
                    id: "mas-deactivated-id".to_string(),
                    username: "user".to_string(),
                    deactivated_at: Some("2026-01-01T00:00:00Z".to_string()),
                }),
                ..Default::default()
            },
            SECRET,
            None,
        )
        .await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let json = json_body(resp).await;
        assert_eq!(json["ok"], true);
    }

    #[tokio::test]
    async fn active_mas_user_does_not_block_invite() {
        // An active MAS account with the same username is unusual (KC was deleted
        // but MAS wasn't deactivated), but should not block the invite.
        let state = build_test_state_full(
            MockKeycloak::default(),
            MockMas {
                user: Some(MasUser {
                    id: "mas-active-id".to_string(),
                    username: "user".to_string(),
                    deactivated_at: None,
                }),
                ..Default::default()
            },
            SECRET,
            None,
        )
        .await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // ── Admin invite UI ───────────────────────────────────────────────────────

    async fn post_admin_invite(
        state: crate::state::AppState,
        cookie: Option<String>,
        body: &str,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/users/invite")
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(c) = cookie {
            builder = builder.header("cookie", c);
        }
        let req = builder.body(Body::from(body.to_string())).unwrap();
        admin_invite_router(state).oneshot(req).await.unwrap()
    }

    #[tokio::test]
    async fn admin_invite_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_admin_invite(
            state,
            None,
            &format!("email=user%40test.com&_csrf={TEST_CSRF}"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn admin_invite_invalid_csrf_redirects_with_error() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_admin_invite(
            state,
            Some(make_auth_cookie(TEST_CSRF)),
            "email=user%40test.com&_csrf=wrong-token",
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            location.starts_with("/?error="),
            "expected /?error= redirect, got: {location}"
        );
    }

    #[tokio::test]
    async fn admin_invite_success_redirects_with_notice() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let resp = post_admin_invite(
            state,
            Some(make_auth_cookie(TEST_CSRF)),
            &format!("email=new%40test.com&_csrf={TEST_CSRF}"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            location.starts_with("/?notice="),
            "expected /?notice= redirect, got: {location}"
        );
    }

    #[tokio::test]
    async fn admin_invite_keycloak_failure_redirects_with_error() {
        let state = build_test_state(
            MockKeycloak {
                fail_create: true,
                ..Default::default()
            },
            SECRET,
            None,
        )
        .await;
        let resp = post_admin_invite(
            state,
            Some(make_auth_cookie(TEST_CSRF)),
            &format!("email=new%40test.com&_csrf={TEST_CSRF}"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let location = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            location.starts_with("/?error="),
            "expected /?error= redirect, got: {location}"
        );
    }

    #[tokio::test]
    async fn mas_lookup_failure_during_invite_still_proceeds() {
        // MAS lookup fails → warning logged → treats as no existing MAS user → invite proceeds.
        let state = build_test_state_full(
            MockKeycloak::default(),
            MockMas {
                fail_get_user_by_username: true,
                ..Default::default()
            },
            SECRET,
            None,
        )
        .await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);
        let json = json_body(resp).await;
        assert_eq!(json["ok"], true);
    }

    #[tokio::test]
    async fn database_error_during_audit_returns_500() {
        // After closing the pool, any DB operation (audit.log) will fail with a
        // Database error, which maps to the catch-all `_ =>` arm (500).
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let pool = state.db.clone();
        let router = invite_router(state);
        // Close the pool so all DB queries fail.
        pool.close().await;

        let req = Request::builder()
            .method(Method::POST)
            .uri("/api/v1/invites")
            .header("content-type", "application/json")
            .header("authorization", format!("Bearer {SECRET}"))
            .body(invite_body("user@test.com"))
            .unwrap();

        let resp = router.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let json = json_body(resp).await;
        assert_eq!(json["ok"], false);
    }

    #[tokio::test]
    async fn bot_invite_uses_trusted_audit_actor_not_payload_invited_by() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let audit = std::sync::Arc::clone(&state.audit);
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            Body::from(r#"{"email":"user@test.com","invited_by":"spoofed-admin"}"#),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::CREATED);

        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        assert!(
            logs.iter().any(|l| l.action == "invite_user"),
            "expected invite_user audit log"
        );
        let invite_log = logs
            .into_iter()
            .find(|l| l.action == "invite_user")
            .unwrap();
        assert_eq!(invite_log.admin_subject, "bot-api");
        assert_eq!(invite_log.admin_username, "bot-api");
        assert!(invite_log.metadata_json.contains("spoofed-admin"));
    }

    #[tokio::test]
    async fn admin_invite_uses_authenticated_admin_as_audit_actor() {
        let state = build_test_state(MockKeycloak::default(), SECRET, None).await;
        let audit = std::sync::Arc::clone(&state.audit);
        let resp = post_admin_invite(
            state,
            Some(make_auth_cookie(TEST_CSRF)),
            &format!("email=new%40test.com&_csrf={TEST_CSRF}"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);

        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        assert!(
            logs.iter().any(|l| l.action == "invite_user"),
            "expected invite_user audit log"
        );
        let invite_log = logs
            .into_iter()
            .find(|l| l.action == "invite_user")
            .unwrap();
        assert_eq!(invite_log.admin_username, "test-admin");
    }

    #[tokio::test]
    async fn mas_reactivate_failure_returns_502() {
        let state = build_test_state_full(
            MockKeycloak::default(),
            MockMas {
                user: Some(MasUser {
                    id: "mas-deactivated-id".to_string(),
                    username: "user".to_string(),
                    deactivated_at: Some("2026-01-01T00:00:00Z".to_string()),
                }),
                fail_reactivate: true,
                ..Default::default()
            },
            SECRET,
            None,
        )
        .await;
        let resp = post_invite(
            state,
            Some(&format!("Bearer {SECRET}")),
            invite_body("user@test.com"),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let json = json_body(resp).await;
        assert_eq!(json["ok"], false);
    }
}
