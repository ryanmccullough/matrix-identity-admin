use askama::Template;
use axum::{
    extract::{Path, State},
    response::{Html, IntoResponse, Redirect},
    Form,
};
use serde::Deserialize;

use crate::{
    auth::{csrf::validate, session::AuthenticatedAdmin},
    error::AppError,
    models::policy_binding::{CachedRoom, PolicyBinding, PolicySubject, PolicyTarget},
    state::AppState,
    utils::pct_encode,
};

// ── Templates ────────────────────────────────────────────────────────────────

#[derive(Template)]
#[template(path = "policy.html")]
struct PolicyTemplate {
    username: String,
    csrf_token: String,
    bindings: Vec<PolicyBinding>,
    groups: Vec<String>,
    roles: Vec<String>,
    rooms: Vec<CachedRoom>,
    synapse_enabled: bool,
    notice: String,
    warning: String,
}

// ── Form types ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct CreateBindingForm {
    pub _csrf: String,
    pub subject_type: String,
    pub subject_value: String,
    pub target_type: String,
    pub target_room_id: String,
    pub power_level: Option<String>,
    pub allow_remove: Option<String>,
}

#[derive(Deserialize)]
pub struct UpdateBindingForm {
    pub _csrf: String,
    pub power_level: Option<String>,
    pub allow_remove: Option<String>,
}

#[derive(Deserialize)]
pub struct CsrfForm {
    pub _csrf: String,
}

