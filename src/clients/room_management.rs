use async_trait::async_trait;

use crate::error::AppError;

/// Abstraction over Matrix room membership enforcement operations.
///
/// Implemented by [`crate::clients::SynapseClient`] today; future Matrix server
/// connectors (Dendrite, proxy gateways) implement this trait without needing to
/// implement the full Synapse admin API surface.
#[async_trait]
pub trait RoomManagementApi: Send + Sync {
    /// Returns the Matrix IDs of all current members of the given room.
    async fn get_joined_members(&self, room_id: &str) -> Result<Vec<String>, AppError>;

    /// Force-joins `user_id` into `room_id` via the server admin API.
    ///
    /// The user is added immediately without requiring an invite acceptance.
    async fn force_join_user(&self, user_id: &str, room_id: &str) -> Result<(), AppError>;

    /// Kicks `user_id` from `room_id` with the given reason string.
    ///
    /// The admin user must already be a member of the room when using the
    /// Matrix client API kick endpoint.
    async fn kick_user(&self, user_id: &str, room_id: &str, reason: &str) -> Result<(), AppError>;
}
