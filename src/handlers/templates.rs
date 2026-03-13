use askama::Template;
use axum::{
    extract::{Query, State},
    response::{Html, Redirect},
    Form,
};
use serde::Deserialize;

use crate::{
    auth::session::AuthenticatedAdmin,
    error::AppError,
    models::onboarding_template::{load_templates, save_templates, OnboardingTemplate},
    state::AppState,
};

#[derive(Deserialize)]
pub struct ListQuery {
    pub notice: Option<String>,
}

#[derive(Template)]
#[template(path = "templates.html")]
struct TemplatesTemplate {
    username: String,
    csrf_token: String,
    current_path: String,
    templates: Vec<OnboardingTemplate>,
    notice: Option<String>,
}

/// GET /templates — list all onboarding templates.
pub async fn list(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Query(query): Query<ListQuery>,
) -> Result<Html<String>, AppError> {
    let templates = load_templates(&state.config.templates_path()).unwrap_or_default();
    let html = TemplatesTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        current_path: "/templates".to_string(),
        templates,
        notice: query.notice,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;
    Ok(Html(html))
}

#[derive(Deserialize)]
pub struct CreateForm {
    pub _csrf: String,
    pub name: String,
    pub description: String,
    pub groups: String,
    pub roles: String,
}

/// POST /templates — create a new onboarding template.
pub async fn create(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Form(form): Form<CreateForm>,
) -> Result<Redirect, AppError> {
    crate::auth::csrf::validate(&admin.csrf_token, &form._csrf)?;

    let name = form.name.trim().to_string();
    if name.is_empty() {
        return Ok(Redirect::to("/templates?notice=Template+name+is+required"));
    }

    let path = state.config.templates_path();
    let mut templates =
        load_templates(&path).map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?;

    if templates.iter().any(|t| t.name == name) {
        return Ok(Redirect::to(
            "/templates?notice=Template+name+already+exists",
        ));
    }

    let groups: Vec<String> = form
        .groups
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let roles: Vec<String> = form
        .roles
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    templates.push(OnboardingTemplate {
        name: name.clone(),
        description: form.description,
        groups: groups.clone(),
        roles: roles.clone(),
    });

    save_templates(&path, &templates).map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?;

    state
        .audit
        .log(
            &admin.subject,
            &admin.username,
            None,
            None,
            "create_onboarding_template",
            crate::models::audit::AuditResult::Success,
            serde_json::json!({
                "template_name": &name,
                "groups": &groups,
                "roles": &roles,
            }),
        )
        .await?;

    Ok(Redirect::to("/templates?notice=Template+created"))
}

#[derive(Deserialize)]
pub struct DeleteForm {
    pub _csrf: String,
    pub name: String,
}

