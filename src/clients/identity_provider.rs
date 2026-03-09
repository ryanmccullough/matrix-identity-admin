use async_trait::async_trait;

use crate::{error::AppError, models::unified::CanonicalUser};

/// Abstraction over an identity provider (Keycloak, LDAP, SCIM, etc.).
///
/// All methods return domain types — no upstream-specific structs. Implement
/// this trait for each backend. `UserService` depends on this trait, not on
/// any concrete provider client, enabling pluggable identity backends in Phase 3.
#[async_trait]
pub trait IdentityProvider: Send + Sync {
    /// Search for users matching `query` with server-side pagination.
    async fn search_users(
        &self,
        query: &str,
        max: u32,
        first: u32,
    ) -> Result<Vec<CanonicalUser>, AppError>;

    /// Fetch a single user by their provider-internal ID.
    async fn get_user(&self, id: &str) -> Result<CanonicalUser, AppError>;

    /// Return the names of all groups the user belongs to.
    async fn get_user_groups(&self, id: &str) -> Result<Vec<String>, AppError>;

    /// Return the names of all roles assigned to the user.
    async fn get_user_roles(&self, id: &str) -> Result<Vec<String>, AppError>;

    /// Invalidate all active sessions for the user in the identity provider.
    async fn logout_user(&self, id: &str) -> Result<(), AppError>;

    /// Return the total count of users matching `query`.
    async fn count_users(&self, query: &str) -> Result<u32, AppError>;
}
