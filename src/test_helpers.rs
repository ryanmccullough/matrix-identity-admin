use std::sync::Arc;

use async_trait::async_trait;
use axum::{routing::post, Router};
use axum_extra::extract::cookie::Key;
use sqlx::sqlite::SqlitePoolOptions;

use crate::{
    auth::oidc::OidcClient,
    auth::session::AdminSession,
    clients::{AuthService, IdentityProvider, KeycloakIdentityProvider, MatrixService},
    config::{Config, KeycloakConfig, MasConfig, OidcConfig},
    error::AppError,
    models::{
        keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
        mas::{MasSession, MasUser, SessionListResult},
        synapse::{RoomDetails, RoomList, SynapseDevice, SynapseUser},
        unified::CanonicalUser,
    },
    services::{AuditService, PolicyService, UserService},
    state::AppState,
};

// ── Test constants ────────────────────────────────────────────────────────────

/// Fixed 64-byte key used for cookie encryption in tests.
/// All test state builders use this key so that `make_auth_cookie` can
/// produce cookies that the handlers will accept.
pub const TEST_KEY: &[u8; 64] = &[42u8; 64];

/// CSRF token used in test sessions and form bodies.
pub const TEST_CSRF: &str = "test-csrf-token";

// ── Mock Keycloak ─────────────────────────────────────────────────────────────

/// Configurable mock for the Keycloak API.
///
/// Defaults to returning empty/successful responses. Set fields to control
/// behaviour in individual tests.
pub struct MockKeycloak {
    /// Users returned by `search_users` and `get_user` (first element).
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
    /// If true, `logout_user` returns an upstream error.
    pub fail_logout: bool,
    /// If true, `delete_user` returns an upstream error.
    pub fail_delete: bool,
    /// If true, `disable_user` returns an upstream error.
    pub fail_disable: bool,
    /// If true, `enable_user` returns an upstream error.
    pub fail_enable: bool,
    /// Value returned by `count_users`.
    pub user_count: u32,
    /// Groups returned by `list_groups`.
    pub all_groups: Vec<KeycloakGroup>,
    /// Roles returned by `list_realm_roles`.
    pub all_roles: Vec<KeycloakRole>,
    /// If true, `list_groups` returns an upstream error.
    pub fail_list_groups: bool,
    /// If true, `list_realm_roles` returns an upstream error.
    pub fail_list_roles: bool,
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
            fail_logout: false,
            fail_delete: false,
            fail_disable: false,
            fail_enable: false,
            user_count: 0,
            all_groups: vec![],
            all_roles: vec![],
            fail_list_groups: false,
            fail_list_roles: false,
        }
    }
}

fn paginated_users(users: &[KeycloakUser], max: u32, first: u32) -> Vec<KeycloakUser> {
    let start = first as usize;
    if start >= users.len() {
        return vec![];
    }

    let end = start.saturating_add(max as usize).min(users.len());
    users[start..end].to_vec()
}

#[async_trait]
impl KeycloakIdentityProvider for MockKeycloak {
    async fn search_users(
        &self,
        _query: &str,
        max: u32,
        first: u32,
    ) -> Result<Vec<KeycloakUser>, AppError> {
        Ok(paginated_users(&self.users, max, first))
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
        if self.fail_logout {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock logout failure".into(),
            })
        } else {
            Ok(())
        }
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

    async fn delete_user(&self, _user_id: &str) -> Result<(), AppError> {
        if self.fail_delete {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock delete_user failure".into(),
            })
        } else {
            Ok(())
        }
    }

    async fn disable_user(&self, _user_id: &str) -> Result<(), AppError> {
        if self.fail_disable {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock disable_user failure".into(),
            })
        } else {
            Ok(())
        }
    }

    async fn enable_user(&self, _user_id: &str) -> Result<(), AppError> {
        if self.fail_enable {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock enable_user failure".into(),
            })
        } else {
            Ok(())
        }
    }

    async fn count_users(&self, _query: &str) -> Result<u32, AppError> {
        Ok(self.user_count)
    }

    async fn list_groups(&self) -> Result<Vec<KeycloakGroup>, AppError> {
        if self.fail_list_groups {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock list_groups failure".into(),
            })
        } else {
            Ok(self.all_groups.clone())
        }
    }

    async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>, AppError> {
        if self.fail_list_roles {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock list_realm_roles failure".into(),
            })
        } else {
            Ok(self.all_roles.clone())
        }
    }
}

