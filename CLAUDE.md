# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What This App Is

A **thin internal admin console** for a self-hosted Matrix environment running MSC3861 (Synapse delegates auth entirely to MAS). It provides a unified read-mostly view over two upstream systems:

- **Keycloak** — identity provider (source of truth for users)
- **MAS (Matrix Authentication Service)** — auth/session layer; source of truth for active sessions

Admin actions: revoke MAS sessions, force Keycloak logout. All mutations are audit-logged. The app does **not** sync, reconcile, or provision — it only observes and performs discrete admin mutations.

> **Synapse**: In MSC3861 mode, compat tokens cannot access the Synapse admin API. Session and device management flows through MAS — revoking a MAS compat session invalidates the corresponding Matrix device. Direct Synapse admin API calls are not used. The client code is preserved in `src/clients/synapse.rs` for future use (e.g. Matrix client API calls for invite management).

## Commands

```bash
# Build
cargo build

# Run (requires .env or environment variables set)
cargo run

# Run tests
cargo test

# Run a single test
cargo test <test_name>

# Check without building
cargo check

# Lint
cargo clippy

# Format
cargo fmt
```

## Stack

- **Runtime**: `tokio` (async)
- **HTTP server**: `axum` with `tower`/`tower-http` middleware
- **Outbound HTTP**: `reqwest` (typed wrappers only — no large SDKs)
- **Templates**: `askama` (server-rendered HTML) + minimal HTMX/vanilla JS
- **Auth**: `openidconnect` crate for OIDC authorization code flow against Keycloak
- **Database**: `sqlx` with SQLite (audit logs only)
- **Serialization**: `serde` / `serde_json`
- **Errors**: `thiserror` for typed errors, `anyhow` where appropriate
- **Logging**: `tracing` + `tracing-subscriber`

## Architecture

```
Admin Browser
   |
Axum Web App
   |
   ├── auth/         OIDC login flow, secure cookie sessions, CSRF
   ├── handlers/     Thin route handlers — delegate to services
   ├── services/     identity_mapper, user_service, audit_service
   ├── clients/      Typed reqwest wrappers for Keycloak and MAS
   ├── models/       Typed structs per upstream + unified app models
   ├── db/           SQLite via sqlx — audit logs only
   └── templates/    Askama HTML templates
```

### Key architectural rules

- **Handlers must be thin** — orchestrate calls to services, render templates, redirect.
- **Clients own all upstream API logic** — base URL, auth headers, typed request/response structs, error conversion. Never leak raw upstream payloads into handlers.
- **Services aggregate** — `identity_service` / `user_service` combine data from multiple clients into unified models.
- **`identity_mapper`** derives the Keycloak → MAS → Matrix ID mapping. It must clearly mark uncertain or missing correlations — never silently assert a mapping is valid when it isn't.
- **Upstream systems are source of truth** — do not persist identity state locally. SQLite is for audit logs only.

### Identity mapping model

```
Keycloak User (keycloak_user.id = stable subject)
   ↓
MAS account (correlated via OIDC subject)
   ↓
Matrix user (@{keycloak_user.username}:{homeserver_domain})
```

## Module Layout

