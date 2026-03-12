use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::{
    clients::identity_provider::IdentityProvider,
    config::KeycloakConfig,
    error::{upstream_error, AppError},
    models::{
        keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
        unified::CanonicalUser,
    },
};

#[async_trait]
pub trait KeycloakIdentityProvider: Send + Sync {
    async fn search_users(
        &self,
        query: &str,
        max: u32,
        first: u32,
    ) -> Result<Vec<KeycloakUser>, AppError>;
    /// Return the total count of users matching `query` (calls `/users/count?search=...`).
    async fn count_users(&self, query: &str) -> Result<u32, AppError>;
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
    /// Disable a user account in Keycloak (sets enabled = false).
    async fn disable_user(&self, user_id: &str) -> Result<(), AppError>;
    /// Enable a user account in Keycloak (sets enabled = true).
    async fn enable_user(&self, user_id: &str) -> Result<(), AppError>;

    /// List all groups in the realm.
    async fn list_groups(&self) -> Result<Vec<KeycloakGroup>, AppError>;

    /// List all realm-level roles.
    async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>, AppError>;

    /// Assigns a user to a Keycloak group by group ID.
    async fn add_user_to_group(&self, user_id: &str, group_id: &str) -> Result<(), AppError>;

    /// Assigns realm roles to a user.
    async fn assign_realm_roles(
        &self,
        user_id: &str,
        roles: &[KeycloakRole],
    ) -> Result<(), AppError>;
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

    async fn clear_cached_token(&self) {
        *self.token_cache.lock().await = None;
    }

    async fn send_admin_request<F>(&self, mut build: F) -> Result<reqwest::Response, AppError>
    where
        F: FnMut(&str) -> reqwest::RequestBuilder,
    {
        let response = self.send_admin_request_once(&mut build).await?;
        if response.status() != reqwest::StatusCode::UNAUTHORIZED {
            return Ok(response);
        }

        tracing::warn!(
            "Keycloak admin request returned 401; refreshing cached token and retrying once"
        );
        self.clear_cached_token().await;
        self.send_admin_request_once(&mut build).await
    }

    async fn send_admin_request_once<F>(&self, build: &mut F) -> Result<reqwest::Response, AppError>
    where
        F: FnMut(&str) -> reqwest::RequestBuilder,
    {
        let token = self.admin_token().await?;
        build(&token)
            .send()
            .await
            .map_err(|e| upstream_error("keycloak", e))
    }

    fn admin_url(&self, path: &str) -> String {
        format!(
            "{}/admin/realms/{}{path}",
            self.config.base_url, self.config.realm
        )
    }
}

#[async_trait]
impl KeycloakIdentityProvider for KeycloakClient {
    async fn search_users(
        &self,
        query: &str,
        max: u32,
        first: u32,
    ) -> Result<Vec<KeycloakUser>, AppError> {
        let url = self.admin_url("/users");

        let users: Vec<KeycloakUser> = self
            .send_admin_request(|token| {
                self.http.get(&url).bearer_auth(token).query(&[
                    ("search", query),
                    ("max", &max.to_string()),
                    ("first", &first.to_string()),
                ])
            })
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(users)
    }

    async fn get_user(&self, user_id: &str) -> Result<KeycloakUser, AppError> {
        let url = self.admin_url(&format!("/users/{user_id}"));

        let user: KeycloakUser = self
            .send_admin_request(|token| self.http.get(&url).bearer_auth(token))
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(user)
    }

    async fn get_user_groups(&self, user_id: &str) -> Result<Vec<KeycloakGroup>, AppError> {
        let url = self.admin_url(&format!("/users/{user_id}/groups"));

        let groups: Vec<KeycloakGroup> = self
            .send_admin_request(|token| self.http.get(&url).bearer_auth(token))
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(groups)
    }

    async fn get_user_roles(&self, user_id: &str) -> Result<Vec<KeycloakRole>, AppError> {
        // Realm-level role mappings
        let url = self.admin_url(&format!("/users/{user_id}/role-mappings/realm"));

        let roles: Vec<KeycloakRole> = self
            .send_admin_request(|token| self.http.get(&url).bearer_auth(token))
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(roles)
    }

    async fn logout_user(&self, user_id: &str) -> Result<(), AppError> {
        let url = self.admin_url(&format!("/users/{user_id}/logout"));

        self.send_admin_request(|token| self.http.post(&url).bearer_auth(token))
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }

