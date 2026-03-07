use serde::{Deserialize, Serialize};

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
    pub inferred_matrix_id: Option<String>,
    pub correlation_status: CorrelationStatus,
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
