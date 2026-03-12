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
    let mut templates = load_templates(&path).unwrap_or_default();

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
        name,
        description: form.description,
        groups,
        roles,
    });

    save_templates(&path, &templates).map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?;

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
    let mut templates = load_templates(&path).unwrap_or_default();
    templates.retain(|t| t.name != form.name);
    save_templates(&path, &templates).map_err(|e| AppError::Internal(anyhow::anyhow!("{e}")))?;

    Ok(Redirect::to("/templates?notice=Template+deleted"))
}

#[cfg(test)]
mod tests {
    use axum::{
        body::Body,
        http::{Method, Request, StatusCode},
    };
    use tower::ServiceExt;

    use crate::test_helpers::{
        build_test_state_full, make_auth_cookie, templates_router, MockKeycloak, MockMas, TEST_CSRF,
    };

    // ── GET /templates ──────────────────────────────────────────────────

    #[tokio::test]
    async fn list_unauthenticated_redirects_to_login() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let req = Request::builder()
            .method(Method::GET)
            .uri("/templates")
            .body(Body::empty())
            .unwrap();
        let resp = templates_router(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    }

    #[tokio::test]
    async fn list_returns_200() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let req = Request::builder()
            .method(Method::GET)
            .uri("/templates")
            .header("cookie", cookie)
            .body(Body::empty())
            .unwrap();
        let resp = templates_router(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── POST /templates (create) ────────────────────────────────────────

    #[tokio::test]
    async fn create_invalid_csrf_returns_400() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let body = "_csrf=wrong&name=Staff&description=Full+access&groups=staff&roles=admin";
        let req = Request::builder()
            .method(Method::POST)
            .uri("/templates")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", make_auth_cookie(TEST_CSRF))
            .body(Body::from(body))
            .unwrap();
        let resp = templates_router(state).oneshot(req).await.unwrap();
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
        let req = Request::builder()
            .method(Method::POST)
            .uri("/templates")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        let resp = templates_router(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    // ── POST /templates/delete ──────────────────────────────────────────

    #[tokio::test]
    async fn delete_invalid_csrf_returns_400() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let body = "_csrf=wrong&name=Staff";
        let req = Request::builder()
            .method(Method::POST)
            .uri("/templates/delete")
            .header("content-type", "application/x-www-form-urlencoded")
            .header("cookie", make_auth_cookie(TEST_CSRF))
            .body(Body::from(body))
            .unwrap();
        let resp = templates_router(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn delete_unauthenticated_redirects_to_login() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let body = format!("_csrf={TEST_CSRF}&name=Staff");
        let req = Request::builder()
            .method(Method::POST)
            .uri("/templates/delete")
            .header("content-type", "application/x-www-form-urlencoded")
            .body(Body::from(body))
            .unwrap();
        let resp = templates_router(state).oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }
}
