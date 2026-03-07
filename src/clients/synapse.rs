use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{
    config::SynapseConfig,
    error::{upstream_error, AppError},
    models::synapse::{SynapseDevice, SynapseUser},
};

#[async_trait]
pub trait SynapseApi: Send + Sync {
    /// Look up a Matrix user via the Synapse admin API.
    /// `matrix_id` is the fully-qualified user ID, e.g. `@alice:example.com`.
    async fn get_user(&self, matrix_id: &str) -> Result<Option<SynapseUser>, AppError>;
    /// List all devices for a Matrix user.
    async fn list_devices(&self, matrix_id: &str) -> Result<Vec<SynapseDevice>, AppError>;
    /// Delete a specific device for a Matrix user.
    async fn delete_device(&self, matrix_id: &str, device_id: &str) -> Result<(), AppError>;
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
        let expires_at =
            std::time::Instant::now() + std::time::Duration::from_secs(4 * 60);

        *cache = Some(CachedToken {
            access_token: resp.access_token.clone(),
            expires_at,
        });

        Ok(resp.access_token)
    }
}

#[async_trait]
impl SynapseApi for SynapseClient {
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