#[derive(Deserialize)]
pub struct PolicyQuery {
    #[serde(default)]
    pub notice: String,
    #[serde(default)]
    pub warning: String,
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// GET /policy — render the policy management page.
pub async fn list(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<PolicyQuery>,
) -> Result<Html<String>, AppError> {
    let bindings = state.policy_service.list_bindings().await?;
    let rooms = state.policy_service.list_cached_rooms().await?;

    // Groups and roles are now loaded via HTMX fragment endpoints
    // (/policy/api/groups, /policy/api/roles). Keep empty vecs here until
    // the template is updated in a follow-up task.
    let groups: Vec<String> = Vec::new();
    let roles: Vec<String> = Vec::new();

    let synapse_enabled = state.synapse.is_some();

    let html = PolicyTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        bindings,
        groups,
        roles,
        rooms,
        synapse_enabled,
        notice: query.notice,
        warning: query.warning,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

    Ok(Html(html))
}

/// POST /policy/bindings — create a new policy binding.
pub async fn create(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Form(form): Form<CreateBindingForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let subject = match form.subject_type.as_str() {
        "group" => PolicySubject::Group(form.subject_value),
        "role" => PolicySubject::Role(form.subject_value),
        _ => return Err(AppError::Validation("Invalid subject type".into())),
    };

    let target = match form.target_type.as_str() {
        "room" => PolicyTarget::Room(form.target_room_id),
        "space" => PolicyTarget::Space(form.target_room_id),
        _ => return Err(AppError::Validation("Invalid target type".into())),
    };

    let power_level = form.power_level.and_then(|s| s.trim().parse::<i64>().ok());

    let allow_remove = form.allow_remove.is_some();

    state
        .policy_service
        .create_binding(
            &subject,
            &target,
            power_level,
            allow_remove,
            &state.audit,
            &admin.subject,
            &admin.username,
        )
        .await?;

    let notice = pct_encode("Binding created");
    Ok(Redirect::to(&format!("/policy?notice={notice}")))
}

/// POST /policy/bindings/{id}/update — update a binding's power level and remove flag.
pub async fn update(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<UpdateBindingForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let power_level = form.power_level.and_then(|s| s.trim().parse::<i64>().ok());

    let allow_remove = form.allow_remove.is_some();

    let updated = state
        .policy_service
        .update_binding(
            &id,
            power_level,
            allow_remove,
            &state.audit,
            &admin.subject,
            &admin.username,
        )
        .await?;

    let redirect = if updated {
        let notice = pct_encode("Binding updated");
        format!("/policy?notice={notice}")
    } else {
        let warning = pct_encode("Binding not found");
        format!("/policy?warning={warning}")
    };

    Ok(Redirect::to(&redirect))
}

/// POST /policy/bindings/{id}/delete — delete a policy binding.
pub async fn delete(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<CsrfForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let deleted = state
        .policy_service
        .delete_binding(&id, &state.audit, &admin.subject, &admin.username)
        .await?;

    let redirect = if deleted {
        let notice = pct_encode("Binding deleted");
        format!("/policy?notice={notice}")
    } else {
        let warning = pct_encode("Binding not found");
        format!("/policy?warning={warning}")
    };

    Ok(Redirect::to(&redirect))
}

/// POST /policy/rooms/refresh — refresh the room cache from Synapse.
pub async fn refresh_rooms(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Form(form): Form<CsrfForm>,
) -> Result<impl IntoResponse, AppError> {
    validate(&admin.csrf_token, &form._csrf)?;

    let synapse = state
        .synapse
        .as_ref()
        .ok_or_else(|| AppError::NotFound("Synapse is not configured".into()))?;

    let count = state
        .policy_service
        .refresh_room_cache(synapse.as_ref())
        .await?;

    let notice = pct_encode(&format!("Refreshed {count} rooms from Synapse"));
    Ok(Redirect::to(&format!("/policy?notice={notice}")))
}

/// GET /policy/api/groups — HTML fragment of `<option>` elements for Keycloak groups.
pub async fn api_groups(
    AuthenticatedAdmin(_admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    match state.keycloak.list_groups().await {
        Ok(groups) => {
            let mut html = String::from(r#"<option value="">Select a group…</option>"#);
            for g in groups {
                html.push_str(&format!(
                    r#"<option value="{name}">{name}</option>"#,
                    name = g.name
                ));
            }
            Ok(Html(html))
        }
        Err(_) => Ok(Html(
            r#"<option value="" disabled>Failed to load groups — try again</option>"#.to_string(),
        )),
    }
}

/// GET /policy/api/roles — HTML fragment of `<option>` elements for Keycloak realm roles.
pub async fn api_roles(
    AuthenticatedAdmin(_admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    match state.keycloak.list_realm_roles().await {
        Ok(roles) => {
            let mut html = String::from(r#"<option value="">Select a role…</option>"#);
            for r in roles {
                html.push_str(&format!(
                    r#"<option value="{name}">{name}</option>"#,
                    name = r.name
                ));
            }
            Ok(Html(html))
        }
        Err(_) => Ok(Html(
            r#"<option value="" disabled>Failed to load roles — try again</option>"#.to_string(),
        )),
    }
}

/// GET /policy/api/rooms — HTML fragment of `<option>` elements for cached rooms/spaces.
pub async fn api_rooms(
    AuthenticatedAdmin(_admin): AuthenticatedAdmin,
    State(state): State<AppState>,
) -> Result<Html<String>, AppError> {
    let rooms = state.policy_service.list_cached_rooms().await?;
    if rooms.is_empty() {
        return Ok(Html(
            r#"<option value="" disabled>No rooms cached — click Refresh Rooms</option>"#
                .to_string(),
        ));
    }
    let mut html = String::from(r#"<option value="">Select a room…</option>"#);
    for r in rooms {
        let prefix = if r.is_space { "[Space]" } else { "[Room]" };
        let label = match (&r.name, &r.canonical_alias) {
            (Some(name), Some(alias)) => format!("{prefix} {name} ({alias})"),
            (Some(name), None) => format!("{prefix} {name}"),
            (None, Some(alias)) => format!("{prefix} {alias}"),
            (None, None) => format!("{prefix} {}", r.room_id),
        };
        html.push_str(&format!(
            r#"<option value="{room_id}">{label}</option>"#,
            room_id = r.room_id,
        ));
    }
    Ok(Html(html))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::test_helpers::{
        build_test_state, make_auth_cookie, policy_router, MockKeycloak, TEST_CSRF,
    };

    // ── Helpers ──────────────────────────────────────────────────────────────

    async fn get_policy(
        state: crate::state::AppState,
        cookie: Option<String>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(Method::GET).uri("/policy");
        if let Some(c) = cookie {
            builder = builder.header("cookie", c);
        }
        let req = builder.body(Body::empty()).unwrap();
        policy_router(state).oneshot(req).await.unwrap()
    }

    async fn post_form(
        state: crate::state::AppState,
        uri: &str,
        body: &str,
        cookie: Option<String>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri(uri)
            .header("content-type", "application/x-www-form-urlencoded");
        if let Some(c) = cookie {
            builder = builder.header("cookie", c);
        }
        let req = builder.body(Body::from(body.to_string())).unwrap();
        policy_router(state).oneshot(req).await.unwrap()
    }

    // ── GET /policy ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn policy_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_policy(state, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn policy_authenticated_returns_200() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_policy(state, Some(make_auth_cookie(TEST_CSRF))).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── POST /policy/bindings (create) ───────────────────────────────────────

    #[tokio::test]
    async fn create_binding_invalid_csrf_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let body = "_csrf=wrong&subject_type=group&subject_value=staff&target_type=room&target_room_id=!r:t.com";
        let resp = post_form(
            state,
            "/policy/bindings",
            body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_binding_success_redirects() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let body = format!(
            "_csrf={TEST_CSRF}&subject_type=group&subject_value=staff&target_type=room&target_room_id=!r:t.com"
        );
        let resp = post_form(
            state,
            "/policy/bindings",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            loc.starts_with("/policy?notice="),
            "expected redirect to /policy with notice, got {loc}"
        );
    }

    #[tokio::test]
    async fn create_binding_unauthenticated_redirects() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let body = format!(
            "_csrf={TEST_CSRF}&subject_type=group&subject_value=staff&target_type=room&target_room_id=!r:t.com"
        );
        let resp = post_form(state, "/policy/bindings", &body, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    // ── POST /policy/bindings/{id}/delete ────────────────────────────────────

    #[tokio::test]
    async fn delete_binding_invalid_csrf_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = post_form(
            state,
            "/policy/bindings/fake-id/delete",
            "_csrf=wrong",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_nonexistent_binding_redirects_with_warning() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let body = format!("_csrf={TEST_CSRF}");
        let resp = post_form(
            state,
            "/policy/bindings/nonexistent/delete",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            loc.contains("warning="),
            "expected warning in redirect, got {loc}"
        );
    }

    // ── POST /policy/rooms/refresh ───────────────────────────────────────────

    #[tokio::test]
    async fn refresh_rooms_without_synapse_returns_404() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let body = format!("_csrf={TEST_CSRF}");
        let resp = post_form(
            state,
            "/policy/rooms/refresh",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn refresh_rooms_invalid_csrf_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = post_form(
            state,
            "/policy/rooms/refresh",
            "_csrf=wrong",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── POST /policy/bindings/{id}/update ────────────────────────────────────

    #[tokio::test]
    async fn update_binding_invalid_csrf_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = post_form(
            state,
            "/policy/bindings/fake-id/update",
            "_csrf=wrong",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn update_nonexistent_binding_redirects_with_warning() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let body = format!("_csrf={TEST_CSRF}&power_level=50");
        let resp = post_form(
            state,
            "/policy/bindings/nonexistent/update",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            loc.contains("warning="),
            "expected warning in redirect, got {loc}"
        );
    }

    // ── End-to-end: create then delete ───────────────────────────────────────

    #[tokio::test]
    async fn create_then_list_shows_binding() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;

        // Create a binding via the service directly (simpler than parsing redirects).
        use crate::models::policy_binding::{PolicySubject, PolicyTarget};
        state
            .policy_service
            .create_binding(
                &PolicySubject::Group("staff".into()),
                &PolicyTarget::Room("!room1:test.com".into()),
                None,
                false,
                &state.audit,
                "sub",
                "admin",
            )
            .await
            .unwrap();

        let resp = get_policy(state, Some(make_auth_cookie(TEST_CSRF))).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let bytes = http_body_util::BodyExt::collect(resp.into_body())
            .await
            .unwrap()
            .to_bytes();
        let body = String::from_utf8_lossy(&bytes);
        assert!(
            body.contains("staff"),
            "expected 'staff' in policy page body"
        );
        assert!(
            body.contains("!room1:test.com"),
            "expected room ID in policy page body"
        );
    }
}
