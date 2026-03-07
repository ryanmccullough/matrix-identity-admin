use serde::{Deserialize, Serialize};

/// Response from GET /_synapse/admin/v2/users/@user:domain
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynapseUser {
    pub name: String,
    pub displayname: Option<String>,
    pub admin: Option<bool>,
    pub deactivated: Option<bool>,
    pub creation_ts: Option<i64>,
    pub avatar_url: Option<String>,
}

/// A single device entry from the Synapse devices list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynapseDevice {
    pub device_id: String,
    pub display_name: Option<String>,
    pub last_seen_ip: Option<String>,
    /// Milliseconds since epoch.
    pub last_seen_ts: Option<i64>,
}

/// Response from GET /_synapse/admin/v2/users/@user:domain/devices
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynapseDeviceList {
    pub devices: Vec<SynapseDevice>,
    pub total: Option<u64>,
}
