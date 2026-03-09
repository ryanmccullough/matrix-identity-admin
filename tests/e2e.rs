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

use std::sync::OnceLock;

use matrix_identity_admin::{
    build_router, build_state, clients::KeycloakIdentityProvider, config::Config,
};

// ── Test server ────────────────────────────────────────────────────────────────

struct TestServer {
    pub base_url: String,
    pub client: reqwest::Client,
    pub bot_secret: String,
    pub config: Config,
    // Keep the task alive — dropped when TestServer is dropped.
    _handle: tokio::task::JoinHandle<()>,
}

/// Shared setup that runs once per test process.
/// Holds the admin access token and room IDs created during setup.
#[allow(dead_code)]
struct SynapseSetup {
    admin_token: String,
    staff_room_id: String,
    eng_general_room_id: String,
    eng_random_room_id: String,
    eng_space_id: String,
}

static SYNAPSE_SETUP: OnceLock<SynapseSetup> = OnceLock::new();

/// Load the e2e `.env` and override DATABASE_URL with in-memory SQLite so
/// tests don't pollute or depend on a persistent audit-log file.
fn load_e2e_env() {
    dotenvy::from_path("e2e/.env").ok();
    // Use an in-memory audit DB for test isolation.
    std::env::set_var("DATABASE_URL", "sqlite::memory:");
}

/// URL-encode a Matrix ID for use in URL paths.
fn urlencoded(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '@' => "%40".chars().collect::<Vec<_>>(),
            ':' => "%3A".chars().collect::<Vec<_>>(),
            '!' => "%21".chars().collect::<Vec<_>>(),
            _ => vec![c],
        })
        .collect()
}

/// Register the admin user in Synapse using the admin_token from MSC3861 config.
/// Uses PUT /_synapse/admin/v2/users/@admin:e2e.test to upsert the user.
/// Then logs in via m.login.password to get an access token for room operations.
async fn register_synapse_admin(client: &reqwest::Client) -> String {
    let synapse_url =
        std::env::var("SYNAPSE_BASE_URL").unwrap_or_else(|_| "http://localhost:8008".to_string());
    let admin_token = "e2e-admin-token"; // matches homeserver.yaml msc3861.admin_token

    // Upsert admin user via admin API
    let resp = client
        .put(format!(
            "{synapse_url}/_synapse/admin/v2/users/%40admin%3Ae2e.test"
        ))
        .header("authorization", format!("Bearer {admin_token}"))
        .json(&serde_json::json!({
            "password": "AdminPass2026!",
            "admin": true,
            "displayname": "E2E Admin"
        }))
        .send()
        .await
        .expect("Synapse not reachable — is the Docker stack running?");

    assert!(
        resp.status().is_success(),
        "Failed to create Synapse admin user: {}",
        resp.status()
    );

    // Login via m.login.password to get an access token
    let login_resp: serde_json::Value = client
        .post(format!("{synapse_url}/_matrix/client/v3/login"))
        .json(&serde_json::json!({
            "type": "m.login.password",
            "identifier": {
                "type": "m.id.user",
                "user": "@admin:e2e.test"
            },
            "password": "AdminPass2026!"
        }))
        .send()
        .await
        .expect("Synapse login failed")
        .json()
        .await
        .expect("Failed to parse login response");

    login_resp["access_token"]
        .as_str()
        .expect("No access_token in login response")
        .to_string()
}

