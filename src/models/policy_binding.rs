use serde::{Deserialize, Serialize};

/// The subject of a policy binding — either a Keycloak group or role name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum PolicySubject {
    Group(String),
    Role(String),
}

impl PolicySubject {
    pub fn subject_type(&self) -> &str {
        match self {
            PolicySubject::Group(_) => "group",
            PolicySubject::Role(_) => "role",
        }
    }

    pub fn value(&self) -> &str {
        match self {
            PolicySubject::Group(v) | PolicySubject::Role(v) => v,
        }
    }
}

impl std::fmt::Display for PolicySubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicySubject::Group(v) => write!(f, "group:{v}"),
            PolicySubject::Role(v) => write!(f, "role:{v}"),
        }
    }
}

/// The target of a policy binding — a Matrix room or space ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "room_id")]
pub enum PolicyTarget {
    Room(String),
    Space(String),
}

impl PolicyTarget {
    pub fn target_type(&self) -> &str {
        match self {
            PolicyTarget::Room(_) => "room",
            PolicyTarget::Space(_) => "space",
        }
    }

    pub fn room_id(&self) -> &str {
        match self {
            PolicyTarget::Room(id) | PolicyTarget::Space(id) => id,
        }
    }
}

/// A policy binding maps a Keycloak group or role to a Matrix room or space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyBinding {
    pub id: String,
    pub subject: PolicySubject,
    pub target: PolicyTarget,
    /// Optional power level to set after joining (e.g. 100 for admin).
    pub power_level: Option<i64>,
    /// Whether to kick users from this room when they lose the group/role.
    pub allow_remove: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Cached metadata about a Matrix room, used for display in the policy UI.
/// Reconciliation uses `room_id` only — never relies on cached names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedRoom {
    pub room_id: String,
    pub name: Option<String>,
    pub canonical_alias: Option<String>,
    pub parent_space_id: Option<String>,
    pub is_space: bool,
    pub last_seen_at: String,
}