    async fn get_user_by_email(&self, email: &str) -> Result<Option<KeycloakUser>, AppError> {
        let url = self.admin_url("/users");

        let users: Vec<KeycloakUser> = self
            .send_admin_request(|token| {
                self.http
                    .get(&url)
                    .bearer_auth(token)
                    .query(&[("email", email), ("exact", "true")])
            })
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(users.into_iter().next())
    }

    async fn create_user(&self, username: &str, email: &str) -> Result<String, AppError> {
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
            .send_admin_request(|token| {
                self.http
                    .post(&url)
                    .bearer_auth(token)
                    .json(&CreateUserBody {
                        username,
                        email,
                        enabled: true,
                        email_verified: false,
                        // UPDATE_PROFILE prompts the user to choose their username
                        // (requires "Edit username" = ON in Keycloak realm settings).
                        required_actions: &["UPDATE_PASSWORD", "UPDATE_PROFILE", "VERIFY_EMAIL"],
                    })
            })
            .await?
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
        let url = self.admin_url(&format!("/users/{user_id}/execute-actions-email"));

        self.send_admin_request(|token| {
            self.http.put(&url).bearer_auth(token).json(&[
                "UPDATE_PASSWORD",
                "UPDATE_PROFILE",
                "VERIFY_EMAIL",
            ])
        })
        .await?
        .error_for_status()
        .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }

    async fn delete_user(&self, user_id: &str) -> Result<(), AppError> {
        let url = self.admin_url(&format!("/users/{user_id}"));

        self.send_admin_request(|token| self.http.delete(&url).bearer_auth(token))
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }

    async fn disable_user(&self, user_id: &str) -> Result<(), AppError> {
        let url = self.admin_url(&format!("/users/{user_id}"));

        #[derive(Serialize)]
        struct DisableBody {
            enabled: bool,
        }

        self.send_admin_request(|token| {
            self.http
                .put(&url)
                .bearer_auth(token)
                .json(&DisableBody { enabled: false })
        })
        .await?
        .error_for_status()
        .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }

    async fn enable_user(&self, user_id: &str) -> Result<(), AppError> {
        let url = self.admin_url(&format!("/users/{user_id}"));

        #[derive(Serialize)]
        struct EnableBody {
            enabled: bool,
        }

        self.send_admin_request(|token| {
            self.http
                .put(&url)
                .bearer_auth(token)
                .json(&EnableBody { enabled: true })
        })
        .await?
        .error_for_status()
        .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }

    async fn count_users(&self, query: &str) -> Result<u32, AppError> {
        let url = self.admin_url("/users/count");

        let count: u32 = self
            .send_admin_request(|token| {
                let req = self.http.get(&url).bearer_auth(token);
                if query.is_empty() {
                    req
                } else {
                    req.query(&[("search", query)])
                }
            })
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(count)
    }

    async fn list_groups(&self) -> Result<Vec<KeycloakGroup>, AppError> {
        let url = format!(
            "{}/admin/realms/{}/groups?briefRepresentation=true",
            self.config.base_url, self.config.realm
        );

        let groups: Vec<KeycloakGroup> = self
            .send_admin_request(|token| self.http.get(&url).bearer_auth(token))
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(groups)
    }

    async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>, AppError> {
        let url = format!(
            "{}/admin/realms/{}/roles?briefRepresentation=true",
            self.config.base_url, self.config.realm
        );

        let roles: Vec<KeycloakRole> = self
            .send_admin_request(|token| self.http.get(&url).bearer_auth(token))
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?
            .json()
            .await
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(roles)
    }

    async fn add_user_to_group(&self, user_id: &str, group_id: &str) -> Result<(), AppError> {
        let url = self.admin_url(&format!("/users/{user_id}/groups/{group_id}"));

        self.send_admin_request(|token| self.http.put(&url).bearer_auth(token))
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }

    async fn assign_realm_roles(
        &self,
        user_id: &str,
        roles: &[KeycloakRole],
    ) -> Result<(), AppError> {
        if roles.is_empty() {
            return Ok(());
        }

        let url = self.admin_url(&format!("/users/{user_id}/role-mappings/realm"));

        self.send_admin_request(|token| self.http.post(&url).bearer_auth(token).json(&roles))
            .await?
            .error_for_status()
            .map_err(|e| upstream_error("keycloak", e))?;

        Ok(())
    }
}

