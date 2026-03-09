use serde::{Deserialize, Serialize};

/// The lifecycle state of a user account, derived from Keycloak and MAS state.
///
/// Precedence:
///   1. `Disabled` — Keycloak account disabled OR MAS account deactivated.
///   2. `Invited` — Keycloak enabled + pending required actions (user has not
///      completed onboarding).
///   3. `Active` — Keycloak enabled + no pending required actions.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum LifecycleState {
    Invited,
    Active,
    Disabled,
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invited => write!(f, "Invited"),
            Self::Active => write!(f, "Active"),
            Self::Disabled => write!(f, "Disabled"),
        }
    }
}

impl LifecycleState {
    /// Returns a CSS class name for badge styling in templates.
    pub fn css_class(&self) -> &'static str {
        match self {
            Self::Invited => "badge-info",
            Self::Active => "badge-ok",
            Self::Disabled => "badge-warning",
        }
    }
}

/// Confidence level for the Keycloak → MAS → Matrix identity correlation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum CorrelationStatus {
    /// Keycloak user and MAS account both found.
    Confirmed,
    /// Keycloak user found; MAS account not found. Matrix ID derived by convention.
    Inferred,
}

impl std::fmt::Display for CorrelationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Confirmed => write!(f, "Confirmed"),
            Self::Inferred => write!(f, "Inferred"),
        }
    }
}

/// Lightweight summary used in search results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedUserSummary {
    pub keycloak_id: String,
    pub username: String,
    pub email: Option<String>,
    pub enabled: bool,
    pub lifecycle_state: LifecycleState,
    pub inferred_matrix_id: Option<String>,
    pub correlation_status: CorrelationStatus,
}

#[cfg(test)]
mod tests {
    use super::{CorrelationStatus, LifecycleState};

    // ── CorrelationStatus ─────────────────────────────────────────────────────

    #[test]
    fn confirmed_display() {
        assert_eq!(CorrelationStatus::Confirmed.to_string(), "Confirmed");
    }

    #[test]
    fn inferred_display() {
        assert_eq!(CorrelationStatus::Inferred.to_string(), "Inferred");
    }

    // ── LifecycleState ────────────────────────────────────────────────────────

    #[test]
    fn lifecycle_invited_display() {
        assert_eq!(LifecycleState::Invited.to_string(), "Invited");
    }

    #[test]
    fn lifecycle_active_display() {
        assert_eq!(LifecycleState::Active.to_string(), "Active");
    }

    #[test]
    fn lifecycle_disabled_display() {
        assert_eq!(LifecycleState::Disabled.to_string(), "Disabled");
    }
}

/// Full detail view for a single user, combining all three upstream systems.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedUserDetail {
    pub keycloak_id: String,
    pub username: String,
    pub email: Option<String>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub enabled: bool,
    pub lifecycle_state: LifecycleState,
    pub groups: Vec<String>,
    pub roles: Vec<String>,
    pub matrix_id: Option<String>,
    pub correlation_status: CorrelationStatus,
    pub sessions: Vec<UnifiedSession>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UnifiedSession {
    pub id: String,
    /// "compat" or "oauth2" — used to route to the correct finish endpoint.
    pub session_type: String,
    pub created_at: Option<String>,
    pub last_active_at: Option<String>,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    /// "active" or "finished", derived from whether finished_at is set.
    pub state: String,
}
