use std::sync::Arc;

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use sqlx::SqlitePool;

use crate::{
    auth::oidc::OidcClient,
    clients::{AuthService, KeycloakIdentityProvider, MatrixService},
    config::Config,
    models::policy::PolicyEngine,
    services::{AuditService, PolicyService, UserService},
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub oidc: Arc<OidcClient>,
    pub keycloak: Arc<dyn KeycloakIdentityProvider>,
    pub mas: Arc<dyn AuthService>,
    /// Optional Synapse connector. `None` when `SYNAPSE_*` env vars are absent.
    pub synapse: Option<Arc<dyn MatrixService>>,
    pub users: Arc<UserService>,
    pub audit: Arc<AuditService>,
    /// Group → room membership policy built from `GROUP_MAPPINGS` config at startup.
    pub policy: Arc<PolicyEngine>,
    /// Dynamic policy binding service backed by SQLite.
    pub policy_service: Arc<PolicyService>,
    /// Encryption key for `PrivateCookieJar`.
    pub cookie_key: Key,
}

/// Allow axum-extra's `PrivateCookieJar` to extract the key from app state.
impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}