#[async_trait]
impl IdentityProvider for KeycloakClient {
    async fn search_users(
        &self,
        query: &str,
        max: u32,
        first: u32,
    ) -> Result<Vec<CanonicalUser>, AppError> {
        let kc_users = KeycloakIdentityProvider::search_users(self, query, max, first).await?;
        Ok(kc_users
            .into_iter()
            .map(|u| CanonicalUser {
                id: u.id,
                username: u.username,
                email: u.email,
                first_name: u.first_name,
                last_name: u.last_name,
                enabled: u.enabled,
                groups: vec![],
                roles: vec![],
                required_actions: u.required_actions,
            })
            .collect())
    }

    async fn get_user(&self, id: &str) -> Result<CanonicalUser, AppError> {
        let u = KeycloakIdentityProvider::get_user(self, id).await?;
        Ok(CanonicalUser {
            id: u.id,
            username: u.username,
            email: u.email,
            first_name: u.first_name,
            last_name: u.last_name,
            enabled: u.enabled,
            groups: vec![],
            roles: vec![],
            required_actions: u.required_actions,
        })
    }

    async fn get_user_groups(&self, id: &str) -> Result<Vec<String>, AppError> {
        let groups = KeycloakIdentityProvider::get_user_groups(self, id).await?;
        Ok(groups.into_iter().map(|g| g.name).collect())
    }

    async fn get_user_roles(&self, id: &str) -> Result<Vec<String>, AppError> {
        let roles = KeycloakIdentityProvider::get_user_roles(self, id).await?;
        Ok(roles.into_iter().map(|r| r.name).collect())
    }

    async fn logout_user(&self, id: &str) -> Result<(), AppError> {
        KeycloakIdentityProvider::logout_user(self, id).await
    }

    async fn count_users(&self, query: &str) -> Result<u32, AppError> {
        KeycloakIdentityProvider::count_users(self, query).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };

    use axum::{
        extract::State,
        http::{header::AUTHORIZATION, HeaderMap, StatusCode},
        response::{IntoResponse, Json},
        routing::{get, post},
        Router,
    };
    use serde_json::json;

    use super::*;

    #[derive(Clone)]
    struct TestState {
        token_requests: Arc<AtomicUsize>,
        user_requests: Arc<AtomicUsize>,
    }

    fn sample_user() -> KeycloakUser {
        KeycloakUser {
            id: "kc-1".to_string(),
            username: "alice".to_string(),
            email: Some("alice@example.com".to_string()),
            first_name: None,
            last_name: None,
            enabled: true,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    async fn token_handler(State(state): State<TestState>) -> Json<serde_json::Value> {
        state.token_requests.fetch_add(1, Ordering::SeqCst);
        Json(json!({
            "access_token": "fresh-token",
            "expires_in": 300u64,
        }))
    }

    async fn users_handler(
        State(state): State<TestState>,
        headers: HeaderMap,
    ) -> impl IntoResponse {
        state.user_requests.fetch_add(1, Ordering::SeqCst);
        let auth = headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("");

        match auth {
            "Bearer stale-token" => StatusCode::UNAUTHORIZED.into_response(),
            "Bearer fresh-token" => Json(vec![sample_user()]).into_response(),
            _ => StatusCode::UNAUTHORIZED.into_response(),
        }
    }

    #[tokio::test]
    async fn search_users_refreshes_cached_token_after_401_and_reuses_new_token() {
        let state = TestState {
            token_requests: Arc::new(AtomicUsize::new(0)),
            user_requests: Arc::new(AtomicUsize::new(0)),
        };
        let app = Router::new()
            .route(
                "/realms/test/protocol/openid-connect/token",
                post(token_handler),
            )
            .route("/admin/realms/test/users", get(users_handler))
            .with_state(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = KeycloakClient::new(KeycloakConfig {
            base_url: format!("http://{addr}"),
            realm: "test".to_string(),
            admin_client_id: "admin-cli".to_string(),
            admin_client_secret: "secret".to_string(),
        });

        *client.token_cache.lock().await = Some(CachedToken {
            access_token: "stale-token".to_string(),
            expires_at: std::time::Instant::now() + std::time::Duration::from_secs(300),
        });

        let first = KeycloakIdentityProvider::search_users(&client, "", 100, 0)
            .await
            .unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(state.token_requests.load(Ordering::SeqCst), 1);
        assert_eq!(state.user_requests.load(Ordering::SeqCst), 2);

        let second = KeycloakIdentityProvider::search_users(&client, "", 100, 0)
            .await
            .unwrap();
        assert_eq!(second.len(), 1);
        assert_eq!(state.token_requests.load(Ordering::SeqCst), 1);
        assert_eq!(state.user_requests.load(Ordering::SeqCst), 3);
    }
}
