use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{
    config::KeycloakConfig,
    error::{upstream_error, AppError},
    models::keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
};

#[async_trait]
pub trait KeycloakApi: Send + Sync {
    async fn search_users(&self, query: &str) -> Result<Vec<KeycloakUser>, AppError>;
    async fn get_user(&self, user_id: &str) -> Result<KeycloakUser, AppError>;
    async fn get_user_by_email(&self, email: &str) -> Result<Option<KeycloakUser>, AppError>;
    async fn get_user_groups(&self, user_id: &str) -> Result<Vec<KeycloakGroup>, AppError>;
    async fn get_user_roles(&self, user_id: &str) -> Result<Vec<KeycloakRole>, AppError>;
    async fn logout_user(&self, user_id: &str) -> Result<(), AppError>;
    /// Create a new enabled user with required actions; returns the new Keycloak user ID.
    async fn create_user(&self, username: &str, email: &str) -> Result<String, AppError>;
    /// Trigger Keycloak to email the user a set-password + verify-email link.
    async fn send_invite_email(&self, user_id: &str) -> Result<(), AppError>;
    /// Permanently delete a user from Keycloak.
    async fn delete_user(&self, user_id: &str) -> Result<(), AppError>;
    /// Count users matching an optional search query. Pass `""` for the total count.
    async fn count_users(&self, query: &str) -> Result<u32, AppError>;
}

struct CachedToken {
    access_token: String,
    expires_at: std::time::Instant,
}

pub struct KeycloakClient {
    http: reqwest::Client,
    config: KeycloakConfig,
    /// Cached admin service account token.
    token_cache: Arc<Mutex<Option<CachedToken>>>,
}

impl KeycloakClient {
    pub fn new(config: KeycloakConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("Failed to build Keycloak HTTP client");
        Self {
            http,
            config,
            token_cache: Arc::new(Mutex::new(None)),
        }
    }

    /// Obtain a valid admin access token, using the cache when possible.
    async fn admin_token(&self) -> Result<String, AppError> {
        let mut cache = self.token_cache.lock().await;

        if let Some(ref cached) = *cache {
            if cached.expires_at > std::time::Instant::now() {
                return Ok(cached.access_token.clone());
            }
        }

        // Fetch a new token via client credentials.
        let token_url = format!(
            "{}/realms/{}/protocol/openid-connect/token",
            self.config.base_url, self.config.realm
        );

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: u64,
        }

        let resp: TokenResponse = self
            .http
            .post(&token_url)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &self.config.admin_client_id),
                ("client_secret", &self.config.admin_client_secret),
            ])
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        // Subtract 30 s from expiry as a safety margin.
        let expires_at = std::time::Instant::now()
            + std::time::Duration::from_secs(resp.expires_in.saturating_sub(30));

        *cache = Some(CachedToken {
            access_token: resp.access_token.clone(),
            expires_at,
        });

        Ok(resp.access_token)
    }

    fn admin_url(&self, path: &str) -> String {
        format!(
            "{}/admin/realms/{}{path}",
            self.config.base_url, self.config.realm
        )
    }
}

#[async_trait]
impl KeycloakApi for KeycloakClient {
    async fn search_users(&self, query: &str) -> Result<Vec<KeycloakUser>, AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url("/users");

        let users: Vec<KeycloakUser> = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .query(&[("search", query), ("max", "50")])
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(users)
    }

    async fn get_user(&self, user_id: &str) -> Result<KeycloakUser, AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url(&format!("/users/{user_id}"));

        let user: KeycloakUser = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(user)
    }

    async fn get_user_groups(&self, user_id: &str) -> Result<Vec<KeycloakGroup>, AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url(&format!("/users/{user_id}/groups"));

        let groups: Vec<KeycloakGroup> = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(groups)
    }

    async fn get_user_roles(&self, user_id: &str) -> Result<Vec<KeycloakRole>, AppError> {
        let token = self.admin_token().await?;
        // Realm-level role mappings
        let url = self.admin_url(&format!("/users/{user_id}/role-mappings/realm"));

        let roles: Vec<KeycloakRole> = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(roles)
    }

    async fn logout_user(&self, user_id: &str) -> Result<(), AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url(&format!("/users/{user_id}/logout"));

        self.http
            .post(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }

    async fn get_user_by_email(&self, email: &str) -> Result<Option<KeycloakUser>, AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url("/users");

        let users: Vec<KeycloakUser> = self
            .http
            .get(&url)
            .bearer_auth(&token)
            .query(&[("email", email), ("exact", "true")])
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(users.into_iter().next())
    }

    async fn create_user(&self, username: &str, email: &str) -> Result<String, AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url("/users");

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct CreateUserBody<'a> {
            username: &'a str,
            email: &'a str,
            enabled: bool,
            email_verified: bool,
            required_actions: &'a [&'a str],
        }

        let resp = self
            .http
            .post(&url)
            .bearer_auth(&token)
            .json(&CreateUserBody {
                username,
                email,
                enabled: true,
                email_verified: false,
                // UPDATE_PROFILE prompts the user to choose their username
                // (requires "Edit username" = ON in Keycloak realm settings).
                required_actions: &["UPDATE_PASSWORD", "UPDATE_PROFILE", "VERIFY_EMAIL"],
            })
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?;

        // Keycloak returns 201 with Location: .../users/{uuid}
        let location = resp
            .headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| AppError::Upstream {
                service: "keycloak".to_string(),
                message: "create_user response missing Location header".to_string(),
            })?;

        let user_id = location
            .rsplit('/')
            .next()
            .ok_or_else(|| AppError::Upstream {
                service: "keycloak".to_string(),
                message: "could not parse user ID from Location header".to_string(),
            })?
            .to_string();

        Ok(user_id)
    }

    async fn send_invite_email(&self, user_id: &str) -> Result<(), AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url(&format!("/users/{user_id}/execute-actions-email"));

        self.http
            .put(&url)
            .bearer_auth(&token)
            .json(&["UPDATE_PASSWORD", "UPDATE_PROFILE", "VERIFY_EMAIL"])
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }

    async fn delete_user(&self, user_id: &str) -> Result<(), AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url(&format!("/users/{user_id}"));

        self.http
            .delete(&url)
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }

    async fn count_users(&self, query: &str) -> Result<u32, AppError> {
        let token = self.admin_token().await?;
        let url = self.admin_url("/users/count");

        let mut req = self.http.get(&url).bearer_auth(&token);
        if !query.is_empty() {
            req = req.query(&[("search", query)]);
        }

        let count: u32 = req
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(count)
    }
}
