use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{
    config::SynapseConfig,
    error::{upstream_error, AppError},
    models::synapse::{RoomDetails, RoomList, SynapseDevice, SynapseUser},
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

    /// List rooms known to the server (paginated).
    async fn list_rooms(&self, limit: u32, from: Option<&str>) -> Result<RoomList, AppError>;

    /// Get details for a specific room.
    async fn get_room_details(&self, room_id: &str) -> Result<RoomDetails, AppError>;

    /// Set a user's power level in a room.
    async fn set_power_level(
        &self,
        room_id: &str,
        user_id: &str,
        level: i64,
    ) -> Result<(), AppError>;
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

    /// Obtain a valid admin access token.
    ///
    /// If `SYNAPSE_ADMIN_TOKEN` is configured (MSC3861 mode), returns it directly —
    /// this static token bypasses MAS introspection and is accepted by Synapse for
    /// both admin API and client API calls.
    ///
    /// Otherwise, falls back to `m.login.password` with caching (non-MSC3861 mode).
    async fn admin_token(&self) -> Result<String, AppError> {
        // MSC3861 static token — no login needed.
        if let Some(ref token) = self.config.admin_token {
            return Ok(token.clone());
        }

        // Fallback: m.login.password with token caching.
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

    async fn list_rooms(&self, limit: u32, from: Option<&str>) -> Result<RoomList, AppError> {
        let token = self.admin_token().await?;
        let mut url = format!(
            "{}/_synapse/admin/v1/rooms?limit={limit}",
            self.config.base_url
        );
        if let Some(from) = from {
            url.push_str(&format!("&from={from}"));
        }

        let resp: RoomList = self
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

        Ok(resp)
    }

    async fn get_room_details(&self, room_id: &str) -> Result<RoomDetails, AppError> {
        let token = self.admin_token().await?;
        let encoded = urlencoded(room_id);
        let url = self.url(&format!("/_synapse/admin/v1/rooms/{encoded}"));

        let mut details: RoomDetails = self
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

        // Check if this room is a space by looking for m.space.child events.
        if let Ok(children) = self.get_space_children(room_id).await {
            details.is_space = !children.is_empty();
        }

        Ok(details)
    }

    async fn set_power_level(
        &self,
        room_id: &str,
        user_id: &str,
        level: i64,
    ) -> Result<(), AppError> {
        let token = self.admin_token().await?;
        let encoded = urlencoded(room_id);
        let url = self.url(&format!(
            "/_matrix/client/v3/rooms/{encoded}/state/m.room.power_levels"
        ));

        // Get current power levels.
        let mut power_levels: serde_json::Value = self
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

        // Update the user's power level.
        if let Some(obj) = power_levels.as_object_mut() {
            let users = obj.entry("users").or_insert_with(|| serde_json::json!({}));
            if let Some(users_obj) = users.as_object_mut() {
                users_obj.insert(user_id.to_string(), serde_json::json!(level));
            }
        }

        // PUT back the updated power levels.
        self.http
            .put(&url)
            .bearer_auth(&token)
            .json(&power_levels)
            .send()
            .await
            .map_err(|e| upstream_error("synapse", e))?
            .error_for_status()
            .map_err(|e| upstream_error("synapse", e))?;

        Ok(())
    }
}

/// Percent-encode a string for safe use in a URL path segment.
fn urlencoded(s: &str) -> String {
    use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
    utf8_percent_encode(s, NON_ALPHANUMERIC).to_string()
}
