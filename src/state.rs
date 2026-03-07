use std::sync::Arc;

use axum::extract::FromRef;
use axum_extra::extract::cookie::Key;
use sqlx::SqlitePool;

use crate::{
    auth::oidc::OidcClient,
    clients::{KeycloakApi, MasApi},
    config::Config,
    services::{AuditService, UserService},
};

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub db: SqlitePool,
    pub oidc: Arc<OidcClient>,
    pub keycloak: Arc<dyn KeycloakApi>,
    pub mas: Arc<dyn MasApi>,
    pub users: Arc<UserService>,
    pub audit: Arc<AuditService>,
    /// Encryption key for `PrivateCookieJar`.
    pub cookie_key: Key,
}

/// Allow axum-extra's `PrivateCookieJar` to extract the key from app state.
impl FromRef<AppState> for Key {
    fn from_ref(state: &AppState) -> Self {
        state.cookie_key.clone()
    }
}