/// Create a room via the Matrix client API. Returns the room ID.
async fn create_room(
    client: &reqwest::Client,
    synapse_url: &str,
    token: &str,
    alias_localpart: &str,
    name: &str,
    is_space: bool,
) -> String {
    let mut body = serde_json::json!({
        "room_alias_name": alias_localpart,
        "name": name,
        "visibility": "private",
    });

    if is_space {
        body["creation_content"] = serde_json::json!({
            "type": "m.space"
        });
    }

    let resp: serde_json::Value = client
        .post(format!("{synapse_url}/_matrix/client/v3/createRoom"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .expect("Failed to create room")
        .json()
        .await
        .expect("Failed to parse createRoom response");

    resp["room_id"]
        .as_str()
        .expect("No room_id in createRoom response")
        .to_string()
}

/// Add a child room to a space via m.space.child state event.
async fn add_space_child(
    client: &reqwest::Client,
    synapse_url: &str,
    token: &str,
    space_id: &str,
    child_id: &str,
) {
    let encoded_space = urlencoded(space_id);
    let encoded_child = urlencoded(child_id);

    let resp = client
        .put(format!(
            "{synapse_url}/_matrix/client/v3/rooms/{encoded_space}/state/m.space.child/{encoded_child}"
        ))
        .bearer_auth(token)
        .json(&serde_json::json!({
            "via": ["e2e.test"]
        }))
        .send()
        .await
        .expect("Failed to add space child");

    assert!(
        resp.status().is_success(),
        "Failed to add space child: {}",
        resp.status()
    );
}

/// Perform one-time Synapse setup: register admin, create rooms, build GROUP_MAPPINGS.
async fn ensure_synapse_setup() -> &'static SynapseSetup {
    if let Some(setup) = SYNAPSE_SETUP.get() {
        return setup;
    }

    let client = reqwest::Client::new();
    let synapse_url =
        std::env::var("SYNAPSE_BASE_URL").unwrap_or_else(|_| "http://localhost:8008".to_string());

    // 1. Register admin user and get access token
    let admin_token = register_synapse_admin(&client).await;

    // 2. Create rooms
    let staff_room_id = create_room(
        &client,
        &synapse_url,
        &admin_token,
        "staff-general",
        "Staff General",
        false,
    )
    .await;
    let eng_general_id = create_room(
        &client,
        &synapse_url,
        &admin_token,
        "eng-general",
        "Engineering General",
        false,
    )
    .await;
    let eng_random_id = create_room(
        &client,
        &synapse_url,
        &admin_token,
        "eng-random",
        "Engineering Random",
        false,
    )
    .await;

    // 3. Create engineering space
    let eng_space_id = create_room(
        &client,
        &synapse_url,
        &admin_token,
        "engineering-space",
        "Engineering",
        true,
    )
    .await;

    // 4. Add children to space
    add_space_child(
        &client,
        &synapse_url,
        &admin_token,
        &eng_space_id,
        &eng_general_id,
    )
    .await;
    add_space_child(
        &client,
        &synapse_url,
        &admin_token,
        &eng_space_id,
        &eng_random_id,
    )
    .await;

    // 5. Build GROUP_MAPPINGS env var
    let group_mappings = serde_json::json!([
        {"keycloak_group": "staff", "matrix_room_id": staff_room_id},
        {"keycloak_group": "engineering", "matrix_room_id": eng_space_id}
    ]);
    std::env::set_var("GROUP_MAPPINGS", group_mappings.to_string());

    let setup = SynapseSetup {
        admin_token,
        staff_room_id,
        eng_general_room_id: eng_general_id,
        eng_random_room_id: eng_random_id,
        eng_space_id,
    };

    SYNAPSE_SETUP.set(setup).ok();
    SYNAPSE_SETUP.get().unwrap()
}

async fn start_server() -> TestServer {
    load_e2e_env();

    // Ensure Synapse is set up (registers admin, creates rooms, sets GROUP_MAPPINGS)
    let _setup = ensure_synapse_setup().await;

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

// ── Synapse setup ────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn synapse_admin_user_registers() {
    load_e2e_env();
    let client = reqwest::Client::new();
    let token = register_synapse_admin(&client).await;
    assert!(!token.is_empty(), "should get a non-empty access token");
}

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn synapse_rooms_created() {
    load_e2e_env();
    let setup = ensure_synapse_setup().await;
    assert!(setup.staff_room_id.starts_with('!'));
    assert!(setup.eng_space_id.starts_with('!'));
    assert!(setup.eng_general_room_id.starts_with('!'));
    assert!(setup.eng_random_room_id.starts_with('!'));
}

// ── Auth & Navigation ────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn dashboard_unauthenticated_redirects() {
    let srv = start_server().await;
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = no_redirect
        .get(format!("{}/", srv.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303, "unauthenticated GET / should redirect");
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(
        location.contains("/auth/login"),
        "should redirect to /auth/login, got: {location}"
    );
}

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn audit_page_unauthenticated_redirects() {
    let srv = start_server().await;
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = no_redirect
        .get(format!("{}/audit", srv.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303);
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    assert!(location.contains("/auth/login"));
}

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn search_unauthenticated_redirects() {
    let srv = start_server().await;
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let resp = no_redirect
        .get(format!("{}/users/search?q=test", srv.base_url))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 303);
}

// ── Invite + Groups ──────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn invited_user_has_no_groups() {
    let srv = start_server().await;
    let email = format!("e2e-groups-{}@e2e.test", uuid::Uuid::new_v4());
    let secret = srv.bot_secret.clone();

    let resp = post_invite(&srv, Some(&secret), &email).await;
    assert_eq!(resp.status(), 201);

    let kc = matrix_identity_admin::clients::KeycloakClient::new(srv.config.keycloak.clone());
    use matrix_identity_admin::clients::KeycloakIdentityProvider;
    let kc_user = kc.get_user_by_email(&email).await.unwrap().unwrap();

    let groups = kc.get_user_groups(&kc_user.id).await.unwrap();
    assert!(
        groups.is_empty(),
        "newly invited user should have no groups"
    );

    cleanup_kc_user(&srv, &email).await;
}