/// `IdentityProvider` implementation for `MockKeycloak`.
///
/// Maps `KeycloakUser` fields to `CanonicalUser` using the same logic as
/// `KeycloakClient`'s `IdentityProvider` impl. Groups and roles returned
/// via separate calls — `get_user` returns an empty-groups/roles canonical
/// user, and `get_user_groups`/`get_user_roles` return the configured slices.
#[async_trait]
impl IdentityProvider for MockKeycloak {
    async fn search_users(
        &self,
        _query: &str,
        max: u32,
        first: u32,
    ) -> Result<Vec<CanonicalUser>, AppError> {
        Ok(paginated_users(&self.users, max, first)
            .into_iter()
            .map(|u| CanonicalUser {
                id: u.id,
                username: u.username,
                email: u.email,
                first_name: u.first_name,
                last_name: u.last_name,
                enabled: u.enabled,
                groups: vec![],
                roles: vec![],
                required_actions: u.required_actions,
            })
            .collect())
    }

    async fn get_user(&self, _id: &str) -> Result<CanonicalUser, AppError> {
        let u = self
            .users
            .first()
            .ok_or_else(|| AppError::NotFound("user not found".into()))?;
        Ok(CanonicalUser {
            id: u.id.clone(),
            username: u.username.clone(),
            email: u.email.clone(),
            first_name: u.first_name.clone(),
            last_name: u.last_name.clone(),
            enabled: u.enabled,
            groups: vec![],
            roles: vec![],
            required_actions: u.required_actions.clone(),
        })
    }

    async fn get_user_groups(&self, _id: &str) -> Result<Vec<String>, AppError> {
        Ok(self.groups.iter().map(|g| g.name.clone()).collect())
    }

    async fn get_user_roles(&self, _id: &str) -> Result<Vec<String>, AppError> {
        Ok(self.roles.iter().map(|r| r.name.clone()).collect())
    }

    async fn logout_user(&self, _id: &str) -> Result<(), AppError> {
        if self.fail_logout {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "mock logout failure".into(),
            })
        } else {
            Ok(())
        }
    }

    async fn count_users(&self, _query: &str) -> Result<u32, AppError> {
        Ok(self.user_count)
    }
}

// ── Mock MAS ──────────────────────────────────────────────────────────────────

#[derive(Default)]
pub struct MockMas {
    pub user: Option<MasUser>,
    pub sessions: Vec<MasSession>,
    /// If true, `finish_session` returns an upstream error.
    pub fail_finish_session: bool,
    /// If true, `delete_user` returns an upstream error.
    pub fail_delete_user: bool,
    /// If true, `reactivate_user` returns an upstream error.
    pub fail_reactivate: bool,
    /// If true, `get_user_by_username` returns an upstream error.
    pub fail_get_user_by_username: bool,
}

#[async_trait]
impl AuthService for MockMas {
    async fn get_user_by_username(&self, _username: &str) -> Result<Option<MasUser>, AppError> {
        if self.fail_get_user_by_username {
            Err(AppError::Upstream {
                service: "mas".into(),
                message: "mock lookup failure".into(),
            })
        } else {
            Ok(self.user.clone())
        }
    }

    async fn list_sessions(&self, _mas_user_id: &str) -> Result<SessionListResult, AppError> {
        Ok(SessionListResult {
            sessions: self.sessions.clone(),
            warnings: vec![],
        })
    }

    async fn finish_session(&self, _session_id: &str, _session_type: &str) -> Result<(), AppError> {
        if self.fail_finish_session {
            Err(AppError::Upstream {
                service: "mas".into(),
                message: "mock finish_session failure".into(),
            })
        } else {
            Ok(())
        }
    }

