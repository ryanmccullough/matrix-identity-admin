pub mod identity_provider;
pub mod keycloak;
pub mod mas;
pub mod room_management;
pub mod synapse;

pub use identity_provider::IdentityProviderApi;
pub use keycloak::{IdentityProvider, KeycloakClient};
pub use mas::{AuthService, MasClient};
pub use room_management::RoomManagementApi;
pub use synapse::{MatrixService, SynapseClient};