// ── Lifecycle ────────────────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn disable_and_reactivate_user_in_keycloak() {
    let srv = start_server().await;
    let email = format!("e2e-disable-{}@e2e.test", uuid::Uuid::new_v4());
    let secret = srv.bot_secret.clone();

    // Create user via invite
    let resp = post_invite(&srv, Some(&secret), &email).await;
    assert_eq!(resp.status(), 201);

    let kc = matrix_identity_admin::clients::KeycloakClient::new(srv.config.keycloak.clone());
    use matrix_identity_admin::clients::KeycloakIdentityProvider;
    let user = kc.get_user_by_email(&email).await.unwrap().unwrap();
    assert!(user.enabled, "user should be enabled after invite");

    // Disable
    kc.disable_user(&user.id)
        .await
        .expect("disable should succeed");
    let user = kc.get_user_by_email(&email).await.unwrap().unwrap();
    assert!(!user.enabled, "user should be disabled after disable");

    // Re-enable
    kc.enable_user(&user.id)
        .await
        .expect("enable should succeed");
    let user = kc.get_user_by_email(&email).await.unwrap().unwrap();
    assert!(user.enabled, "user should be enabled after reactivate");

    cleanup_kc_user(&srv, &email).await;
}

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn delete_user_removes_from_keycloak() {
    let srv = start_server().await;
    let email = format!("e2e-delete-{}@e2e.test", uuid::Uuid::new_v4());
    let secret = srv.bot_secret.clone();

    let resp = post_invite(&srv, Some(&secret), &email).await;
    assert_eq!(resp.status(), 201);

    let kc = matrix_identity_admin::clients::KeycloakClient::new(srv.config.keycloak.clone());
    use matrix_identity_admin::clients::KeycloakIdentityProvider;
    let user = kc.get_user_by_email(&email).await.unwrap().unwrap();

    kc.delete_user(&user.id)
        .await
        .expect("delete should succeed");

    let result = kc.get_user_by_email(&email).await.unwrap();
    assert!(result.is_none(), "user should not exist after delete");
}

#[tokio::test]
#[ignore = "requires Docker e2e stack — run with: cargo test --test e2e -- --include-ignored"]
async fn force_keycloak_logout_succeeds() {
    let srv = start_server().await;
    let email = format!("e2e-logout-{}@e2e.test", uuid::Uuid::new_v4());
    let secret = srv.bot_secret.clone();

    let resp = post_invite(&srv, Some(&secret), &email).await;
    assert_eq!(resp.status(), 201);

    let kc = matrix_identity_admin::clients::KeycloakClient::new(srv.config.keycloak.clone());
    use matrix_identity_admin::clients::KeycloakIdentityProvider;
    let user = kc.get_user_by_email(&email).await.unwrap().unwrap();

    // Force logout should succeed even with no active sessions
    kc.logout_user(&user.id)
        .await
        .expect("logout should succeed");

    cleanup_kc_user(&srv, &email).await;
}