    async fn delete_user(&self, _mas_user_id: &str) -> Result<(), AppError> {
        if self.fail_delete_user {
            Err(AppError::Upstream {
                service: "mas".into(),
                message: "mock delete_user failure".into(),
            })
        } else {
            Ok(())
        }
    }

    async fn reactivate_user(&self, _mas_user_id: &str) -> Result<(), AppError> {
        if self.fail_reactivate {
            Err(AppError::Upstream {
                service: "mas".into(),
                message: "mock reactivate_user failure".into(),
            })
        } else {
            Ok(())
        }
    }
}

// ── Mock Synapse ──────────────────────────────────────────────────────────────

/// Configurable mock for the Synapse API.
#[derive(Default)]
pub struct MockSynapse {
    /// Members already in the room (returned by `get_joined_room_members`).
    pub members: Vec<String>,
    pub fail_get_members: bool,
    pub fail_force_join: bool,
    pub fail_kick: bool,
    /// Child room IDs returned by `get_space_children`. Keyed by space ID.
    pub space_children: std::collections::HashMap<String, Vec<String>>,
    pub fail_get_space_children: bool,
    pub room_list: Vec<crate::models::synapse::RoomListEntry>,
    pub room_details: Option<crate::models::synapse::RoomDetails>,
    pub fail_list_rooms: bool,
    pub fail_set_power_level: bool,
}

#[async_trait]
impl MatrixService for MockSynapse {
    async fn get_user(&self, _: &str) -> Result<Option<SynapseUser>, AppError> {
        unimplemented!()
    }
    async fn list_devices(&self, _: &str) -> Result<Vec<SynapseDevice>, AppError> {
        unimplemented!()
    }
    async fn delete_device(&self, _: &str, _: &str) -> Result<(), AppError> {
        unimplemented!()
    }

    async fn get_joined_room_members(&self, _room_id: &str) -> Result<Vec<String>, AppError> {
        if self.fail_get_members {
            return Err(AppError::Upstream {
                service: "synapse".into(),
                message: "mock member fetch failure".into(),
            });
        }
        Ok(self.members.clone())
    }

    async fn force_join_user(&self, _user_id: &str, _room_id: &str) -> Result<(), AppError> {
        if self.fail_force_join {
            return Err(AppError::Upstream {
                service: "synapse".into(),
                message: "mock force_join failure".into(),
            });
        }
        Ok(())
    }

    async fn kick_user_from_room(
        &self,
        _user_id: &str,
        _room_id: &str,
        _reason: &str,
    ) -> Result<(), AppError> {
        if self.fail_kick {
            return Err(AppError::Upstream {
                service: "synapse".into(),
                message: "mock kick failure".into(),
            });
        }
        Ok(())
    }

    async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError> {
        if self.fail_get_space_children {
            return Err(AppError::Upstream {
                service: "synapse".into(),
                message: "mock get_space_children failure".into(),
            });
        }
        Ok(self
            .space_children
            .get(space_id)
            .cloned()
            .unwrap_or_default())
    }

    async fn list_rooms(&self, _limit: u32, _from: Option<&str>) -> Result<RoomList, AppError> {
        if self.fail_list_rooms {
            return Err(AppError::Upstream {
                service: "synapse".into(),
                message: "mock list_rooms failure".into(),
            });
        }
        Ok(RoomList {
            rooms: self.room_list.clone(),
            next_batch: None,
            total_rooms: Some(self.room_list.len() as i64),
        })
    }

    async fn get_room_details(&self, _room_id: &str) -> Result<RoomDetails, AppError> {
        self.room_details
            .clone()
            .ok_or_else(|| AppError::NotFound("room not found".into()))
    }

    async fn set_power_level(
        &self,
        _room_id: &str,
        _user_id: &str,
        _level: i64,
    ) -> Result<(), AppError> {
        if self.fail_set_power_level {
            return Err(AppError::Upstream {
                service: "synapse".into(),
                message: "mock set_power_level failure".into(),
            });
        }
        Ok(())
    }
}

