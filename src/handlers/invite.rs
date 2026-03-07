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
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "Internal error".to_string()),
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
    let at = email.find('@').ok_or_else(|| {
        AppError::Validation("Invalid email address".to_string())
    })?;
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
