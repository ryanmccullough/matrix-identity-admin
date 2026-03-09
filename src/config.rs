/// Strongly-typed configuration loaded from environment variables.
/// The application will exit immediately if any required variable is missing.
#[derive(Debug, Clone)]
pub struct Config {
    pub bind_addr: String,
    pub base_url: String,
    pub session_secret: String,
    pub required_admin_role: String,
    pub homeserver_domain: String,
    pub oidc: OidcConfig,
    pub keycloak: KeycloakConfig,
    pub mas: MasConfig,
    /// Optional Synapse connector. Required for group membership reconciliation.
    /// All three `SYNAPSE_*` vars must be set together; if any is missing the
    /// connector is disabled and the Reconcile button is hidden in the UI.
    pub synapse: Option<SynapseConfig>,
    pub database_url: String,
    /// Shared secret used by the maubot invite plugin to authenticate.
    pub bot_api_secret: String,
    /// If set, only emails from these domains may be invited (comma-separated).
    pub invite_allowed_domains: Option<Vec<String>>,
    /// Keycloak group → Matrix room membership policy.
    /// Loaded from `GROUP_MAPPINGS` as a JSON array.
    pub group_mappings: Vec<crate::models::group_mapping::GroupMapping>,
    /// When true, kick users from mapped rooms if they are no longer in the
    /// corresponding Keycloak group. Defaults to false (join-only).
    pub reconcile_remove_from_rooms: bool,
}

#[derive(Debug, Clone)]
pub struct OidcConfig {
    pub issuer_url: String,
    pub client_id: String,
    pub client_secret: String,
    pub redirect_url: String,
}

#[derive(Debug, Clone)]
pub struct KeycloakConfig {
    pub base_url: String,
    pub realm: String,
    pub admin_client_id: String,
    pub admin_client_secret: String,
}

#[derive(Debug, Clone)]
pub struct MasConfig {
    pub base_url: String,
    pub admin_client_id: String,
    pub admin_client_secret: String,
}

#[derive(Debug, Clone)]
pub struct SynapseConfig {
    pub base_url: String,
    pub admin_user: String,
    pub admin_password: String,
}

fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("Required environment variable {key} is not set"))
}

impl Config {
    pub fn from_env() -> Self {
        Self {
            bind_addr: std::env::var("APP_BIND_ADDR")
                .unwrap_or_else(|_| "127.0.0.1:3000".to_string()),
            base_url: require_env("APP_BASE_URL"),
            session_secret: require_env("APP_SESSION_SECRET"),
            required_admin_role: std::env::var("APP_REQUIRED_ADMIN_ROLE")
                .unwrap_or_else(|_| "matrix-admin".to_string()),
            homeserver_domain: require_env("HOMESERVER_DOMAIN"),
            oidc: OidcConfig {
                issuer_url: require_env("OIDC_ISSUER_URL"),
                client_id: require_env("OIDC_CLIENT_ID"),
                client_secret: require_env("OIDC_CLIENT_SECRET"),
                redirect_url: require_env("OIDC_REDIRECT_URL"),
            },
            keycloak: KeycloakConfig {
                base_url: require_env("KEYCLOAK_BASE_URL"),
                realm: require_env("KEYCLOAK_REALM"),
                admin_client_id: require_env("KEYCLOAK_ADMIN_CLIENT_ID"),
                admin_client_secret: require_env("KEYCLOAK_ADMIN_CLIENT_SECRET"),
            },
            mas: MasConfig {
                base_url: require_env("MAS_BASE_URL"),
                admin_client_id: require_env("MAS_ADMIN_CLIENT_ID"),
                admin_client_secret: require_env("MAS_ADMIN_CLIENT_SECRET"),
            },
            synapse: {
                match (
                    std::env::var("SYNAPSE_BASE_URL"),
                    std::env::var("SYNAPSE_ADMIN_USER"),
                    std::env::var("SYNAPSE_ADMIN_PASSWORD"),
                ) {
                    (Ok(base_url), Ok(admin_user), Ok(admin_password)) => Some(SynapseConfig {
                        base_url,
                        admin_user,
                        admin_password,
                    }),
                    _ => None,
                }
            },
            database_url: require_env("DATABASE_URL"),
            bot_api_secret: require_env("BOT_API_SECRET"),
            invite_allowed_domains: std::env::var("INVITE_ALLOWED_DOMAINS").ok().map(|s| {
                s.split(',')
                    .map(|d| d.trim().to_lowercase())
                    .filter(|d| !d.is_empty())
                    .collect()
            }),
            group_mappings: std::env::var("GROUP_MAPPINGS")
                .ok()
                .map(|s| {
                    serde_json::from_str(&s)
                        .unwrap_or_else(|e| panic!("Invalid GROUP_MAPPINGS JSON: {e}"))
                })
                .unwrap_or_default(),
            reconcile_remove_from_rooms: std::env::var("RECONCILE_REMOVE_FROM_ROOMS")
                .ok()
                .map(|s| s.eq_ignore_ascii_case("true"))
                .unwrap_or(false),
        }
    }
}
