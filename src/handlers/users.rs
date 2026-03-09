use askama::Template;
use axum::{
    extract::{Path, Query, State},
    response::Html,
};
use serde::Deserialize;

use crate::{
    auth::session::AuthenticatedAdmin,
    error::AppError,
    models::unified::{UnifiedUserDetail, UnifiedUserSummary},
    state::AppState,
};

// ── Search ────────────────────────────────────────────────────────────────────

const PAGE_SIZE: u32 = 25;

#[derive(Deserialize)]
pub struct SearchParams {
    pub q: Option<String>,
    pub page: Option<u32>,
}

#[derive(Template)]
#[template(path = "users_search.html")]
struct SearchTemplate {
    username: String,
    csrf_token: String,
    query: String,
    results: Vec<UnifiedUserSummary>,
    page: u32,
    total_pages: u32,
}

pub async fn search(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Query(params): Query<SearchParams>,
) -> Result<Html<String>, AppError> {
    let query = params.q.unwrap_or_default();
    let page = params.page.unwrap_or(1).max(1);

    let (results, total_pages) = if query.is_empty() {
        (vec![], 1)
    } else {
        let first = (page - 1) * PAGE_SIZE;
        let total = state.keycloak.count_users(&query).await?;
        let total_pages = total.div_ceil(PAGE_SIZE).max(1);
        let results = state.users.search(&query, PAGE_SIZE, first).await?;
        (results, total_pages)
    };

    let html = SearchTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        query,
        results,
        page,
        total_pages,
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

    Ok(Html(html))
}

// ── Detail ────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DetailQuery {
    pub notice: Option<String>,
    pub warning: Option<String>,
}

#[derive(Template)]
#[template(path = "user_detail.html")]
struct DetailTemplate {
    username: String,
    csrf_token: String,
    user: UnifiedUserDetail,
    audit_logs: Vec<AuditEntry>,
    notice: Option<String>,
    warning: Option<String>,
    synapse_enabled: bool,
}

struct AuditEntry {
    pub timestamp: String,
    pub action: String,
    pub result: String,
    pub admin_username: String,
}

pub async fn detail(
    AuthenticatedAdmin(admin): AuthenticatedAdmin,
    State(state): State<AppState>,
    Path(keycloak_id): Path<String>,
    Query(query): Query<DetailQuery>,
) -> Result<Html<String>, AppError> {
    let (user, audit_logs) = tokio::join!(
        state.users.get_detail(&keycloak_id),
        state.audit.for_user(&keycloak_id, 20),
    );

    let user = user?;
    let audit_logs = audit_logs
        .unwrap_or_default()
        .into_iter()
        .map(|l| AuditEntry {
            timestamp: l.timestamp,
            action: l.action,
            result: l.result,
            admin_username: l.admin_username,
        })
        .collect();

    let html = DetailTemplate {
        username: admin.username,
        csrf_token: admin.csrf_token,
        user,
        audit_logs,
        notice: query.notice,
        warning: query.warning,
        synapse_enabled: state.synapse.is_some(),
    }
    .render()
    .map_err(|e| AppError::Internal(anyhow::anyhow!("Template error: {e}")))?;

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
        models::{
            audit::AuditResult,
            keycloak::KeycloakUser,
            mas::{MasSession, MasUser},
        },
        test_helpers::{
            build_test_state_full, make_auth_cookie, reads_router, MockKeycloak, MockMas, TEST_CSRF,
        },
    };

    fn alice() -> KeycloakUser {
        KeycloakUser {
            id: "kc-alice".to_string(),
            username: "alice".to_string(),
            email: Some("alice@example.com".to_string()),
            first_name: Some("Alice".to_string()),
            last_name: Some("Smith".to_string()),
            enabled: true,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    async fn get_search(
        state: crate::state::AppState,
        query: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let uri = if query.is_empty() {
            "/users/search".to_string()
        } else {
            format!("/users/search?q={query}")
        };
        let mut builder = Request::builder().method(Method::GET).uri(uri);
        if let Some(cookie) = auth_cookie {
            builder = builder.header("cookie", cookie);
        }
        reads_router(state)
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    async fn get_detail(
        state: crate::state::AppState,
        user_id: &str,
        auth_cookie: Option<&str>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(Method::GET)
            .uri(format!("/users/{user_id}"));
        if let Some(cookie) = auth_cookie {
            builder = builder.header("cookie", cookie);
        }
        reads_router(state)
            .oneshot(builder.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    // ── Search ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn search_unauthenticated_redirects_to_login() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let resp = get_search(state, "alice", None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn search_empty_query_returns_200() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = get_search(state, "", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn search_with_results_returns_200() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![alice()],
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = get_search(state, "alice", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Detail ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn detail_unauthenticated_redirects_to_login() {
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let resp = get_detail(state, "kc-alice", None).await;
        assert_eq!(resp.status(), StatusCode::SEE_OTHER);
        assert_eq!(resp.headers().get("location").unwrap(), "/auth/login");
    }

    #[tokio::test]
    async fn detail_user_not_found_returns_404() {
        // MockKeycloak with no users → get_user returns NotFound.
        let state =
            build_test_state_full(MockKeycloak::default(), MockMas::default(), "secret", None)
                .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = get_detail(state, "nonexistent", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn detail_returns_200_for_known_user() {
        let state = build_test_state_full(
            MockKeycloak {
                users: vec![alice()],
                ..Default::default()
            },
            MockMas::default(),
            "secret",
            None,
        )
        .await;
        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = get_detail(state, "kc-alice", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn detail_with_mas_user_sessions_and_audit_logs_returns_200() {
        // Covers: AuditEntry mapping (lines 100-105), MockMas.list_sessions (test_helpers),
        // and finished session state in user_service.
        let keycloak = MockKeycloak {
            users: vec![KeycloakUser {
                id: "kc-detail".to_string(),
                username: "alice".to_string(),
                email: Some("alice@test.com".to_string()),
                first_name: None,
                last_name: None,
                enabled: true,
                email_verified: true,
                created_timestamp: None,
                required_actions: vec![],
            }],
            ..Default::default()
        };
        let mas = MockMas {
            user: Some(MasUser {
                id: "mas-alice".to_string(),
                username: "alice".to_string(),
                deactivated_at: None,
            }),
            sessions: vec![MasSession {
                id: "s1".to_string(),
                session_type: "compat".to_string(),
                created_at: None,
                last_active_at: None,
                user_agent: None,
                ip_address: None,
                finished_at: Some("2026-01-01T00:00:00Z".to_string()),
            }],
            ..Default::default()
        };

        let state = build_test_state_full(keycloak, mas, "secret", None).await;
        state
            .audit
            .log(
                "sub",
                "admin",
                Some("kc-detail"),
                Some("@alice:test.com"),
                "detail_action",
                AuditResult::Success,
                serde_json::json!({}),
            )
            .await
            .unwrap();

        let cookie = make_auth_cookie(TEST_CSRF);
        let resp = get_detail(state, "kc-detail", Some(&cookie)).await;
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
