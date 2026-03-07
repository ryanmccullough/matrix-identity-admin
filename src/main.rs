use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};
use axum_extra::extract::cookie::Key;
use sha2::{Digest, Sha512};
use tower_http::timeout::TimeoutLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

mod auth;
mod clients;
mod config;
mod db;
mod error;
mod handlers;
mod models;
mod services;
mod state;
#[cfg(test)]
mod test_helpers;

use clients::{KeycloakClient, MasClient};
use config::Config;
use services::{AuditService, UserService};
use state::AppState;

#[tokio::main]
async fn main() {
    // Load .env file if present (ignored if missing).
    dotenvy::dotenv().ok();

    // Initialise structured logging.
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with(tracing_subscriber::fmt::layer())
        .init();

    let config = Config::from_env();

    tracing::info!(bind_addr = %config.bind_addr, "Starting matrix-identity-admin");

    // ── Database ──────────────────────────────────────────────────────────────
    let pool = db::connect(&config.database_url)
        .await
        .expect("Failed to connect to database");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("Failed to run database migrations");

    // ── Upstream clients ──────────────────────────────────────────────────────
    let keycloak: Arc<dyn clients::KeycloakApi> =
        Arc::new(KeycloakClient::new(config.keycloak.clone()));
    let mas: Arc<dyn clients::MasApi> = Arc::new(MasClient::new(config.mas.clone()));

    // ── OIDC client (discovery happens here) ─────────────────────────────────
    let oidc = auth::oidc::OidcClient::init(&config.oidc, &config.required_admin_role)
        .await
        .expect("OIDC initialisation failed");

    // ── Services ──────────────────────────────────────────────────────────────
    let user_service = Arc::new(UserService::new(
        Arc::clone(&keycloak),
        Arc::clone(&mas),
        &config.homeserver_domain,
    ));

    let audit_service = Arc::new(AuditService::new(pool.clone()));

    // ── Cookie encryption key (derived from session secret) ───────────────────
    // Derive a 64-byte key from the session secret via SHA-512 so that
    // APP_SESSION_SECRET can be any length (UUID, passphrase, etc.).
    let key_material = Sha512::digest(config.session_secret.as_bytes());
    let cookie_key = Key::from(&key_material);

    let state = AppState {
        config: Arc::new(config.clone()),
        db: pool,
        oidc: Arc::new(oidc),
        keycloak,
        mas,
        users: user_service,
        audit: audit_service,
        cookie_key,
    };

    // ── Router ────────────────────────────────────────────────────────────────
    let app = Router::new()
        // Auth
        .route("/auth/login", get(handlers::auth::login))
        .route("/auth/callback", get(handlers::auth::callback))
        .route("/auth/logout", post(handlers::auth::logout))
        // Dashboard
        .route("/", get(handlers::dashboard::dashboard))
        // User search & detail
        .route("/users/search", get(handlers::users::search))
        .route("/users/:id", get(handlers::users::detail))
        // Mutations (all POST, CSRF-protected)
        .route(
            "/users/:id/sessions/:session_id/revoke",
            post(handlers::sessions::revoke),
        )
        .route(
            "/users/:id/keycloak/logout",
            post(handlers::devices::force_keycloak_logout),
        )
        // Bot invite API (bearer-token authenticated, no CSRF)
        .route("/api/v1/invites", post(handlers::invite::create_invite))
        // Audit log
        .route("/audit", get(handlers::audit::list))
        .layer(TimeoutLayer::new(std::time::Duration::from_secs(30)))
        .with_state(state);

    let bind_addr = config.bind_addr.clone();
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|_| panic!("Failed to bind to {bind_addr}"));

    tracing::info!("Listening on http://{bind_addr}");

    axum::serve(listener, app)
        .await
        .expect("Server error");
}
