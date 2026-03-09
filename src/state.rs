use std::sync::Arc;

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use sqlx::SqlitePool;

use crate::{
    auth::oidc::OidcClient,
    clients::{AuthService, IdentityProvider, MatrixService, RoomManagementApi},
    config::Config,
    models::policy::PolicyEngine,
    services::{AuditService, UserService},
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub oidc: Arc<OidcClient>,
    pub keycloak: Arc<dyn IdentityProvider>,
    pub mas: Arc<dyn AuthService>,
    /// Optional Synapse connector. `None` when `SYNAPSE_*` env vars are absent.
    pub synapse: Option<Arc<dyn MatrixService>>,
    /// Room membership enforcement abstraction, backed by `SynapseClient` when
    /// Synapse is configured. `None` when Synapse is not configured — reconciliation
    /// is disabled in that case.
    pub room_mgmt: Option<Arc<dyn RoomManagementApi>>,
    pub users: Arc<UserService>,
    pub audit: Arc<AuditService>,
    /// Group → room membership policy built from `GROUP_MAPPINGS` config at startup.
    pub policy: Arc<PolicyEngine>,
    /// Encryption key for `PrivateCookieJar`.
    pub cookie_key: Key,
}

/// Allow axum-extra's `PrivateCookieJar` to extract the key from app state.
impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}