/// POST /templates/delete — delete an onboarding template by name.
pub async fn delete(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Form(form): Form<DeleteForm>,
) -> Result<Redirect, AppError> {
    crate::auth::csrf::validate(&admin.csrf_token, &form._csrf)?;

    let path = state.config.templates_path();
    let mut templates =
        load_templates(&path).map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?;
    templates.retain(|t| t.name != form.name);
    save_templates(&path, &templates).map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?;

    state
        .audit
        .log(
            &admin.subject,
            &admin.username,
            None,
            None,
            "delete_onboarding_template",
            crate::models::audit::AuditResult::Success,
            serde_json::json!({
                "template_name": &form.name,
            }),
        )
        .await?;

    Ok(Redirect::to("/templates?notice=Template+deleted"))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    use crate::test_helpers::{
        build_test_state_full, make_auth_cookie, templates_router, MockKeycloak, MockMas, TEST_CSRF,
    };

    /// Build test state with `onboarding_templates_file` pointed at `path`.
    async fn state_with_file(path: &str) -> crate::state::AppState {
        let mut state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let mut config = (*state.config).clone();
        config.onboarding_templates_file = Some(path.to_string());
        state.config = Arc::new(config);
        state
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
        templates_router(state).oneshot(req).await.unwrap()
    }

    async fn get_templates(
        state: crate::state::AppState,
        cookie: Option<String>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(Method::GET).uri("/templates");
        if let Some(c) = cookie {
            builder = builder.header("cookie", c);
        }
        let req = builder.body(Body::empty()).unwrap();
        templates_router(state).oneshot(req).await.unwrap()
    }

    // ── GET /templates ──────────────────────────────────────────────────

    #[tokio::test]
    async fn list_unauthenticated_redirects_to_login() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let resp = get_templates(state, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn list_returns_200() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let resp = get_templates(state, Some(make_auth_cookie(TEST_CSRF))).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn list_shows_templates_on_disk() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            f.path(),
            r#"[{"name":"Staff","description":"Full access","groups":["staff"],"roles":["admin"]}]"#,
        )
        .unwrap();

        let state = state_with_file(f.path().to_str().unwrap()).await;
        let resp = get_templates(state, Some(make_auth_cookie(TEST_CSRF))).await;
        assert_eq!(resp.status(), StatusCode::OK);

        let body = resp.into_body().collect().await.unwrap().to_bytes();
        let html = String::from_utf8_lossy(&body);
        assert!(
            html.contains("Staff"),
            "expected template name in response body"
        );
    }

    // ── POST /templates (create) ────────────────────────────────────────

    #[tokio::test]
    async fn create_invalid_csrf_returns_400() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let resp = post_form(
            state,
            "/templates",
            "_csrf=wrong&name=Staff&description=Full+access&groups=staff&roles=admin",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn create_unauthenticated_redirects_to_login() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let body = format!(
            "_csrf={TEST_CSRF}&name=Staff&description=Full+access&groups=staff&roles=admin"
        );
        let resp = post_form(state, "/templates", &body, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn create_success_redirects_with_notice() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), "[]").unwrap();

        let state = state_with_file(f.path().to_str().unwrap()).await;
        let body = format!(
            "_csrf={TEST_CSRF}&name=Staff&description=Full+access&groups=staff&roles=admin"
        );
        let resp = post_form(
            state,
            "/templates",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            loc.contains("notice=Template+created"),
            "expected created notice in redirect, got {loc}"
        );

        // Verify template was persisted
        let templates = crate::models::onboarding_template::load_templates(f.path()).unwrap();
        assert_eq!(templates.len(), 1);
        assert_eq!(templates[0].name, "Staff");
    }

    #[tokio::test]
    async fn create_duplicate_name_redirects_with_notice() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            f.path(),
            r#"[{"name":"Staff","description":"Existing","groups":[],"roles":[]}]"#,
        )
        .unwrap();

        let state = state_with_file(f.path().to_str().unwrap()).await;
        let body = format!("_csrf={TEST_CSRF}&name=Staff&description=New+desc&groups=g1&roles=r1");
        let resp = post_form(
            state,
            "/templates",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            loc.contains("already+exists"),
            "expected duplicate notice in redirect, got {loc}"
        );
    }

    #[tokio::test]
    async fn create_empty_name_redirects_with_notice() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), "[]").unwrap();

        let state = state_with_file(f.path().to_str().unwrap()).await;
        let body = format!("_csrf={TEST_CSRF}&name=+&description=Desc&groups=g&roles=r");
        let resp = post_form(
            state,
            "/templates",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            loc.contains("name+is+required"),
            "expected name-required notice in redirect, got {loc}"
        );
    }

    #[tokio::test]
    async fn create_corrupt_file_returns_error() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(f.path(), "not valid json").unwrap();

        let state = state_with_file(f.path().to_str().unwrap()).await;
        let body = format!("_csrf={TEST_CSRF}&name=Staff&description=Desc&groups=g&roles=r");
        let resp = post_form(
            state,
            "/templates",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    // ── POST /templates/delete ──────────────────────────────────────────

    #[tokio::test]
    async fn delete_invalid_csrf_returns_400() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let resp = post_form(
            state,
            "/templates/delete",
            "_csrf=wrong&name=Staff",
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_unauthenticated_redirects_to_login() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let body = format!("_csrf={TEST_CSRF}&name=Staff");
        let resp = post_form(state, "/templates/delete", &body, None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn delete_success_redirects_with_notice() {
        let f = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            f.path(),
            r#"[{"name":"Staff","description":"Full access","groups":["staff"],"roles":["admin"]}]"#,
        )
        .unwrap();

        let state = state_with_file(f.path().to_str().unwrap()).await;
        let body = format!("_csrf={TEST_CSRF}&name=Staff");
        let resp = post_form(
            state,
            "/templates/delete",
            &body,
            Some(make_auth_cookie(TEST_CSRF)),
        )
        .await;

        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        let loc = resp.headers().get("location").unwrap().to_str().unwrap();
        assert!(
            loc.contains("notice=Template+deleted"),
            "expected deleted notice in redirect, got {loc}"
        );

        // Verify template was removed from disk
        let templates = crate::models::onboarding_template::load_templates(f.path()).unwrap();
        assert!(templates.is_empty(), "template should have been deleted");
    }
}
