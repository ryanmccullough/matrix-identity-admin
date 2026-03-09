use async_trait::async_trait;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{
    config::MasConfig,
    error::{upstream_error, AppError},
    models::mas::{MasSession, MasUser},
};

struct CachedToken {
    access_token: String,
    expires_at: std::time::Instant,
}

#[async_trait]
pub trait AuthService: Send + Sync {
    /// Look up a MAS user by their username (matches Keycloak username).
    async fn get_user_by_username(&self, username: &str) -> Result<Option<MasUser>, AppError>;
    /// List active compat + OAuth2 sessions for a MAS user (by MAS ULID).
    async fn list_sessions(&self, mas_user_id: &str) -> Result<Vec<MasSession>, AppError>;
    /// Finish a session. `session_type` must be "compat" or "oauth2".
    async fn finish_session(&self, session_id: &str, session_type: &str) -> Result<(), AppError>;
    /// Deactivate a MAS user by their MAS ULID, revoking all sessions.
    /// Note: does not free the email address — see TODO in the implementation.
    async fn delete_user(&self, mas_user_id: &str) -> Result<(), AppError>;
    /// Reactivate a previously deactivated MAS user by their MAS ULID.
    async fn reactivate_user(&self, mas_user_id: &str) -> Result<(), AppError>;
}

// ── JSON:API response structs (internal to this module) ───────────────────────

#[derive(Deserialize)]
struct ApiSingleResponse<T> {
    data: T,
}

#[derive(Deserialize)]
struct ApiListResponse<T> {
    data: Vec<T>,
}

#[derive(Deserialize)]
struct ApiUserResource {
    id: String,
    attributes: ApiUserAttributes,
}

#[derive(Deserialize)]
struct ApiUserAttributes {
    username: String,
    deactivated_at: Option<String>,
}

#[derive(Deserialize)]
struct ApiSessionResource {
    id: String,
    #[serde(rename = "type")]
    resource_type: String,
    attributes: ApiSessionAttributes,
}

#[derive(Deserialize)]
struct ApiSessionAttributes {
    created_at: Option<String>,
    last_active_at: Option<String>,
    user_agent: Option<String>,
    last_active_ip: Option<String>,
    finished_at: Option<String>,
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: u64,
}

// ── Client ────────────────────────────────────────────────────────────────────

pub struct MasClient {
    http: reqwest::Client,
    config: MasConfig,
    token_cache: Arc<Mutex<Option<CachedToken>>>,
}

impl MasClient {
    pub fn new(config: MasConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build MAS HTTP client");
        Self {
            http,
            config,
            token_cache: Arc::new(Mutex::new(None)),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.config.base_url)
    }

    /// Obtain a valid admin access token via client credentials, using cache when possible.
    async fn admin_token(&self) -> Result<String, AppError> {
        let mut cache = self.token_cache.lock().await;

        if let Some(ref cached) = *cache {
            if cached.expires_at > std::time::Instant::now() {
                return Ok(cached.access_token.clone());
            }
        }

        let token_url = self.url("/oauth2/token");

        let resp: TokenResponse = self
            .http
            .post(&token_url)
            .basic_auth(
                &self.config.admin_client_id,
                Some(&self.config.admin_client_secret),
            )
            .form(&[
                ("grant_type", "client_credentials"),
                ("scope", "urn:mas:admin"),
            ])
            .send()
            .await
            .map_err(|e| upstream_error("mas", e))?
            .error_for_status()
            .map_err(|e| upstream_error("mas", e))?
            .json()
            .await
            .map_err(|e| upstream_error("mas", e))?;

        // Subtract 30 s from expiry as a safety margin.
        let expires_at = std::time::Instant::now()
            + std::time::Duration::from_secs(resp.expires_in.saturating_sub(30));

        *cache = Some(CachedToken {
            access_token: resp.access_token.clone(),
            expires_at,
        });

        Ok(resp.access_token)
    }
}

