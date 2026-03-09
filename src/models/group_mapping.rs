use serde::Deserialize;

/// Maps a Keycloak group name to a Matrix room ID.
///
/// Loaded from the `GROUP_MAPPINGS` environment variable as a JSON array:
/// ```json
/// [{"keycloak_group":"staff","matrix_room_id":"!abc:example.com"}]
/// ```
#[derive(Debug, Clone, Deserialize)]
pub struct GroupMapping {
    pub keycloak_group: String,
    pub matrix_room_id: String,
}
