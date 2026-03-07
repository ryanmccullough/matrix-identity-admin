use axum::{async_trait, extract::FromRequestParts, http::request::Parts, response::Redirect};
use axum_extra::extract::cookie::{Cookie, Key, PrivateCookieJar, SameSite};
use serde::{Deserialize, Serialize};

pub const SESSION_COOKIE: &str = "session";

/// Data stored in the encrypted session cookie for an authenticated admin.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdminSession {
    pub subject: String,
    pub username: String,
    pub email: Option<String>,
    pub roles: Vec<String>,
    /// CSRF token for this session; included in every mutating form.
    pub csrf_token: String,
}

/// Axum extractor that enforces authentication. Redirects to `/auth/login`
/// if the session cookie is absent or invalid.
pub struct AuthenticatedAdmin(pub AdminSession);

#[async_trait]
impl<S> FromRequestParts<S> for AuthenticatedAdmin
where
    S: Send + Sync,
    Key: axum::extract::FromRef<S>,
{
    type Rejection = Redirect;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let jar = PrivateCookieJar::<Key>::from_request_parts(parts, state)
            .await
            // PrivateCookieJar extraction is infallible (never returns Err).
            .expect("PrivateCookieJar extraction is infallible");

        let cookie = jar
            .get(SESSION_COOKIE)
            .ok_or_else(|| Redirect::to("/auth/login"))?;

        let session: AdminSession =
            serde_json::from_str(cookie.value()).map_err(|_| Redirect::to("/auth/login"))?;

        Ok(AuthenticatedAdmin(session))
    }
}

/// Build a new session cookie containing the given `AdminSession`.
pub fn build_session_cookie(session: &AdminSession) -> Result<Cookie<'static>, serde_json::Error> {
    let value = serde_json::to_string(session)?;
    let mut cookie = Cookie::new(SESSION_COOKIE, value);
    cookie.set_http_only(true);
    cookie.set_same_site(SameSite::Lax);
    cookie.set_path("/");
    // In production this should be `true`; kept configurable via the cookie jar's secure flag.
    cookie.set_secure(true);
    Ok(cookie)
}

/// Return the cookie jar with the session cookie removed (logout).
pub fn clear_session(jar: PrivateCookieJar) -> PrivateCookieJar {
    jar.remove(Cookie::from(SESSION_COOKIE))
}
