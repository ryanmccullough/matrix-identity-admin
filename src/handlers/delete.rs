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

    // ── Delete MAS user first (if present) ───────────────────────────────────
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
                "delete_mas_user",
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
