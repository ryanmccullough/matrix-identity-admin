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
    models::policy_binding::{PolicyBinding, PolicySubject, PolicyTarget},
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
    room_count: usize,
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

fn parse_optional_power_level(input: Option<String>) -> Result<Option<i64>, AppError> {
    match input {
        None => Ok(None),
        Some(raw) => {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            trimmed
                .parse::<i64>()
                .map(Some)
                .map_err(|_| AppError::Validation("Invalid power level".into()))
        }
    }
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

// ── Handlers ─────────────────────────────────────────────────────────────────

/// GET /policy — render the policy management page.
pub async fn list(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    axum::extract::Query(query): axum::extract::Query<PolicyQuery>,
) -> Result<Html<String>, AppError> {
    let bindings = state.policy_service.list_bindings().await?;
    let room_count = state.policy_service.list_cached_rooms().await?.len();

    let synapse_enabled = state.synapse.is_some();

    let html = PolicyTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        bindings,
        room_count,
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

    let power_level = parse_optional_power_level(form.power_level)?;

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

    let power_level = parse_optional_power_level(form.power_level)?;

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
                let escaped = escape_html(&g.name);
                html.push_str(&format!(
                    r#"<option value="{name}">{name}</option>"#,
                    name = escaped
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
                let escaped = escape_html(&r.name);
                html.push_str(&format!(
                    r#"<option value="{name}">{name}</option>"#,
                    name = escaped
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
        let escaped_room_id = escape_html(&r.room_id);
        let escaped_label = escape_html(&label);
        html.push_str(&format!(
            r#"<option value="{room_id}">{label}</option>"#,
            room_id = escaped_room_id,
            label = escaped_label,
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

    use crate::{
        db::policy::upsert_cached_room,
        models::{
            keycloak::{KeycloakGroup, KeycloakRole},
            policy_binding::CachedRoom,
        },
        test_helpers::{
            build_test_state, make_auth_cookie, policy_router, MockKeycloak, TEST_CSRF,
        },
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

    async fn get_path(
        state: crate::state::AppState,
        uri: &str,
        cookie: Option<String>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(Method::GET).uri(uri);
        if let Some(c) = cookie {
            builder = builder.header("cookie", c);
        }
        let req = builder.body(Body::empty()).unwrap();
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

    #[tokio::test]
    async fn create_binding_invalid_power_level_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let body = format!(
            "_csrf={TEST_CSRF}&subject_type=group&subject_value=staff&target_type=room&target_room_id=!r:t.com&power_level=not-a-number"
        );
        let resp = post_form(
            state,
            "/policy/bindings",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
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

    #[tokio::test]
    async fn update_binding_invalid_power_level_returns_400() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let body = format!("_csrf={TEST_CSRF}&power_level=invalid");
        let resp = post_form(
            state,
            "/policy/bindings/nonexistent/update",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── GET /policy/api/* ────────────────────────────────────────────────────

    #[tokio::test]
    async fn api_groups_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_path(state, "/policy/api/groups", None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn api_rooms_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_path(state, "/policy/api/rooms", None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn api_roles_unauthenticated_redirects_to_login() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_path(state, "/policy/api/roles", None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn api_rooms_empty_cache_returns_disabled_option() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        let resp = get_path(
            state,
            "/policy/api/rooms",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(
            &http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes(),
        )
        .to_string();
        assert!(
            body.contains("No rooms cached"),
            "expected empty-cache message"
        );
    }

    #[tokio::test]
    async fn api_groups_success_returns_options() {
        let state = build_test_state(
            MockKeycloak {
                all_groups: vec![KeycloakGroup {
                    id: "g1".into(),
                    name: "staff".into(),
                    path: "/staff".into(),
                }],
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let resp = get_path(
            state,
            "/policy/api/groups",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(
            &http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes(),
        )
        .to_string();
        assert!(body.contains(r#"<option value="staff">staff</option>"#));
    }

    #[tokio::test]
    async fn api_groups_escapes_html_in_values() {
        let state = build_test_state(
            MockKeycloak {
                all_groups: vec![KeycloakGroup {
                    id: "g1".into(),
                    name: r#"bad"><script>alert(1)</script>"#.into(),
                    path: "/bad".into(),
                }],
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let resp = get_path(
            state,
            "/policy/api/groups",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(
            &http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes(),
        )
        .to_string();
        assert!(!body.contains("<script>alert(1)</script>"));
        assert!(body.contains("&lt;script&gt;alert(1)&lt;/script&gt;"));
    }

    #[tokio::test]
    async fn api_roles_success_returns_options() {
        let state = build_test_state(
            MockKeycloak {
                all_roles: vec![KeycloakRole {
                    id: "r1".into(),
                    name: "matrix-admin".into(),
                    composite: false,
                    client_role: false,
                    container_id: None,
                }],
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let resp = get_path(
            state,
            "/policy/api/roles",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(
            &http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes(),
        )
        .to_string();
        assert!(body.contains(r#"<option value="matrix-admin">matrix-admin</option>"#));
    }

    #[tokio::test]
    async fn api_groups_upstream_failure_returns_disabled_option() {
        let state = build_test_state(
            MockKeycloak {
                fail_list_groups: true,
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let resp = get_path(
            state,
            "/policy/api/groups",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(
            &http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes(),
        )
        .to_string();
        assert!(body.contains("Failed to load groups"));
    }

    #[tokio::test]
    async fn api_roles_upstream_failure_returns_disabled_option() {
        let state = build_test_state(
            MockKeycloak {
                fail_list_roles: true,
                ..Default::default()
            },
            "secret",
            None,
        )
        .await;
        let resp = get_path(
            state,
            "/policy/api/roles",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(
            &http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes(),
        )
        .to_string();
        assert!(body.contains("Failed to load roles"));
    }

    #[tokio::test]
    async fn api_rooms_success_returns_options() {
        let state = build_test_state(MockKeycloak::default(), "secret", None).await;
        upsert_cached_room(
            &state.db,
            &CachedRoom {
                room_id: "!abc:test.com".into(),
                name: Some("General".into()),
                canonical_alias: Some("#general:test.com".into()),
                parent_space_id: None,
                is_space: false,
                last_seen_at: "2026-03-09T00:00:00Z".into(),
            },
        )
        .await
        .unwrap();
        let resp = get_path(
            state,
            "/policy/api/rooms",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::OK);
        let body = String::from_utf8_lossy(
            &http_body_util::BodyExt::collect(resp.into_body())
                .await
                .unwrap()
                .to_bytes(),
        )
        .to_string();
        assert!(body.contains(r#"<option value="!abc:test.com">"#));
        assert!(body.contains("General"));
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
