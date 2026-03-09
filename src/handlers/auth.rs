use axum::{
    extract::{Query, State},
    response::{IntoResponse, Redirect},
};
use axum_extra::extract::cookie::{Cookie, PrivateCookieJar, SameSite};
use serde::Deserialize;

use crate::{
    auth::{
        csrf::{generate_token, validate},
        oidc::{OidcFlowState, OIDC_FLOW_COOKIE},
        session::{build_session_cookie, clear_session, AdminSession, AuthenticatedAdmin},
    },
    error::AppError,
    state::AppState,
};

pub async fn login(
    State(state): State<AppState>,
    jar: PrivateCookieJar,
) -> Result<impl IntoResponse, AppError> {
    let (auth_url, flow_state) = state.oidc.begin_auth();

    let flow_json = serde_json::to_string(&flow_state).map_err(|e| AppError::Internal(e.into()))?;

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

#[derive(Deserialize)]
pub struct LogoutForm {
    pub _csrf: String,
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

    let session_cookie =
        build_session_cookie(&session).map_err(|e| AppError::Internal(e.into()))?;

    let jar = jar
        .remove(Cookie::from(OIDC_FLOW_COOKIE))
        .add(session_cookie);

    Ok((jar, Redirect::to("/")))
}

pub async fn logout(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    jar: PrivateCookieJar,
    axum::extract::Form(form): axum::extract::Form<LogoutForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;
    let jar = clear_session(jar);
    Ok((jar, Redirect::to("/auth/login")))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
        routing::post,
        Router,
    };
    use tower::ServiceExt;

    use crate::test_helpers::{build_test_state, make_auth_cookie, MockKeycloak, TEST_CSRF};

    use super::logout;

    async fn post_logout(
        state: crate::state::AppState,
        csrf: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let body = format!("_csrf={csrf}");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("/auth/logout")
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(cookie) = auth_cookie {
            builder = builder.header("cookie", cookie);
        }

        Router::new()
            .route("/auth/logout", post(logout))
            .with_state(state)
            .oneshot(builder.body(Body::from(body)).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn logout_with_valid_csrf_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_logout(state, TEST_CSRF, Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn logout_with_invalid_csrf_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = post_logout(state, "wrong-csrf", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn logout_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = post_logout(state, TEST_CSRF, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }
}
