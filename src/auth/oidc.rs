use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreIdToken, CoreProviderMetadata},
    reqwest::async_http_client,
    AuthorizationCode, ClientId, ClientSecret, CsrfToken, IssuerUrl, Nonce,
    PkceCodeChallenge, PkceCodeVerifier, RedirectUrl, Scope, TokenResponse,
};
use serde::{Deserialize, Serialize};

use crate::{config::OidcConfig, error::AppError};

/// State stored in a short-lived cookie during the OIDC authorization code flow.
#[derive(Serialize, Deserialize)]
pub struct OidcFlowState {
    pub csrf_token: String,
    pub pkce_verifier: String,
    pub nonce: String,
}

pub const OIDC_FLOW_COOKIE: &str = "oidc_flow";

/// Thin wrapper around the openidconnect CoreClient.
pub struct OidcClient {
    inner: CoreClient,
    required_admin_role: String,
}

impl OidcClient {
    /// Discover the OIDC provider metadata and build the client.
    /// Called once at startup; panics if discovery fails.
    pub async fn init(config: &OidcConfig, required_admin_role: &str) -> Result<Self, AppError> {
        let issuer_url = IssuerUrl::new(config.issuer_url.clone()).map_err(|e| {
            AppError::Internal(anyhow::anyhow!("Invalid OIDC issuer URL: {e}"))
        })?;

        let provider_metadata =
            CoreProviderMetadata::discover_async(issuer_url, async_http_client)
                .await
                .map_err(|e| {
                    AppError::Internal(anyhow::anyhow!("OIDC discovery failed: {e}"))
                })?;

        let client = CoreClient::from_provider_metadata(
            provider_metadata,
            ClientId::new(config.client_id.clone()),
            Some(ClientSecret::new(config.client_secret.clone())),
        )
        .set_redirect_uri(
            RedirectUrl::new(config.redirect_url.clone())
                .map_err(|e| AppError::Internal(anyhow::anyhow!("Invalid redirect URL: {e}")))?,
        );

        Ok(Self {
            inner: client,
            required_admin_role: required_admin_role.to_string(),
        })
    }

    /// Build the authorization URL and return it along with the flow state
    /// that must be persisted in a cookie.
    pub fn begin_auth(&self) -> (String, OidcFlowState) {
        let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
        let nonce = Nonce::new_random();

        let (auth_url, csrf_token, _nonce) = self
            .inner
            .authorize_url(
                CoreAuthenticationFlow::AuthorizationCode,
                CsrfToken::new_random,
                move || nonce.clone(),
            )
            .add_scope(Scope::new("openid".to_string()))
            .add_scope(Scope::new("profile".to_string()))
            .add_scope(Scope::new("email".to_string()))
            .set_pkce_challenge(pkce_challenge)
            .url();

        let flow_state = OidcFlowState {
            csrf_token: csrf_token.secret().clone(),
            pkce_verifier: URL_SAFE_NO_PAD.encode(pkce_verifier.secret()),
            nonce: _nonce.secret().clone(),
        };

        (auth_url.to_string(), flow_state)
    }