#[async_trait]
impl AuthService for MasClient {
    async fn get_user_by_username(&self, username: &str) -> Result<Option<MasUser>, AppError> {
        let token = self.admin_token().await?;
        let url = self.url(&format!("/api/admin/v1/users/by-username/{username}"));

        let resp = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("mas", e))?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let body: ApiSingleResponse<ApiUserResource> = resp
            .error_for_status()
            .map_err(|e| upstream_error("mas", e))?
            .json()
            .await
            .map_err(|e| upstream_error("mas", e))?;

        Ok(Some(MasUser {
            id: body.data.id,
            username: body.data.attributes.username,
            deactivated_at: body.data.attributes.deactivated_at,
        }))
    }

    async fn list_sessions(&self, mas_user_id: &str) -> Result<Vec<MasSession>, AppError> {
        // Fetch active compat sessions and active OAuth2 sessions concurrently.
        let (compat_result, oauth2_result) = tokio::join!(
            self.fetch_sessions("compat-sessions", mas_user_id),
            self.fetch_sessions("oauth2-sessions", mas_user_id),
        );

        let mut sessions = compat_result.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to fetch MAS compat sessions");
            vec![]
        });
        sessions.extend(oauth2_result.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to fetch MAS OAuth2 sessions");
            vec![]
        }));

        Ok(sessions)
    }

    async fn finish_session(&self, session_id: &str, session_type: &str) -> Result<(), AppError> {
        let token = self.admin_token().await?;
        let path = match session_type {
            "compat" => format!("/api/admin/v1/compat-sessions/{session_id}/finish"),
            "oauth2" => format!("/api/admin/v1/oauth2-sessions/{session_id}/finish"),
            other => {
                return Err(AppError::Validation(format!(
                    "Unknown MAS session type: {other}"
                )))
            }
        };

        self.http
            .post(self.url(&path))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("mas", e))?
            .error_for_status()
            .map_err(|e| upstream_error("mas", e))?;

        Ok(())
    }

    async fn delete_user(&self, mas_user_id: &str) -> Result<(), AppError> {
        // TODO: MAS does not expose a hard-delete endpoint via its admin REST API.
        // `POST /deactivate` locks the account and revokes all sessions, but the
        // user record (including their email address) remains in the MAS database.
        // This means the email cannot be re-invited until the record is removed
        // directly from the MAS database (users + user_emails tables).
        // A permanent solution would either:
        //   a) use direct DB access on the MAS host after deactivation, or
        //   b) wait for MAS to expose a hard-delete endpoint in a future release.
        let token = self.admin_token().await?;
        let url = self.url(&format!("/api/admin/v1/users/{mas_user_id}/deactivate"));

        self.http
            .post(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("mas", e))?
            .error_for_status()
            .map_err(|e| upstream_error("mas", e))?;

        Ok(())
    }

    async fn reactivate_user(&self, mas_user_id: &str) -> Result<(), AppError> {
        let token = self.admin_token().await?;
        let url = self.url(&format!("/api/admin/v1/users/{mas_user_id}/reactivate"));

        self.http
            .post(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("mas", e))?
            .error_for_status()
            .map_err(|e| upstream_error("mas", e))?;

        Ok(())
    }
}

impl MasClient {
    /// Fetch active sessions from a single session endpoint (compat or oauth2).
    async fn fetch_sessions(
        &self,
        endpoint: &str,
        mas_user_id: &str,
    ) -> Result<Vec<MasSession>, AppError> {
        let token = self.admin_token().await?;
        let url = self.url(&format!("/api/admin/v1/{endpoint}"));

        let body: ApiListResponse<ApiSessionResource> = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .query(&[
                ("filter[user]", mas_user_id),
                ("filter[status]", "active"),
                ("page[first]", "50"),
            ])
            .send()
            .await
            .map_err(|e| upstream_error("mas", e))?
            .error_for_status()
            .map_err(|e| upstream_error("mas", e))?
            .json()
            .await
            .map_err(|e| upstream_error("mas", e))?;

        Ok(body
            .data
            .into_iter()
            .map(|r| MasSession {
                id: r.id,
                session_type: r.resource_type,
                created_at: r.attributes.created_at,
                last_active_at: r.attributes.last_active_at,
                user_agent: r.attributes.user_agent,
                ip_address: r.attributes.last_active_ip,
                finished_at: r.attributes.finished_at,
            })
            .collect())
    }
}
