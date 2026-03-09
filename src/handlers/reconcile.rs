use axum::{
    extract::{Path, State},
    response::{IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;

use crate::{
    auth::{csrf::validate, session::AuthenticatedAdmin},
    error::AppError,
    services::reconcile_membership::reconcile_membership,
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
        &state.config.group_mappings,
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
