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
    pub database_url: String,
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
            database_url: require_env("DATABASE_URL"),
        }
    }
}
