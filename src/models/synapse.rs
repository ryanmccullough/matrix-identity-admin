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

/// A single room entry from the Synapse admin room list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomListEntry {
    pub room_id: String,
    pub name: Option<String>,
    pub canonical_alias: Option<String>,
    pub joined_members: Option<i64>,
}

/// Paginated response from GET /_synapse/admin/v1/rooms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomList {
    pub rooms: Vec<RoomListEntry>,
    pub next_batch: Option<String>,
    pub total_rooms: Option<i64>,
}

/// Detailed room info from GET /_synapse/admin/v1/rooms/{room_id}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomDetails {
    pub room_id: String,
    pub name: Option<String>,
    pub canonical_alias: Option<String>,
    pub topic: Option<String>,
    pub joined_members: Option<i64>,
    #[serde(default)]
    pub is_space: bool,
}
