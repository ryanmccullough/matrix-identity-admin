use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::{error::AppError, models::audit::AuditResult, state::AppState};

#[derive(Deserialize)]
pub struct InviteRequest {
    pub email: String,
    /// Matrix display name or username of the admin who issued the invite command.
    pub invited_by: String,
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

async fn handle_invite(
    state: &AppState,
    headers: &HeaderMap,
    body: InviteRequest,
) -> Result<String, AppError> {
    // ── Auth ──────────────────────────────────────────────────────────────────
    let expected = format!("Bearer {}", state.config.bot_api_secret);
    let provided = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if provided != expected {
        return Err(AppError::Auth("Invalid bot API secret".to_string()));
    }

    // ── Validate email ────────────────────────────────────────────────────────
    let email = body.email.trim().to_lowercase();
    let at = email
        .find('@')
        .ok_or_else(|| AppError::Validation("Invalid email address".to_string()))?;
    let local = &email[..at];
    let domain = &email[at + 1..];

    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return Err(AppError::Validation("Invalid email address".to_string()));
    }

    // ── Domain allowlist ──────────────────────────────────────────────────────
    if let Some(ref allowed) = state.config.invite_allowed_domains {
        if !allowed.iter().any(|d| d == domain) {
            return Err(AppError::Validation(format!(
                "Email domain '{domain}' is not permitted"
            )));
        }
    }

    // ── Check for existing Keycloak user ──────────────────────────────────────
    if let Some(existing) = state.keycloak.get_user_by_email(&email).await? {
        return Err(AppError::Validation(format!(
            "A user with email {email} already exists (id: {})",
            existing.id
        )));
    }

    // ── Create user in Keycloak ───────────────────────────────────────────────
    // Use the email local part as the Matrix username.
    let user_id = state.keycloak.create_user(local, &email).await?;
    let matrix_user_id = format!("@{}:{}", local, state.config.homeserver_domain);

    // ── Send invite email via Keycloak ────────────────────────────────────────
    let invite_result = state.keycloak.send_invite_email(&user_id).await;

    let audit_result = if invite_result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    state
        .audit
        .log(
            "bot",
            &body.invited_by,
            Some(&user_id),
            Some(&matrix_user_id),
            "invite_user",
            audit_result,
            json!({
                "email": email,
                "invited_by": body.invited_by,
                "keycloak_user_id": user_id,
            }),
        )
        .await?;

    invite_result?;

    Ok(format!("Invite sent to {email} — they will receive an email to set their password and can then log into Matrix."))
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
        test_helpers::{build_test_state, invite_router, MockKeycloak},
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
}