    /// Exchange the authorization code for tokens, validate the ID token,
    /// and return the extracted claims.
    pub async fn complete_auth(
        &self,
        code: String,
        flow_state: OidcFlowState,
        returned_state: String,
    ) -> Result<AuthenticatedClaims, AppError> {
        // Verify the CSRF state parameter.
        if returned_state != flow_state.csrf_token {
            return Err(AppError::Auth("OIDC state mismatch".to_string()));
        }

        let verifier_bytes = URL_SAFE_NO_PAD
            .decode(&flow_state.pkce_verifier)
            .map_err(|_| AppError::Auth("Invalid PKCE verifier encoding".to_string()))?;
        let verifier_secret =
            String::from_utf8(verifier_bytes).map_err(|_| AppError::Auth("Invalid PKCE verifier".to_string()))?;
        let pkce_verifier = PkceCodeVerifier::new(verifier_secret);
        let nonce = Nonce::new(flow_state.nonce);

        let token_response = self
            .inner
            .exchange_code(AuthorizationCode::new(code))
            .set_pkce_verifier(pkce_verifier)
            .request_async(async_http_client)
            .await
            .map_err(|e| AppError::Auth(format!("Token exchange failed: {e}")))?;

        let id_token = token_response
            .id_token()
            .ok_or_else(|| AppError::Auth("No ID token in response".to_string()))?;

        let claims = id_token
            .claims(&self.inner.id_token_verifier(), &nonce)
            .map_err(|e| AppError::Auth(format!("ID token validation failed: {e}")))?;

        let subject = claims.subject().to_string();

        // Decode the raw JWT payload to access Keycloak-specific claims
        // (preferred_username, realm_access). The signature was already verified
        // above via claims(), so it is safe to trust the decoded payload.
        let extra = decode_jwt_payload(id_token);

        let username = extra
            .as_ref()
            .and_then(|v| v.get("preferred_username"))
            .and_then(|v| v.as_str())
            .unwrap_or(&subject)
            .to_string();

        let email = extra
            .as_ref()
            .and_then(|v| v.get("email"))
            .and_then(|v| v.as_str())
            .map(str::to_string);

        let roles: Vec<String> = extra
            .as_ref()
            .and_then(|v| v.get("realm_access"))
            .and_then(|v| v.get("roles"))
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();

        tracing::debug!(payload = ?extra, roles = ?roles, required = %self.required_admin_role, "Checking admin role");

        if !roles.contains(&self.required_admin_role) {
            return Err(AppError::Auth(format!(
                "Access denied: required role '{}' not present",
                self.required_admin_role
            )));
        }

        Ok(AuthenticatedClaims {
            subject,
            username,
            email,
            roles,
        })
    }

    /// Build a stub `OidcClient` for use in tests.
    ///
    /// Constructs a `CoreClient` from a minimal in-memory provider metadata
    /// document so that `AppState` can be built in tests without network
    /// access. The inner client is never exercised by invite handler tests —
    /// auth routes are not included in the test routers.
    #[cfg(test)]
    pub fn new_stub() -> Self {
        let meta: CoreProviderMetadata = serde_json::from_str(
            r#"{
                "issuer": "http://localhost",
                "authorization_endpoint": "http://localhost/auth",
                "jwks_uri": "http://localhost/jwks",
                "response_types_supported": ["code"],
                "subject_types_supported": ["public"],
                "id_token_signing_alg_values_supported": ["RS256"]
            }"#,
        )
        .expect("stub OIDC provider metadata must be valid");

        let inner = CoreClient::from_provider_metadata(
            meta,
            ClientId::new("test-client".to_string()),
            None,
        );

        Self {
            inner,
            required_admin_role: "matrix-admin".to_string(),
        }
    }
}

/// Decode the payload segment of a JWT into a JSON value.
/// openidconnect's IdToken serializes as the raw JWT string; we decode
/// the middle (payload) segment here.
fn decode_jwt_payload(id_token: &CoreIdToken) -> Option<serde_json::Value> {
    // IdToken serializes as a JSON string containing the raw JWT.
    let jwt_json = serde_json::to_string(id_token).ok()?;
    // Strip the surrounding JSON quotes to get the raw JWT string.
    let jwt_str = jwt_json.trim_matches('"');
    let payload_b64 = jwt_str.splitn(3, '.').nth(1)?;
    let payload_bytes = URL_SAFE_NO_PAD.decode(payload_b64).ok()?;
    serde_json::from_slice(&payload_bytes).ok()
}

#[derive(Debug, Clone)]
pub struct AuthenticatedClaims {
    pub subject: String,
    pub username: String,
    pub email: Option<String>,
    pub roles: Vec<String>,
}
