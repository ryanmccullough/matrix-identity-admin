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
pub struct ForceLogoutForm {
    pub _csrf: String,
}

pub async fn force_keycloak_logout(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
    Form(form): Form<ForceLogoutForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let result = state.keycloak.logout_user(&keycloak_id).await;

    let audit_result = match &result {
        Ok(_) => AuditResult::Success,
        Err(_) => AuditResult::Failure,
    };

    state
        .audit
        .log(
            &admin.subject,
            &admin.username,
            Some(&keycloak_id),
            None,
            "force_keycloak_logout",
            audit_result,
            json!({ "keycloak_user_id": keycloak_id }),
        )
        .await?;

    result?;

    Ok(Redirect::to(&format!("/users/{keycloak_id}")))
}
