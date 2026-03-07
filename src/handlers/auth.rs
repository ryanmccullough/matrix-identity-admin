use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect},
};
use axum_extra::extract::cookie::{Cookie, PrivateCookieJar, SameSite};
use serde::Deserialize;

use crate::{
    auth::{
        csrf::generate_token,
        oidc::{OidcFlowState, OIDC_FLOW_COOKIE},
        session::{build_session_cookie, clear_session, AdminSession},
    },
    error::AppError,
    state::AppState,
};

pub async fn login(
    State(state): State<AppState>,
    jar: PrivateCookieJar,
) -> Result<impl IntoResponse, AppError> {
    let (auth_url, flow_state) = state.oidc.begin_auth();

    let flow_json = serde_json::to_string(&flow_state)
        .map_err(|e| AppError::Internal(e.into()))?;

    let mut flow_cookie = Cookie::new(OIDC_FLOW_COOKIE, flow_json);
    flow_cookie.set_http_only(true);
    flow_cookie.set_same_site(SameSite::Lax);
    flow_cookie.set_path("/");
    flow_cookie.set_secure(true);
    // Short-lived: only needed during the redirect round-trip.
    flow_cookie.set_max_age(cookie::time::Duration::minutes(10));

    Ok((jar.add(flow_cookie), Redirect::to(&auth_url)))
}

#[derive(Deserialize)]
pub struct CallbackParams {
    pub code: String,
    pub state: String,
}

pub async fn callback(
    State(state): State<AppState>,
    Query(params): Query<CallbackParams>,
    jar: PrivateCookieJar,
) -> Result<impl IntoResponse, AppError> {
    // Retrieve and consume the pre-auth flow state cookie.
    let flow_cookie = jar
        .get(OIDC_FLOW_COOKIE)
        .ok_or_else(|| AppError::Auth("Missing OIDC flow state cookie".to_string()))?;

    let flow_state: OidcFlowState = serde_json::from_str(flow_cookie.value())
        .map_err(|_| AppError::Auth("Invalid OIDC flow state".to_string()))?;

    let claims = state
        .oidc
        .complete_auth(params.code, flow_state, params.state)
        .await?;

    let session = AdminSession {
        subject: claims.subject,
        username: claims.username,
        email: claims.email,
        roles: claims.roles,
        csrf_token: generate_token(),
    };

    let session_cookie = build_session_cookie(&session)
        .map_err(|e| AppError::Internal(e.into()))?;

    let jar = jar
        .remove(Cookie::from(OIDC_FLOW_COOKIE))
        .add(session_cookie);

    Ok((jar, Redirect::to("/")))
}

pub async fn logout(
    jar: PrivateCookieJar,
) -> impl IntoResponse {
    let jar = clear_session(jar);
    (jar, Redirect::to("/auth/login"))
}