// ── State builders ────────────────────────────────────────────────────────────

/// Build an `AppState` backed by an in-memory SQLite database.
///
/// Accepts both a `MockKeycloak` and a `MockMas` for full control.
/// Uses `TEST_KEY` for cookie encryption so that `make_auth_cookie` produces
/// cookies this state will accept.
pub async fn build_test_state_full(
    keycloak: MockKeycloak,
    mas: MockMas,
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
        synapse: None,
        group_mappings: vec![],
    });

    // MockKeycloak implements both KeycloakIdentityProvider and IdentityProvider.
    // Construct a shared Arc<MockKeycloak> and coerce to each trait object
    // separately so both AppState.keycloak and UserService see the same mock.
    let mock_kc = Arc::new(keycloak);
    let keycloak: Arc<dyn KeycloakIdentityProvider> =
        Arc::clone(&mock_kc) as Arc<dyn KeycloakIdentityProvider>;
    let identity_provider: Arc<dyn IdentityProvider> =
        Arc::clone(&mock_kc) as Arc<dyn IdentityProvider>;
    let mas: Arc<dyn AuthService> = Arc::new(mas);
    let users = Arc::new(UserService::new(
        identity_provider,
        Arc::clone(&mas),
        "test.com",
    ));
    let audit = Arc::new(AuditService::new(pool.clone()));
    let policy_service = Arc::new(PolicyService::new(pool.clone()));
    let oidc = Arc::new(OidcClient::new_stub());
    let cookie_key = Key::from(TEST_KEY);

    AppState {
        config,
        db: pool,
        oidc,
        keycloak,
        mas,
        synapse: None,
        users,
        audit,
        policy_service,
        cookie_key,
    }
}

/// Build an `AppState` with a default `MockMas`. Convenience wrapper around
/// `build_test_state_full` for tests that only care about Keycloak behaviour
/// (e.g. invite handler tests).
pub async fn build_test_state(
    keycloak: MockKeycloak,
    bot_secret: &str,
    allowed_domains: Option<Vec<String>>,
) -> AppState {
    build_test_state_full(keycloak, MockMas::default(), bot_secret, allowed_domains).await
}

/// Build an `AppState` with a wired-in `MockSynapse` and optional group mappings.
/// Used by reconcile handler tests.
///
/// Policy bindings are bootstrapped into the DB so that
/// `state.policy_service.list_bindings()` returns them.
pub async fn build_test_state_with_synapse(
    keycloak: MockKeycloak,
    synapse: MockSynapse,
    group_mappings: Vec<crate::models::group_mapping::GroupMapping>,
    reconcile_remove_from_rooms: bool,
) -> AppState {
    use crate::models::policy_binding::{PolicySubject, PolicyTarget};

    let mut state = build_test_state_full(keycloak, MockMas::default(), "secret", None).await;
    let mut config = (*state.config).clone();
    config.group_mappings = group_mappings.clone();
    state.config = Arc::new(config);
    state.synapse = Some(Arc::new(synapse) as Arc<dyn MatrixService>);

    // Populate policy bindings in DB so handlers can read them.
    let audit = &state.audit;
    for mapping in &group_mappings {
        let _ = state
            .policy_service
            .create_binding(
                &PolicySubject::Group(mapping.keycloak_group.clone()),
                &PolicyTarget::Room(mapping.matrix_room_id.clone()),
                None,
                reconcile_remove_from_rooms,
                audit,
                "test",
                "test",
            )
            .await;
    }

    state
}

// ── Auth helpers ──────────────────────────────────────────────────────────────

/// Build an encrypted session cookie header value for use in test requests.
///
/// Uses `TEST_KEY` — the same key baked into `build_test_state_full` — so
/// the returned cookie will be accepted by any state built with that function.
pub fn make_auth_cookie(csrf: &str) -> String {
    use cookie::{Cookie as RawCookie, CookieJar};

    let session = AdminSession {
        subject: "test-subject".to_string(),
        username: "test-admin".to_string(),
        email: Some("admin@test.com".to_string()),
        roles: vec!["matrix-admin".to_string()],
        csrf_token: csrf.to_string(),
    };
    let json = serde_json::to_string(&session).unwrap();

    let key = Key::from(TEST_KEY);
    let mut jar = CookieJar::new();
    jar.private_mut(&key).add(RawCookie::new("session", json));

    jar.iter()
        .map(|c| c.to_string())
        .collect::<Vec<_>>()
        .join("; ")
}

