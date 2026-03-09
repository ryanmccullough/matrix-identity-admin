use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{
    clients::room_management::RoomManagementApi,
    config::SynapseConfig,
    error::{upstream_error, AppError},
    models::synapse::{SynapseDevice, SynapseUser},
};

#[async_trait]
pub trait MatrixService: Send + Sync {
    /// Look up a Matrix user via the Synapse admin API.
    /// `matrix_id` is the fully-qualified user ID, e.g. `@alice:example.com`.
    async fn get_user(&self, matrix_id: &str) -> Result<Option<SynapseUser>, AppError>;
    /// List all devices for a Matrix user.
    async fn list_devices(&self, matrix_id: &str) -> Result<Vec<SynapseDevice>, AppError>;
    /// Delete a specific device for a Matrix user.
    async fn delete_device(&self, matrix_id: &str, device_id: &str) -> Result<(), AppError>;

    // ── Room membership ───────────────────────────────────────────────────────

    /// Return the Matrix IDs of all joined members in a room.
    async fn get_joined_room_members(&self, room_id: &str) -> Result<Vec<String>, AppError>;
    /// Force-join a user into a room via the Synapse admin API.
    /// The user is added immediately without requiring an invite acceptance.
    async fn force_join_user(&self, user_id: &str, room_id: &str) -> Result<(), AppError>;
    /// Kick a user from a room via the Matrix client API.
    /// The admin user must already be a member of the room.
    async fn kick_user_from_room(
        &self,
        user_id: &str,
        room_id: &str,
        reason: &str,
    ) -> Result<(), AppError>;

    /// Return the room IDs of all direct children of a Matrix space.
    /// If the room is not a space (no `m.space.child` events), returns an empty vec.
    async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError>;
}

struct CachedToken {
    access_token: String,
    /// Tokens obtained via m.login.password don't carry an explicit expiry in the
    /// compat response, so we conservatively refresh after 4 minutes.
    expires_at: std::time::Instant,
}

pub struct SynapseClient {
    http: reqwest::Client,
    config: SynapseConfig,
    token_cache: Arc<Mutex<Option<CachedToken>>>,
}

impl SynapseClient {
    pub fn new(config: SynapseConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build Synapse HTTP client");
        Self {
            http,
            config,
            token_cache: Arc::new(Mutex::new(None)),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.base_url)
    }

    /// Obtain a valid admin access token via Matrix compat password login, caching the result.
    /// MAS compat tokens don't carry an explicit expiry in the login response, so we
    /// conservatively refresh every 4 minutes.
    async fn admin_token(&self) -> Result<String, AppError> {
        let mut cache = self.token_cache.lock().await;

        if let Some(ref cached) = *cache {
            if cached.expires_at > std::time::Instant::now() {
                return Ok(cached.access_token.clone());
            }
        }

        #[derive(Serialize)]
        struct LoginRequest<'a> {
            #[serde(rename = "type")]
            kind: &'a str,
            identifier: Identifier<'a>,
            password: &'a str,
        }

        #[derive(Serialize)]
        struct Identifier<'a> {
            #[serde(rename = "type")]
            kind: &'a str,
            user: &'a str,
        }

        #[derive(Deserialize)]
        struct LoginResponse {
            access_token: String,
        }

        let url = self.url("/_matrix/client/v3/login");

        let resp: LoginResponse = self
            .http
            .post(&url)
            .json(&LoginRequest {
                kind: "m.login.password",
                identifier: Identifier {
                    kind: "m.id.user",
                    user: &self.config.admin_user,
                },
                password: &self.config.admin_password,
            })
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?
            .json()
            .await
            .map_err(|e| upstream_error("synapse", e))?;

        // Refresh after 4 minutes — well within typical MAS token lifetime.
        let expires_at = std::time::Instant::now() + std::time::Duration::from_secs(4 * 60);

        *cache = Some(CachedToken {
            access_token: resp.access_token.clone(),
            expires_at,
        });

        Ok(resp.access_token)
    }
}

#[async_trait]
impl MatrixService for SynapseClient {
    async fn get_user(&self, matrix_id: &str) -> Result<Option<SynapseUser>, AppError> {
        let token = self.admin_token().await?;
        // Percent-encode the Matrix user ID for use in the URL path.
        let encoded = urlencoded(matrix_id);
        let url = self.url(&format!("/_synapse/admin/v2/users/{encoded}"));

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let user: SynapseUser = resp
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?
            .json()
            .await
            .map_err(|e| upstream_error("synapse", e))?;

        Ok(Some(user))
    }

