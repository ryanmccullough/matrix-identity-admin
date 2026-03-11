use serde::{Deserialize, Serialize};

/// Represents a MAS user account.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MasUser {
    /// MAS internal ULID.
    pub id: String,
    /// Username (matches Keycloak username / OIDC preferred_username).
    pub username: String,
    /// Set if the account has been deactivated; None means the account is active.
    pub deactivated_at: Option<String>,
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

/// Result of listing MAS sessions, carrying any warnings about partial data.
///
/// When both compat and OAuth2 session endpoints succeed, `warnings` is empty.
/// When one fails, the successfully-fetched sessions are still returned, and
/// a warning describes which endpoint failed.
///
/// # Security note
///
/// Lifecycle mutations (disable/offboard) warn-and-continue when session
/// listing is partial rather than failing closed. This is acceptable because
/// disabling the Keycloak account is the hard security boundary — Synapse
/// validates tokens via MAS introspection, and MAS checks the upstream IdP.
/// A disabled Keycloak account causes introspection to fail, effectively
/// killing all sessions regardless of explicit revocation. Explicit session
/// revocation is belt-and-suspenders, not the security boundary.
///
/// If this assumption changes (e.g. long-lived cached tokens bypass
/// introspection), revisit this decision and consider failing closed.
pub struct SessionListResult {
    pub sessions: Vec<MasSession>,
    pub warnings: Vec<String>,
}
