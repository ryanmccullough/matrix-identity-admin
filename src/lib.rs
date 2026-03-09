pub mod auth;
pub mod clients;
pub mod config;
pub mod db;
pub mod error;
pub mod handlers;
pub mod models;
pub mod services;
pub mod state;
pub(crate) mod utils;

#[cfg(test)]
pub mod test_helpers;

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};
use axum_extra::extract::cookie::Key;
use sha2::{Digest, Sha512};
use tower_http::{services::ServeDir, timeout::TimeoutLayer};

use clients::{IdentityProviderApi, KeycloakClient, MasClient, SynapseClient};
use config::Config;
use models::policy::PolicyEngine;
use services::{AuditService, UserService};
use state::AppState;

/// Build a fully-initialised [`AppState`] against real upstream services.
///
/// Connects to the database (running migrations), fetches OIDC discovery, and
/// wires up Keycloak and MAS clients. Used by both `main` and integration tests.
pub async fn build_state(config: &Config) -> anyhow::Result<AppState> {
    let pool = db::connect(&config.database_url).await?;

    sqlx::migrate!("./migrations").run(&pool).await?;

    let keycloak: Arc<dyn clients::KeycloakApi> =
        Arc::new(KeycloakClient::new(config.keycloak.clone()));
    // A second KeycloakClient instance used as IdentityProviderApi by UserService.
    // KeycloakClient is cheap to construct (shared HTTP client, lazy token fetch).
    let identity_provider: Arc<dyn IdentityProviderApi> =
        Arc::new(KeycloakClient::new(config.keycloak.clone()));
    let mas: Arc<dyn clients::MasApi> = Arc::new(MasClient::new(config.mas.clone()));
    let synapse: Option<Arc<dyn clients::SynapseApi>> = config
        .synapse
        .as_ref()
        .map(|c| -> Arc<dyn clients::SynapseApi> { Arc::new(SynapseClient::new(c.clone())) });

    let oidc = auth::oidc::OidcClient::init(&config.oidc, &config.required_admin_role).await?;

    let users = Arc::new(UserService::new(
        identity_provider,
        Arc::clone(&mas),
        &config.homeserver_domain,
    ));
    let audit = Arc::new(AuditService::new(pool.clone()));

    let policy = Arc::new(PolicyEngine::new(config.group_mappings.clone()));

    let key_material = Sha512::digest(config.session_secret.as_bytes());
    let cookie_key = Key::from(&key_material);

    Ok(AppState {
        config: Arc::new(config.clone()),
        db: pool,
        oidc: Arc::new(oidc),
        keycloak,
        mas,
        synapse,
        users,
        audit,
        policy,
        cookie_key,
    })
}

/// Construct the full application router from an already-built [`AppState`].
pub fn build_router(state: AppState) -> Router {
    Router::new()
        // Auth
        .route("/auth/login", get(handlers::auth::login))
        .route("/auth/callback", get(handlers::auth::callback))
        .route("/auth/logout", post(handlers::auth::logout))
        // Static assets
        .nest_service("/static", ServeDir::new("static"))
        // Dashboard
        .route("/", get(handlers::dashboard::dashboard))
        .route("/status", get(handlers::dashboard::status))
        // User search & detail
        .route("/users/search", get(handlers::users::search))
        .route("/users/{id}", get(handlers::users::detail))
        // Mutations (all POST, CSRF-protected)
        .route(
            "/users/{id}/sessions/{session_id}/revoke",
            post(handlers::sessions::revoke),
        )
        .route(
            "/users/{id}/keycloak/logout",
            post(handlers::devices::force_keycloak_logout),
        )
        .route(
            "/users/{id}/delete",
            post(handlers::delete::delete_user_handler),
        )
        .route("/users/{id}/disable", post(handlers::disable::disable))
        .route(
            "/users/{id}/reconcile",
            post(handlers::reconcile::reconcile),
        )
        .route(
            "/users/{id}/reconcile/preview",
            post(handlers::reconcile::reconcile_preview),
        )
        .route(
            "/users/reconcile/all",
            post(handlers::bulk_reconcile::bulk_reconcile),
        )
        // Admin invite (OIDC session + CSRF)
        .route("/users/invite", post(handlers::invite::admin_invite))
        // Bot invite API (bearer-token authenticated, no CSRF)
        .route("/api/v1/invites", post(handlers::invite::create_invite))
        // Audit log
        .route("/audit", get(handlers::audit::list))
        .layer(TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(30),
        ))
        .with_state(state)
}