```
src/
  main.rs
  config.rs       # Strongly typed config from env vars — fails fast on missing values
  error.rs        # Central AppError type — auth / upstream / validation / not-found / db
  state.rs        # Shared app state (clients, db pool, config)

  auth/
    mod.rs
    oidc.rs       # OIDC authorization code flow
    session.rs    # Secure cookie session management
    csrf.rs       # CSRF token generation and validation

  clients/
    mod.rs
    keycloak.rs   # KeycloakApi trait + reqwest impl
    mas.rs        # MasApi trait + reqwest impl (OAuth2 client credentials, token cache)
    synapse.rs    # SynapseApi trait + reqwest impl — NOT wired in; preserved for future use

  services/
    mod.rs
    identity_mapper.rs  # Best-effort Keycloak→MAS→Matrix correlation
    user_service.rs     # Aggregates across clients into unified models
    audit_service.rs    # Writes audit_logs entries

  handlers/
    mod.rs
    auth.rs        # /auth/login, /auth/callback, /auth/logout
    dashboard.rs   # GET /
    users.rs       # GET /users/search, GET /users/:id
    sessions.rs    # POST /users/:id/sessions/:session_id/revoke
    devices.rs     # POST /users/:id/keycloak/logout
    audit.rs       # GET /audit

  models/
    mod.rs
    keycloak.rs    # KeycloakUser, KeycloakGroup, KeycloakRole
    mas.rs         # MasUser, MasSession
    synapse.rs     # SynapseUser, SynapseDevice — NOT compiled; preserved for future use
    unified.rs     # UnifiedUserSummary, UnifiedUserDetail, UnifiedSession, CorrelationStatus
    audit.rs       # AuditLog struct

  db/
    mod.rs
    audit.rs       # sqlx queries for audit_logs table
    migrations/    # Initial migration: audit_logs table + indexes

templates/
  base.html
  login.html
  dashboard.html
  users_search.html
  user_detail.html
  audit.html

static/
  app.css
```

## Client Traits

Each upstream client is defined as an async trait (using `async_trait`) so handlers/services can be tested with mocks:

```rust
#[async_trait]
pub trait KeycloakApi {
    async fn search_users(&self, query: &str) -> Result<Vec<KeycloakUser>>;
    async fn get_user(&self, user_id: &str) -> Result<KeycloakUser>;
    async fn get_user_groups(&self, user_id: &str) -> Result<Vec<KeycloakGroup>>;
    async fn get_user_roles(&self, user_id: &str) -> Result<Vec<KeycloakRole>>;
    async fn logout_user(&self, user_id: &str) -> Result<()>;
}

#[async_trait]
pub trait MasApi {
    async fn get_user_by_username(&self, username: &str) -> Result<Option<MasUser>>;
    async fn list_sessions(&self, mas_user_id: &str) -> Result<Vec<MasSession>>;
    async fn finish_session(&self, session_id: &str, session_type: &str) -> Result<()>;
}
```

`MasClient` authenticates via OAuth2 client credentials (`grant_type=client_credentials`, scope `urn:mas:admin`) and caches the token until 30 seconds before expiry.

Concrete implementations use `reqwest`. All upstream API uncertainty is isolated inside the relevant client module and documented with comments.

## Config (Environment Variables)

```
APP_BIND_ADDR (default: 127.0.0.1:3000)
APP_BASE_URL, APP_SESSION_SECRET, APP_REQUIRED_ADMIN_ROLE (default: matrix-admin)
HOMESERVER_DOMAIN
OIDC_ISSUER_URL, OIDC_CLIENT_ID, OIDC_CLIENT_SECRET, OIDC_REDIRECT_URL
KEYCLOAK_BASE_URL, KEYCLOAK_REALM, KEYCLOAK_ADMIN_CLIENT_ID, KEYCLOAK_ADMIN_CLIENT_SECRET
MAS_BASE_URL, MAS_ADMIN_CLIENT_ID, MAS_ADMIN_CLIENT_SECRET
DATABASE_URL (e.g. sqlite://data/app.db)
RUST_LOG
```

Config must fail fast on missing required values. See `.env.example` for full reference.

## Security Rules

- All mutating endpoints are POST-only with CSRF validation
- Admin role (`APP_REQUIRED_ADMIN_ROLE`) required on all protected routes
- Upstream tokens never exposed to the browser — all API calls are server-side
- Add request timeouts to all upstream `reqwest` calls
- Never log secrets or tokens

## Audit Logging

Every mutation (revoke MAS session, force Keycloak logout) must write an audit log entry with: `id`, `timestamp`, `admin_subject`, `admin_username`, `target_keycloak_user_id`, `target_matrix_user_id`, `action`, `result` (`success`/`failure`), `metadata_json`.

## What Not To Build

Do not add: reconciliation workers, room provisioning, role sync, bidirectional sync, SCIM, multi-realm support, user self-service, encryption key management.
