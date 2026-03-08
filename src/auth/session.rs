use axum::{extract::FromRequestParts, http::request::Parts, response::Redirect};
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

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
        routing::post,
        Router,
    };
    use axum_extra::extract::cookie::PrivateCookieJar;
    use tower::ServiceExt;

    use super::{build_session_cookie, clear_session, AdminSession, SESSION_COOKIE};
    use crate::test_helpers::{build_test_state, make_auth_cookie, MockKeycloak, TEST_CSRF};

    fn test_session() -> AdminSession {
        AdminSession {
            subject: "sub".to_string(),
            username: "admin".to_string(),
            email: None,
            roles: vec![],
            csrf_token: "tok".to_string(),
        }
    }

    #[test]
    fn build_session_cookie_has_correct_name_and_flags() {
        let session = test_session();
        let cookie = build_session_cookie(&session).expect("should serialize");
        assert_eq!(cookie.name(), SESSION_COOKIE);
        assert_eq!(cookie.http_only(), Some(true));
        assert_eq!(cookie.secure(), Some(true));
        assert_eq!(cookie.path(), Some("/"));
    }

    #[test]
    fn build_session_cookie_value_deserializes_back() {
        let session = test_session();
        let cookie = build_session_cookie(&session).unwrap();
        let roundtripped: AdminSession = serde_json::from_str(cookie.value()).unwrap();
        assert_eq!(roundtripped.username, "admin");
        assert_eq!(roundtripped.csrf_token, "tok");
    }

    #[tokio::test]
    async fn clear_session_removes_the_session_cookie() {
        async fn logout_handler(jar: PrivateCookieJar) -> (PrivateCookieJar, StatusCode) {
            (clear_session(jar), StatusCode::OK)
        }

        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let app = Router::new()
            .route("/logout", post(logout_handler))
            .with_state(state);

        let req = Request::builder()
            .method(Method::POST)
            .uri("/logout")
            .header("cookie", make_auth_cookie(TEST_CSRF))
            .body(Body::empty())
            .unwrap();

        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // The response should contain a Set-Cookie header that clears the session cookie.
        let set_cookie = resp
            .headers()
            .get("set-cookie")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            set_cookie.contains(SESSION_COOKIE),
            "expected Set-Cookie to reference session cookie, got: {set_cookie}"
        );
    }
}
