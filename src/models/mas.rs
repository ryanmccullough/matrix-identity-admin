use serde::{Deserialize, Serialize};

/// Represents a MAS user account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasUser {
    /// MAS internal ULID.
    pub id: String,
    /// Username (matches Keycloak username / OIDC preferred_username).
    pub username: String,
}

/// A single MAS session (compat or OAuth2).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasSession {
    pub id: String,
    /// "compat" or "oauth2" — determines which finish endpoint to call.
    pub session_type: String,
    pub created_at: Option<String>,
    pub last_active_at: Option<String>,
    pub user_agent: Option<String>,
    pub ip_address: Option<String>,
    /// If set the session is already finished.
    pub finished_at: Option<String>,
}
