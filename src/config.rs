use anyhow::Context;

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
    /// Both `SYNAPSE_BASE_URL` and `SYNAPSE_ADMIN_TOKEN` must be set; if either
    /// is missing the connector is disabled and the Reconcile button is hidden.
    pub synapse: Option<SynapseConfig>,
    pub database_url: String,
    /// Shared secret used by the maubot invite plugin to authenticate.
    pub bot_api_secret: String,
    /// If set, only emails from these domains may be invited (comma-separated).
    pub invite_allowed_domains: Option<Vec<String>>,
    /// Keycloak group → Matrix room membership policy (bootstrap/migration only).
    /// Loaded from `GROUP_MAPPINGS_FILE` (path to a JSON file) if set,
    /// otherwise from `GROUP_MAPPINGS` as an inline JSON array.
    /// Imported into SQLite on first run; DB is source of truth thereafter.
    pub group_mappings: Vec<crate::models::group_mapping::GroupMapping>,
    /// Optional path to the onboarding templates JSON file.
    /// Defaults to `onboarding_templates.json` in the working directory.
    pub onboarding_templates_file: Option<String>,
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
    /// Admin token for Synapse API access. In `matrix_authentication_service` mode,
    /// this is a `mas-cli`-provisioned compat token with `urn:synapse:admin:*` scope.
    pub admin_token: String,
}

fn require_env(key: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| panic!("Required environment variable {key} is not set"))
}

/// Load group mappings from `GROUP_MAPPINGS_FILE` (preferred) or `GROUP_MAPPINGS` env var.
///
/// Returns an error if the file cannot be read or either source contains invalid JSON.
/// Returns an empty vec if neither variable is set.
pub fn load_group_mappings() -> anyhow::Result<Vec<crate::models::group_mapping::GroupMapping>> {
    if let Ok(path) = std::env::var("GROUP_MAPPINGS_FILE") {
        let content = std::fs::read_to_string(&path)
            .with_context(|| format!("GROUP_MAPPINGS_FILE: could not read {path}"))?;
        let mappings = serde_json::from_str(&content)
            .with_context(|| format!("GROUP_MAPPINGS_FILE: invalid JSON in {path}"))?;
        return Ok(mappings);
    }

    match std::env::var("GROUP_MAPPINGS") {
        Ok(val) => {
            let mappings = serde_json::from_str(&val).context("GROUP_MAPPINGS: invalid JSON")?;
            Ok(mappings)
        }
        Err(_) => Ok(vec![]),
    }
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
                    std::env::var("SYNAPSE_ADMIN_TOKEN"),
                ) {
                    (Ok(base_url), Ok(admin_token)) => Some(SynapseConfig {
                        base_url,
                        admin_token,
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
            group_mappings: load_group_mappings().unwrap_or_else(|e| panic!("{e}")),
            onboarding_templates_file: std::env::var("ONBOARDING_TEMPLATES_FILE").ok(),
        }
    }

    /// Returns the path for the onboarding templates JSON file.
    pub fn templates_path(&self) -> std::path::PathBuf {
        match &self.onboarding_templates_file {
            Some(p) => std::path::PathBuf::from(p),
            None => std::path::PathBuf::from("onboarding_templates.json"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    // These tests mutate process-wide env vars. Rust runs tests in parallel by
    // default, so without serialisation they race each other. This mutex
    // ensures only one test touches GROUP_MAPPINGS / GROUP_MAPPINGS_FILE at a
    // time. Any test that calls set_var/remove_var on those keys must hold this
    // lock for its entire duration.
    static ENV_MUTEX: Mutex<()> = Mutex::new(());

    // ── GROUP_MAPPINGS_FILE ───────────────────────────────────────────────────

    #[test]
    fn load_group_mappings_file_valid_returns_mappings() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let path = format!("/tmp/mia_test_group_mappings_{}.json", std::process::id());
        std::fs::write(
            &path,
            r#"[{"keycloak_group":"staff","matrix_room_id":"!abc:example.com"}]"#,
        )
        .unwrap();

        std::env::remove_var("GROUP_MAPPINGS");
        std::env::set_var("GROUP_MAPPINGS_FILE", &path);

        let result = load_group_mappings();
        std::env::remove_var("GROUP_MAPPINGS_FILE");
        std::fs::remove_file(&path).ok();

        let mappings = result.expect("should parse successfully");
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].keycloak_group, "staff");
        assert_eq!(mappings[0].matrix_room_id, "!abc:example.com");
    }

    #[test]
    fn load_group_mappings_file_nonexistent_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("GROUP_MAPPINGS");
        std::env::set_var(
            "GROUP_MAPPINGS_FILE",
            "/tmp/mia_test_does_not_exist_99999.json",
        );

        let result = load_group_mappings();
        std::env::remove_var("GROUP_MAPPINGS_FILE");

        let err = result.expect_err("should fail on missing file");
        assert!(
            err.to_string().contains("could not read"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn load_group_mappings_file_invalid_json_returns_error() {
        let _guard = ENV_MUTEX.lock().unwrap();
        let path = format!(
            "/tmp/mia_test_group_mappings_bad_{}.json",
            std::process::id()
        );
        std::fs::write(&path, b"not valid json").unwrap();

        std::env::remove_var("GROUP_MAPPINGS");
        std::env::set_var("GROUP_MAPPINGS_FILE", &path);

        let result = load_group_mappings();
        std::env::remove_var("GROUP_MAPPINGS_FILE");
        std::fs::remove_file(&path).ok();

        let err = result.expect_err("should fail on invalid JSON");
        assert!(
            err.to_string().contains("invalid JSON"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn load_group_mappings_env_var_used_when_file_not_set() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("GROUP_MAPPINGS_FILE");
        std::env::set_var(
            "GROUP_MAPPINGS",
            r#"[{"keycloak_group":"admins","matrix_room_id":"!xyz:example.com"}]"#,
        );

        let result = load_group_mappings();
        std::env::remove_var("GROUP_MAPPINGS");

        let mappings = result.expect("should parse successfully");
        assert_eq!(mappings.len(), 1);
        assert_eq!(mappings[0].keycloak_group, "admins");
        assert_eq!(mappings[0].matrix_room_id, "!xyz:example.com");
    }

    #[test]
    fn load_group_mappings_neither_set_returns_empty() {
        let _guard = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("GROUP_MAPPINGS_FILE");
        std::env::remove_var("GROUP_MAPPINGS");

        let mappings = load_group_mappings().expect("should return empty vec");
        assert!(mappings.is_empty());
    }
}
