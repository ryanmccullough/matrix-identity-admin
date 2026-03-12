use crate::models::unified::{is_valid_matrix_localpart, CanonicalUser, CorrelationStatus};

/// The result of attempting to correlate an identity provider user with their
/// MAS account and Matrix identity.
#[derive(Debug, Clone)]
pub struct MappedIdentity {
    /// Canonical domain representation — no connector-specific types.
    /// Previously held `KeycloakUser`; replaced in issue #40.
    pub canonical: CanonicalUser,
    /// Derived Matrix user ID, e.g. `@alice:example.com`.
    /// Convention: `@{username}:{homeserver_domain}`.
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

    // TODO: correlation uses mutable usernames — if an admin renames a Keycloak
    // user, the derived Matrix ID and MAS lookup will silently break. See #99
    // for the plan to use stable external IDs instead.
    /// Derive the expected Matrix user ID from a username, returning `None` if
    /// the username is not a valid Matrix localpart.
    pub fn derive_matrix_id(&self, username: &str) -> Option<String> {
        if is_valid_matrix_localpart(username) {
            Some(format!("@{}:{}", username, self.homeserver_domain))
        } else {
            tracing::warn!(
                username,
                "Username is not a valid Matrix localpart — cannot derive Matrix ID"
            );
            None
        }
    }

    /// Build a best-effort `MappedIdentity` from a `CanonicalUser` and optional
    /// MAS lookup result.
    ///
    /// - `Confirmed`: MAS account found (identity provider + MAS both known).
    /// - `Inferred`: MAS account not found; Matrix ID derived by convention only.
    pub fn map(&self, canonical: CanonicalUser, mas_user_id: Option<String>) -> MappedIdentity {
        let inferred_matrix_id = self.derive_matrix_id(&canonical.username);

        let correlation_status = if mas_user_id.is_some() {
            CorrelationStatus::Confirmed
        } else {
            CorrelationStatus::Inferred
        };

        MappedIdentity {
            canonical,
            inferred_matrix_id,
            correlation_status,
        }
    }

    /// Produce a summary mapping without any upstream lookups.
    /// Used for search results where we don't want to fan out N+1 queries.
    pub fn map_summary_only(&self, canonical: CanonicalUser) -> MappedIdentity {
        let inferred_matrix_id = self.derive_matrix_id(&canonical.username);
        MappedIdentity {
            canonical,
            inferred_matrix_id,
            correlation_status: CorrelationStatus::Inferred,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::unified::CanonicalUser;

    fn test_canonical(username: &str) -> CanonicalUser {
        CanonicalUser {
            id: "kc-123".to_string(),
            username: username.to_string(),
            email: None,
            first_name: None,
            last_name: None,
            enabled: true,
            groups: vec![],
            roles: vec![],
            required_actions: vec![],
        }
    }

    #[test]
    fn derives_matrix_id_by_convention() {
        let mapper = IdentityMapper::new("example.com");
        assert_eq!(
            mapper.derive_matrix_id("alice"),
            Some("@alice:example.com".to_string())
        );
    }

    #[test]
    fn derive_matrix_id_returns_none_for_invalid_localpart() {
        let mapper = IdentityMapper::new("example.com");
        assert_eq!(mapper.derive_matrix_id("Alice"), None);
        assert_eq!(mapper.derive_matrix_id("user+tag"), None);
        assert_eq!(mapper.derive_matrix_id("user name"), None);
        assert_eq!(mapper.derive_matrix_id(""), None);
    }

    #[test]
    fn confirmed_when_mas_found() {
        let mapper = IdentityMapper::new("example.com");
        let identity = mapper.map(test_canonical("alice"), Some("mas-456".to_string()));
        assert_eq!(identity.correlation_status, CorrelationStatus::Confirmed);
    }

    #[test]
    fn inferred_when_mas_not_found() {
        let mapper = IdentityMapper::new("example.com");
        let identity = mapper.map(test_canonical("alice"), None);
        assert_eq!(identity.correlation_status, CorrelationStatus::Inferred);
        assert!(identity.inferred_matrix_id.is_some());
    }

    #[test]
    fn map_invalid_username_returns_none_matrix_id() {
        let mapper = IdentityMapper::new("example.com");
        let identity = mapper.map(test_canonical("Alice+Bad"), None);
        assert!(identity.inferred_matrix_id.is_none());
    }

    #[test]
    fn map_summary_only_uses_inferred_status() {
        let mapper = IdentityMapper::new("example.com");
        let identity = mapper.map_summary_only(test_canonical("bob"));
        assert_eq!(identity.correlation_status, CorrelationStatus::Inferred);
        assert_eq!(
            identity.inferred_matrix_id.as_deref(),
            Some("@bob:example.com")
        );
    }

    #[test]
    fn map_summary_only_invalid_username_returns_none() {
        let mapper = IdentityMapper::new("example.com");
        let identity = mapper.map_summary_only(test_canonical("Bad User"));
        assert!(identity.inferred_matrix_id.is_none());
    }

    #[test]
    fn canonical_user_fields_preserved_through_map() {
        let mapper = IdentityMapper::new("example.com");
        let canonical = CanonicalUser {
            id: "kc-123".to_string(),
            username: "alice".to_string(),
            email: Some("alice@example.com".to_string()),
            first_name: Some("Alice".to_string()),
            last_name: Some("Smith".to_string()),
            enabled: false,
            groups: vec!["staff".to_string()],
            roles: vec!["admin".to_string()],
            required_actions: vec!["VERIFY_EMAIL".to_string()],
        };

        let identity = mapper.map(canonical, None);

        assert_eq!(identity.canonical.id, "kc-123");
        assert_eq!(identity.canonical.username, "alice");
        assert_eq!(
            identity.canonical.email.as_deref(),
            Some("alice@example.com")
        );
        assert_eq!(identity.canonical.first_name.as_deref(), Some("Alice"));
        assert_eq!(identity.canonical.last_name.as_deref(), Some("Smith"));
        assert!(!identity.canonical.enabled);
        assert_eq!(identity.canonical.groups, vec!["staff"]);
        assert_eq!(identity.canonical.roles, vec!["admin"]);
        assert_eq!(
            identity.canonical.required_actions,
            vec!["VERIFY_EMAIL".to_string()]
        );
    }
}
