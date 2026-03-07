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
pub struct RevokeForm {
    pub _csrf: String,
    /// "compat" or "oauth2" — determines which MAS endpoint to call.
    pub session_type: String,
}

pub async fn revoke(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path((keycloak_id, session_id)): Path<(String, String)>,
    Form(form): Form<RevokeForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let result = state
        .mas
        .finish_session(&session_id, &form.session_type)
        .await;

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
            "finish_mas_session",
            audit_result,
            json!({
                "session_id": session_id,
                "session_type": form.session_type,
            }),
        )
        .await?;

    result?;

    Ok(Redirect::to(&format!("/users/{keycloak_id}")))
}
