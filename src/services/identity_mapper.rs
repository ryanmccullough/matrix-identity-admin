use crate::models::{
    keycloak::KeycloakUser,
    unified::CorrelationStatus,
};

/// The result of attempting to correlate a Keycloak user with their
/// MAS account and Matrix identity.
#[derive(Debug, Clone)]
pub struct MappedIdentity {
    pub keycloak_user: KeycloakUser,
    /// Derived Matrix user ID, e.g. `@alice:example.com`.
    /// Convention: `@{keycloak_username}:{homeserver_domain}`.
    pub inferred_matrix_id: Option<String>,
    pub correlation_status: CorrelationStatus,
}

pub struct IdentityMapper {
    homeserver_domain: String,
}

impl IdentityMapper {
    pub fn new(homeserver_domain: &str) -> Self {
        Self {
            homeserver_domain: homeserver_domain.to_string(),
        }
    }

    /// Derive the expected Matrix user ID from a Keycloak user.
    ///
    /// Convention: `@{keycloak_username}:{homeserver_domain}`.
    pub fn derive_matrix_id(&self, username: &str) -> String {
        format!("@{}:{}", username, self.homeserver_domain)
    }

    /// Build a best-effort `MappedIdentity` from a Keycloak user and optional
    /// MAS lookup result.
    ///
    /// - `Confirmed`: MAS account found (Keycloak + MAS both known).
    /// - `Inferred`: MAS account not found; Matrix ID derived by convention only.
    pub fn map(
        &self,
        keycloak_user: KeycloakUser,
        mas_user_id: Option<String>,
    ) -> MappedIdentity {
        let inferred_matrix_id = Some(self.derive_matrix_id(&keycloak_user.username));

        let correlation_status = if mas_user_id.is_some() {
            CorrelationStatus::Confirmed
        } else {
            CorrelationStatus::Inferred
        };

        MappedIdentity {
            keycloak_user,
            inferred_matrix_id,
            correlation_status,
        }
    }

    /// Produce a summary mapping without any upstream lookups.
    /// Used for search results where we don't want to fan out N+1 queries.
    pub fn map_summary_only(&self, keycloak_user: KeycloakUser) -> MappedIdentity {
        let inferred_matrix_id = Some(self.derive_matrix_id(&keycloak_user.username));
        MappedIdentity {
            keycloak_user,
            inferred_matrix_id,
            correlation_status: CorrelationStatus::Inferred,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::keycloak::KeycloakUser;

    fn test_user(username: &str) -> KeycloakUser {
        KeycloakUser {
            id: "kc-123".to_string(),
            username: username.to_string(),
            email: None,
            first_name: None,
            last_name: None,
            enabled: true,
            email_verified: false,
            created_timestamp: None,
        }
    }

    #[test]
    fn derives_matrix_id_by_convention() {
        let mapper = IdentityMapper::new("example.com");
        assert_eq!(
            mapper.derive_matrix_id("alice"),
            "@alice:example.com"
        );
    }

    #[test]
    fn confirmed_when_mas_found() {
        let mapper = IdentityMapper::new("example.com");
        let identity = mapper.map(test_user("alice"), Some("mas-456".to_string()));
        assert_eq!(identity.correlation_status, CorrelationStatus::Confirmed);
    }

    #[test]
    fn inferred_when_mas_not_found() {
        let mapper = IdentityMapper::new("example.com");
        let identity = mapper.map(test_user("alice"), None);
        assert_eq!(identity.correlation_status, CorrelationStatus::Inferred);
        assert!(identity.inferred_matrix_id.is_some());
    }
}
