use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuditResult {
    Success,
    Failure,
}

impl std::fmt::Display for AuditResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Success => write!(f, "success"),
            Self::Failure => write!(f, "failure"),
        }
    }
}

impl AuditResult {
    pub fn from_str(s: &str) -> Self {
        match s {
            "success" => Self::Success,
            _ => Self::Failure,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLog {
    pub id: String,
    pub timestamp: String,
    pub admin_subject: String,
    pub admin_username: String,
    pub target_keycloak_user_id: Option<String>,
    pub target_matrix_user_id: Option<String>,
    pub action: String,
    pub result: String,
    pub metadata_json: String,
}