// ── Router builders ───────────────────────────────────────────────────────────

/// Minimal router exposing only the invite endpoint (bearer-token auth).
pub fn invite_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/api/v1/invites",
            post(crate::handlers::invite::create_invite),
        )
        .with_state(state)
}

/// Router exposing the read-only user endpoints (search and detail).
pub fn reads_router(state: AppState) -> Router {
    use axum::routing::get;
    Router::new()
        .route("/users/search", get(crate::handlers::users::search))
        .route("/users/{id}", get(crate::handlers::users::detail))
        .with_state(state)
}

/// Router exposing the dashboard endpoint.
pub fn dashboard_router(state: AppState) -> Router {
    use axum::routing::get;
    Router::new()
        .route("/", get(crate::handlers::dashboard::dashboard))
        .with_state(state)
}

/// Router exposing the audit log listing endpoint.
pub fn audit_router(state: AppState) -> Router {
    use axum::routing::get;
    Router::new()
        .route("/audit", get(crate::handlers::audit::list))
        .with_state(state)
}

/// Router exposing audit log listing and CSV export endpoints.
pub fn audit_export_router(state: AppState) -> Router {
    use axum::routing::get;
    Router::new()
        .route("/audit", get(crate::handlers::audit::list))
        .route("/audit/export", get(crate::handlers::audit::export_csv))
        .with_state(state)
}

/// Router exposing the policy management endpoints.
pub fn policy_router(state: AppState) -> Router {
    use axum::routing::get;
    Router::new()
        .route("/policy", get(crate::handlers::policy::list))
        .route("/policy/bindings", post(crate::handlers::policy::create))
        .route(
            "/policy/bindings/{id}/update",
            post(crate::handlers::policy::update),
        )
        .route(
            "/policy/bindings/{id}/delete",
            post(crate::handlers::policy::delete),
        )
        .route(
            "/policy/rooms/refresh",
            post(crate::handlers::policy::refresh_rooms),
        )
        .route(
            "/policy/api/groups",
            get(crate::handlers::policy::api_groups),
        )
        .route("/policy/api/roles", get(crate::handlers::policy::api_roles))
        .route("/policy/api/rooms", get(crate::handlers::policy::api_rooms))
        .with_state(state)
}

/// Router exposing the admin UI invite endpoint.
pub fn admin_invite_router(state: AppState) -> Router {
    Router::new()
        .route("/users/invite", post(crate::handlers::invite::admin_invite))
        .with_state(state)
}

/// Router exposing all session-authenticated mutation endpoints.
///
/// Used to test the revoke, force-logout, and delete handlers without
/// standing up the full application.
pub fn mutations_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/users/{id}/sessions/{session_id}/revoke",
            post(crate::handlers::sessions::revoke),
        )
        .route(
            "/users/{id}/keycloak/logout",
            post(crate::handlers::devices::force_keycloak_logout),
        )
        .route(
            "/users/{id}/delete",
            post(crate::handlers::delete::delete_user_handler),
        )
        .route(
            "/users/{id}/disable",
            post(crate::handlers::disable::disable),
        )
        .route(
            "/users/{id}/reactivate",
            post(crate::handlers::reactivate::reactivate),
        )
        .route(
            "/users/{id}/offboard",
            post(crate::handlers::offboard::offboard),
        )
        .route(
            "/users/{id}/reconcile",
            post(crate::handlers::reconcile::reconcile),
        )
        .route(
            "/users/{id}/reconcile/preview",
            post(crate::handlers::reconcile::reconcile_preview),
        )
        .route(
            "/users/reconcile/all",
            post(crate::handlers::bulk_reconcile::bulk_reconcile),
        )
        .with_state(state)
}