    async fn list_devices(&self, matrix_id: &str) -> Result<Vec<SynapseDevice>, AppError> {
        use crate::models::synapse::SynapseDeviceList;

        let token = self.admin_token().await?;
        let encoded = urlencoded(matrix_id);
        let url = self.url(&format!("/_synapse/admin/v2/users/{encoded}/devices"));

        let list: SynapseDeviceList = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?
            .json()
            .await
            .map_err(|e| upstream_error("synapse", e))?;

        Ok(list.devices)
    }

    async fn delete_device(&self, matrix_id: &str, device_id: &str) -> Result<(), AppError> {
        let token = self.admin_token().await?;
        let encoded = urlencoded(matrix_id);
        let url = self.url(&format!(
            "/_synapse/admin/v2/users/{encoded}/devices/{device_id}"
        ));

        self.http
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?;

        Ok(())
    }

    async fn get_joined_room_members(&self, room_id: &str) -> Result<Vec<String>, AppError> {
        #[derive(Deserialize)]
        struct MembersResponse {
            members: Vec<String>,
        }

        let token = self.admin_token().await?;
        let encoded = urlencoded(room_id);
        let url = self.url(&format!("/_synapse/admin/v1/rooms/{encoded}/members"));

        let resp: MembersResponse = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?
            .json()
            .await
            .map_err(|e| upstream_error("synapse", e))?;

        Ok(resp.members)
    }

    async fn force_join_user(&self, user_id: &str, room_id: &str) -> Result<(), AppError> {
        #[derive(Serialize)]
        struct JoinRequest<'a> {
            user_id: &'a str,
        }

        let token = self.admin_token().await?;
        let encoded = urlencoded(room_id);
        let url = self.url(&format!("/_synapse/admin/v1/join/{encoded}"));

        self.http
            .post(&url)
            .bearer_auth(&token)
            .json(&JoinRequest { user_id })
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?;

        Ok(())
    }

    async fn kick_user_from_room(
        &self,
        user_id: &str,
        room_id: &str,
        reason: &str,
    ) -> Result<(), AppError> {
        #[derive(Serialize)]
        struct KickRequest<'a> {
            user_id: &'a str,
            reason: &'a str,
        }

        let token = self.admin_token().await?;
        let encoded = urlencoded(room_id);
        let url = self.url(&format!("/_matrix/client/v3/rooms/{encoded}/kick"));

        self.http
            .post(&url)
            .bearer_auth(&token)
            .json(&KickRequest { user_id, reason })
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?;

        Ok(())
    }

    async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError> {
        #[derive(Deserialize)]
        struct StateEvent {
            #[serde(rename = "type")]
            event_type: String,
            state_key: String,
            content: serde_json::Value,
        }

        #[derive(Deserialize)]
        struct StateResponse {
            state: Vec<StateEvent>,
        }

        let token = self.admin_token().await?;
        let encoded = urlencoded(space_id);
        let url = self.url(&format!("/_synapse/admin/v1/rooms/{encoded}/state"));

        let resp: StateResponse = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?
            .json()
            .await
            .map_err(|e| upstream_error("synapse", e))?;

        // m.space.child state events with non-empty content represent active children.
        // Empty content means the child was removed.
        let children = resp
            .state
            .into_iter()
            .filter(|e| {
                e.event_type == "m.space.child"
                    && !e.state_key.is_empty()
                    && e.content.as_object().is_some_and(|o| !o.is_empty())
            })
            .map(|e| e.state_key)
            .collect();

        Ok(children)
    }
}

#[async_trait]
impl RoomManagementApi for SynapseClient {
    async fn get_joined_members(&self, room_id: &str) -> Result<Vec<String>, AppError> {
        self.get_joined_room_members(room_id).await
    }

    async fn force_join_user(&self, user_id: &str, room_id: &str) -> Result<(), AppError> {
        // Delegate to the MatrixService impl — same HTTP call.
        <Self as MatrixService>::force_join_user(self, user_id, room_id).await
    }

    async fn kick_user(&self, user_id: &str, room_id: &str, reason: &str) -> Result<(), AppError> {
        self.kick_user_from_room(user_id, room_id, reason).await
    }
}

/// Percent-encode a string for safe use in a URL path segment.
/// Encodes `@` and `:` which appear in Matrix user IDs.
fn urlencoded(s: &str) -> String {
    s.chars()
        .flat_map(|c| match c {
            '@' => "%40".chars().collect::<Vec<_>>(),
            ':' => "%3A".chars().collect::<Vec<_>>(),
            _ => vec![c],
        })
        .collect()
}
