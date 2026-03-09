use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeycloakUser {
    pub id: String,
    pub username: String,
    pub email: Option<String>,
    #[serde(rename = "firstName")]
    pub first_name: Option<String>,
    #[serde(rename = "lastName")]
    pub last_name: Option<String>,
    pub enabled: bool,
    #[serde(rename = "emailVerified")]
    pub email_verified: bool,
    #[serde(rename = "createdTimestamp")]
    pub created_timestamp: Option<i64>,
    /// Keycloak required actions pending for this user (e.g. UPDATE_PASSWORD,
    /// VERIFY_EMAIL). Non-empty means the user has not completed onboarding.
    #[serde(rename = "requiredActions", default)]
    pub required_actions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeycloakGroup {
    pub id: String,
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeycloakRole {
    pub id: String,
    pub name: String,
    pub composite: bool,
    #[serde(rename = "clientRole")]
    pub client_role: bool,
    #[serde(rename = "containerId")]
    pub container_id: Option<String>,
}
