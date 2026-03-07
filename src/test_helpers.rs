use std::sync::Arc;

use async_trait::async_trait;
use axum::{routing::post, Router};
use axum_extra::extract::cookie::Key;
use sqlx::sqlite::SqlitePoolOptions;

use crate::{
    auth::oidc::OidcClient,
    clients::{KeycloakApi, MasApi},
    config::{Config, KeycloakConfig, MasConfig, OidcConfig},
    error::AppError,
    models::{
        keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
        mas::{MasSession, MasUser},
    },
    services::{AuditService, UserService},
    state::AppState,
};

// ── Mock Keycloak ─────────────────────────────────────────────────────────────

/// Configurable mock for the Keycloak API.
///
/// Defaults to returning empty/successful responses. Set fields to control
/// behaviour in individual tests.
pub struct MockKeycloak {
    /// Users returned by `search_users`.
    pub users: Vec<KeycloakUser>,
    pub groups: Vec<KeycloakGroup>,
    pub roles: Vec<KeycloakRole>,
    /// User returned by `get_user_by_email` (None = no existing user).
    pub user_by_email: Option<KeycloakUser>,
    /// ID returned by `create_user` on success.
    pub create_user_id: String,
    /// If true, `create_user` returns an upstream error.
    pub fail_create: bool,
    /// If true, `send_invite_email` returns an upstream error.
    pub fail_send_invite: bool,
}

impl Default for MockKeycloak {
    fn default() -> Self {
        Self {
            users: vec![],
            groups: vec![],
            roles: vec![],
            user_by_email: None,
            create_user_id: "new-kc-id".to_string(),
            fail_create: false,
            fail_send_invite: false,
        }
    }
}

#[async_trait]
impl KeycloakApi for MockKeycloak {
    async fn search_users(&self, _query: &str) -> Result<Vec<KeycloakUser>, AppError> {
        Ok(self.users.clone())
    }

    async fn get_user(&self, _user_id: &str) -> Result<KeycloakUser, AppError> {
        self.users
            .first()
            .cloned()
            .ok_or_else(|| AppError::NotFound("user not found".into()))
    }

    async fn get_user_by_email(&self, _email: &str) -> Result<Option<KeycloakUser>, AppError> {
        Ok(self.user_by_email.clone())
    }

    async fn get_user_groups(&self, _user_id: &str) -> Result<Vec<KeycloakGroup>, AppError> {
        Ok(self.groups.clone())
    }

    async fn get_user_roles(&self, _user_id: &str) -> Result<Vec<KeycloakRole>, AppError> {
        Ok(self.roles.clone())
    }

    async fn logout_user(&self, _user_id: &str) -> Result<(), AppError> {
        Ok(())
    }

    async fn create_user(&self, _username: &str, _email: &str) -> Result<String, AppError> {
        if self.fail_create {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock create_user failure".into(),
            })
        } else {
            Ok(self.create_user_id.clone())
        }
    }

    async fn send_invite_email(&self, _user_id: &str) -> Result<(), AppError> {
        if self.fail_send_invite {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock send_invite_email failure".into(),
            })
        } else {
            Ok(())
        }
    }
}

// ── Mock MAS ──────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct MockMas {
    pub user: Option<MasUser>,
    pub sessions: Vec<MasSession>,
}

#[async_trait]
impl MasApi for MockMas {
    async fn get_user_by_username(&self, _username: &str) -> Result<Option<MasUser>, AppError> {
        Ok(self.user.clone())
    }

    async fn list_sessions(&self, _mas_user_id: &str) -> Result<Vec<MasSession>, AppError> {
        Ok(self.sessions.clone())
    }

    async fn finish_session(&self, _session_id: &str, _session_type: &str) -> Result<(), AppError> {
        Ok(())
    }
}

// ── State builder ─────────────────────────────────────────────────────────────

/// Build an `AppState` backed by an in-memory SQLite database.
///
/// Uses a pool capped at one connection so that all reads/writes share the
/// same in-memory database instance.
pub async fn build_test_state(
    keycloak: MockKeycloak,
    bot_secret: &str,
    allowed_domains: Option<Vec<String>>,
) -> AppState {
    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect("sqlite::memory:")
        .await
        .expect("failed to open in-memory SQLite");

    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .expect("failed to run migrations on in-memory SQLite");

    let config = Arc::new(Config {
        bind_addr: "127.0.0.1:0".to_string(),
        base_url: "http://localhost".to_string(),
        session_secret: "test-session-secret".to_string(),
        required_admin_role: "matrix-admin".to_string(),
        homeserver_domain: "test.com".to_string(),
        oidc: OidcConfig {
            issuer_url: "http://localhost".to_string(),
            client_id: "test".to_string(),
            client_secret: "test".to_string(),
            redirect_url: "http://localhost/callback".to_string(),
        },
        keycloak: KeycloakConfig {
            base_url: "http://localhost".to_string(),
            realm: "test".to_string(),
            admin_client_id: "test".to_string(),
            admin_client_secret: "test".to_string(),
        },
        mas: MasConfig {
            base_url: "http://localhost".to_string(),
            admin_client_id: "test".to_string(),
            admin_client_secret: "test".to_string(),
        },
        database_url: "sqlite::memory:".to_string(),
        bot_api_secret: bot_secret.to_string(),
        invite_allowed_domains: allowed_domains,
    });

    let keycloak: Arc<dyn KeycloakApi> = Arc::new(keycloak);
    let mas: Arc<dyn MasApi> = Arc::new(MockMas::default());
    let users = Arc::new(UserService::new(
        Arc::clone(&keycloak),
        Arc::clone(&mas),
        "test.com",
    ));
    let audit = Arc::new(AuditService::new(pool.clone()));
    let oidc = Arc::new(OidcClient::new_stub());
    let cookie_key = Key::generate();

    AppState {
        config,
        db: pool,
        oidc,
        keycloak,
        mas,
        users,
        audit,
        cookie_key,
    }
}

// ── Router builder ────────────────────────────────────────────────────────────

/// Build a minimal router that only exposes the invite endpoint.
///
/// Auth-protected routes are intentionally excluded — the invite endpoint
/// uses bearer token auth, not the OIDC session cookie.
pub fn invite_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/v1/invites",
            post(crate::handlers::invite::create_invite),
        )
        .with_state(state)
}
