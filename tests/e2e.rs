//! E2E integration tests for the invite API.
//!
//! These tests start the real application server in-process against the Docker
//! e2e stack (Keycloak + MAS + Mailpit). They are marked `#[ignore]` so normal
//! `cargo test` skips them.
//!
//! ## Running
//!
//! ```sh
//! # 1. Start the Docker stack
//! docker compose -f e2e/docker-compose.yml up -d
//!
//! # 2. Wait for services to be healthy (first run ~60s)
//! docker compose -f e2e/docker-compose.yml ps
//!
//! # 3. Run the e2e tests
//! cargo test --test e2e -- --include-ignored
//! ```
//!
//! The tests load `e2e/.env` automatically. No manual env export needed.

use matrix_identity_admin::{build_router, build_state, clients::KeycloakApi, config::Config};

// ── Test server ────────────────────────────────────────────────────────────────

struct TestServer {
    pub base_url: String,
    pub client: reqwest::Client,
    pub bot_secret: String,
    pub config: Config,
    // Keep the task alive — dropped when TestServer is dropped.
    _handle: tokio::task::JoinHandle<()>,
}

/// Load the e2e `.env` and override DATABASE_URL with in-memory SQLite so
/// tests don't pollute or depend on a persistent audit-log file.
fn load_e2e_env() {
    dotenvy::from_path("e2e/.env").ok();
    // Use an in-memory audit DB for test isolation.
    std::env::set_var("DATABASE_URL", "sqlite::memory:");
}

async fn start_server() -> TestServer {
    load_e2e_env();

    let config = Config::from_env();
    let bot_secret = config.bot_api_secret.clone();

    let state = build_state(&config)
        .await
        .expect("failed to build app state — is the Docker stack running?");

    let app = build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    TestServer {
        base_url: format!("http://{addr}"),
        client: reqwest::Client::new(),
        bot_secret,
        config,
        _handle: handle,
    }
}

/// POST to /api/v1/invites with the given auth header and body.
async fn post_invite(srv: &TestServer, auth: Option<&str>, email: &str) -> reqwest::Response {
    let mut req = srv
        .client
        .post(format!("{}/api/v1/invites", srv.base_url))
        .json(&serde_json::json!({
            "email": email,
            "invited_by": "e2e-test"
        }));

    if let Some(token) = auth {
        req = req.header("authorization", format!("Bearer {token}"));
    }

    req.send().await.unwrap()
}

/// Call the Keycloak admin API to look up a user by email, then delete it.
/// Used for test cleanup. Silently ignores errors (user may not exist).
async fn cleanup_kc_user(srv: &TestServer, email: &str) {
    let kc = matrix_identity_admin::clients::KeycloakClient::new(srv.config.keycloak.clone());
    if let Ok(Some(user)) = kc.get_user_by_email(email).await {
        let _ = kc.delete_user(&user.id).await;
    }
}

// ── Auth tests ─────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn invite_missing_auth_returns_401() {
    let srv = start_server().await;
    let resp = post_invite(&srv, None, "anyone@e2e.test").await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn invite_wrong_secret_returns_401() {
    let srv = start_server().await;
    let resp = post_invite(&srv, Some("wrong-secret"), "anyone@e2e.test").await;
    assert_eq!(resp.status(), 401);
}

// ── Validation tests ───────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn invite_invalid_email_no_at_returns_422() {
    let srv = start_server().await;
    let resp = post_invite(&srv, Some(&srv.bot_secret.clone()), "notanemail").await;
    assert_eq!(resp.status(), 422);
}

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn invite_invalid_email_no_dot_in_domain_returns_422() {
    let srv = start_server().await;
    let resp = post_invite(&srv, Some(&srv.bot_secret.clone()), "user@nodot").await;
    assert_eq!(resp.status(), 422);
}

// ── Happy path tests ───────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn invite_creates_keycloak_user() {
    let srv = start_server().await;
    let email = format!("e2e-{}@e2e.test", uuid::Uuid::new_v4());
    let secret = srv.bot_secret.clone();

    let resp = post_invite(&srv, Some(&secret), &email).await;
    assert_eq!(resp.status(), 201, "invite should return 201");

    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["ok"], true);

    // Verify the user was created in Keycloak.
    let kc = matrix_identity_admin::clients::KeycloakClient::new(srv.config.keycloak.clone());
    let kc_user = kc
        .get_user_by_email(&email)
        .await
        .expect("Keycloak lookup failed");

    assert!(
        kc_user.is_some(),
        "user should exist in Keycloak after invite"
    );
    let kc_user = kc_user.unwrap();
    assert_eq!(kc_user.email.as_deref(), Some(email.as_str()));
    assert!(kc_user.enabled, "invited user should be enabled");

    // Verify Mailpit captured the invite email.
    let mailpit: serde_json::Value = reqwest::get("http://localhost:8025/api/v1/messages")
        .await
        .expect("Mailpit not reachable — is the Docker stack running?")
        .json()
        .await
        .unwrap();
    let total = mailpit["total"].as_i64().unwrap_or(0);
    assert!(
        total > 0,
        "expected Mailpit to have captured at least one email"
    );

    cleanup_kc_user(&srv, &email).await;
}

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn invite_duplicate_email_returns_422() {
    let srv = start_server().await;
    let email = format!("e2e-dup-{}@e2e.test", uuid::Uuid::new_v4());
    let secret = srv.bot_secret.clone();

    // First invite: should succeed.
    let first = post_invite(&srv, Some(&secret), &email).await;
    assert_eq!(first.status(), 201, "first invite should succeed");

    // Second invite with the same email: should be rejected.
    let second = post_invite(&srv, Some(&secret), &email).await;
    assert_eq!(second.status(), 422, "duplicate email should return 422");

    let body: serde_json::Value = second.json().await.unwrap();
    assert_eq!(body["ok"], false);

    cleanup_kc_user(&srv, &email).await;
}

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn invite_email_is_case_insensitive() {
    let srv = start_server().await;
    let id = uuid::Uuid::new_v4();
    let upper_email = format!("E2E-CASE-{id}@E2E.TEST");
    let lower_email = format!("e2e-case-{id}@e2e.test");
    let secret = srv.bot_secret.clone();

    // Invite with upper-case email — app should lowercase before creating.
    let resp = post_invite(&srv, Some(&secret), &upper_email).await;
    assert_eq!(resp.status(), 201);

    // Keycloak user should exist under the lowercased email.
    let kc = matrix_identity_admin::clients::KeycloakClient::new(srv.config.keycloak.clone());
    let kc_user = kc.get_user_by_email(&lower_email).await.unwrap();
    assert!(
        kc_user.is_some(),
        "user should exist under lowercased email"
    );

    cleanup_kc_user(&srv, &lower_email).await;
}
