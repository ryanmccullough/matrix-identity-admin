# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## What This App Is — Vision

`matrix-identity-admin` (MIA) is the **identity and lifecycle control plane for self-hosted Matrix infrastructure.**

It fills the operational gap between Matrix infrastructure (Synapse, MAS), identity providers (Keycloak), and organizational policy. The long-term goal is to give administrators a single system to manage identity, access, and user lifecycle — the equivalent of Slack Admin or Google Workspace Admin for self-hosted Matrix.

**Current state (Phase 1):** A working admin console with OIDC login, user search, MAS session management, Keycloak admin actions, invite flow, and audit logging. The architecture is evolving — new features should be added incrementally toward the control-plane model without rewriting working code.

**Direction:** Read `vision.md` and `building_guide.md` for the full architectural direction. The summary:

- Evolving from a read-mostly console → identity lifecycle orchestrator
- Will add: reconciliation, group→room mapping, onboarding/offboarding workflows, lifecycle state model
- Will not add: full moderation suite, observability platform, federation governance, SCIM

---

## Development Standards

### Before every commit (required, no exceptions)

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

All three must pass locally before committing. CI enforces the same checks — a failing CI blocks merge.

### Branch naming

Format: `type/short-description` in kebab-case. Type must match the commit type.

```
feat/lifecycle-state-model
fix/mas-token-refresh
refactor/extract-disable-workflow
test/handler-coverage
ci/add-deny-check
chore/update-pr-template
```

### Commit format (Conventional Commits)

```
type(scope): short description        ← 50 chars max, imperative, no period
                                      ← blank line
Why this change was made.             ← body, 72 chars wrap, explain the why
```

Types: `feat`, `fix`, `refactor`, `perf`, `test`, `docs`, `ci`, `build`, `chore`
Scope: the affected module or area, lowercase — e.g. `feat(invite)`, `fix(mas)`

Use the `/commit` skill to create commits. It enforces this format.

### Coding style

**Comments:**
- `///` doc comments on all public types, traits, and non-trivial public functions
- `//` inline comments only where the code is non-obvious — never narrate what the code clearly shows
- `// NOTE:` for critical non-obvious behavior (e.g. ordering requirements, upstream quirks)
- `// TODO:` for known limitations that should be addressed later
- Section dividers `// ── Label ───────────────────────────────────────────────` only inside `#[cfg(test)]` blocks to group test cases; not in production code

**Naming:**
- Types: `PascalCase`; functions and variables: `snake_case`; constants: `SCREAMING_SNAKE_CASE`
- Concrete client implementations: `XxxClient` (e.g. `KeycloakClient`, `MasClient`)
- Test functions: `{what}_{condition}_{expected_result}` (e.g. `revoke_invalid_csrf_returns_400`)

**Error handling:**
- No `unwrap()` or `expect()` in production code paths — use `?` or explicit error handling
- `upstream_error()` helper for all `reqwest` errors — never construct `AppError::Upstream` directly
- Use `?` propagation where the error type converts cleanly; explicit `match` only when branching on the error variant

**Imports:**
- Grouped: `std` → external crates → `crate::` — blank line between groups
- `use super::*` only inside `#[cfg(test)]` test modules, nowhere else
- `rustfmt` enforces import formatting automatically

### Testing requirements

Every new handler must have tests covering:
1. Success path (correct redirect or response)
2. Unauthenticated request → redirects to `/auth/login`
3. Invalid CSRF → 400
4. Upstream failure → 502
5. Audit log written on success (where applicable)

Every new service/workflow function must have unit tests with mock implementations covering:
1. Happy path
2. Each upstream failure mode (graceful degradation or hard error, depending on contract)

Every new model type that implements `Display` or has non-trivial derived behavior must have basic tests.

New code must not reduce test coverage. The CI coverage report (Codecov) tracks this. If coverage drops, investigate before merging.

### CI gates — all must be green before merging

| Check | Tool | Blocks merge |
|-------|------|-------------|
| Formatting | `cargo fmt --check` | Yes |
| Compilation | `cargo check --all-targets` | Yes |
| Lint | `cargo clippy --all-targets -- -D warnings` | Yes |
| Tests | `cargo test` | Yes |
| Coverage | `cargo llvm-cov` + Codecov | Review on regression |
| Security | `cargo audit` | Yes (weekly + on push) |

Do not push to a branch with known CI failures. Do not merge a PR with failing checks.

---

## Commands

```bash
# All commands must run inside Flox (provides libiconv)
flox activate -- cargo build
flox activate -- cargo run       # requires .env or env vars
flox activate -- cargo test
flox activate -- cargo test <test_name>
flox activate -- cargo check
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo fmt
```

**Pre-commit checklist (required before every commit):**
```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

---

## Stack

- **Runtime**: `tokio` (async)
- **HTTP server**: `axum` with `tower`/`tower-http` middleware
- **Outbound HTTP**: `reqwest` (typed wrappers only — no large SDKs)
- **Templates**: `askama` (server-rendered HTML) + minimal HTMX/vanilla JS
- **Auth**: `openidconnect` crate for OIDC authorization code flow against Keycloak
- **Database**: `sqlx` with SQLite (audit logs only — identity state lives upstream)
- **Serialization**: `serde` / `serde_json`
- **Errors**: `thiserror` for typed errors, `anyhow` where appropriate
- **Logging**: `tracing` + `tracing-subscriber`

---

## Architecture — Four Layers

New work should be placed in the correct layer. Keep these seams clean.

### 1. Domain layer (`models/`)
Internal concepts that represent **organizational state**, not just upstream API responses.

Current: `unified.rs` (UnifiedUserSummary, UnifiedUserDetail, CorrelationStatus)
Direction: add `LifecycleState`, `GroupMapping`, canonical `User` model

### 2. Connector layer (`clients/`)
All communication with external systems lives here. Connectors own: base URLs, auth headers, typed request/response structs, retries, error conversion.

- `clients/keycloak.rs` — KeycloakApi trait + reqwest impl
- `clients/mas.rs` — MasApi trait + reqwest impl (OAuth2 client credentials, token cache)
- `clients/synapse.rs` — SynapseApi trait + reqwest impl — NOT compiled; preserved for future use

**Never leak raw upstream payloads into handlers or services.**

### 3. Workflow layer (`services/`)
Multi-step business logic that coordinates connectors and domain state.

Current: `user_service.rs`, `identity_mapper.rs`, `audit_service.rs`
Direction: extract explicit workflow modules — `invite_user`, `disable_user`, `offboard_user`, `reconcile_membership`

### 4. Interface layer (`handlers/`, `templates/`)
Thin HTTP handlers that call workflows and render templates. **No business logic here.**

```
handlers/
  auth.rs        # /auth/login, /auth/callback, /auth/logout
  dashboard.rs   # GET /
  users.rs       # GET /users/search, GET /users/{id}
  sessions.rs    # POST /users/{id}/sessions/{session_id}/revoke
  devices.rs     # POST /users/{id}/keycloak/logout
  delete.rs      # POST /users/{id}/delete
  invite.rs      # POST /api/v1/invites (bearer token auth)
  audit.rs       # GET /audit
```

---

## Full Module Layout

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

  clients/        # Connector layer
    mod.rs
    keycloak.rs
    mas.rs
    synapse.rs    # NOT compiled — preserved for future Matrix client API use

  services/       # Workflow layer
    mod.rs
    identity_mapper.rs
    user_service.rs
    audit_service.rs

  handlers/       # Interface layer
    mod.rs
    auth.rs / dashboard.rs / users.rs / sessions.rs
    devices.rs / delete.rs / invite.rs / audit.rs

  models/         # Domain layer
    mod.rs
    keycloak.rs   # KeycloakUser, KeycloakGroup, KeycloakRole
    mas.rs        # MasUser, MasSession
    synapse.rs    # NOT compiled — preserved for future use
    unified.rs    # UnifiedUserSummary, UnifiedUserDetail, UnifiedSession, CorrelationStatus
    audit.rs      # AuditLog struct

  db/
    mod.rs
    audit.rs      # sqlx queries for audit_logs table
    migrations/   # Initial migration: audit_logs table + indexes

templates/
  base.html / login.html / dashboard.html
  users_search.html / user_detail.html / audit.html

static/
  app.css
```

---

## Key Architectural Rules

- **Handlers are thin** — orchestrate calls to services/workflows, render templates, redirect. No business logic.
- **Connectors own all upstream API logic** — auth, requests, responses, errors. Never leak raw upstream payloads.
- **Services/workflows aggregate and coordinate** — combine data from multiple connectors into unified models.
- **`identity_mapper`** derives Keycloak → MAS → Matrix ID mapping. Mark uncertain or missing correlations explicitly — never silently assert.
- **Do not persist identity state locally** — SQLite is for audit logs only. Upstream systems are source of truth.
- **Refactor only where the next feature needs a cleaner boundary** — do not rewrite working code speculatively.

### Identity mapping model

```
Keycloak User (keycloak_user.id = stable subject)
   ↓
MAS account (correlated via OIDC subject)
   ↓
Matrix user (@{keycloak_user.username}:{homeserver_domain})
```

Correlation status:
- `Confirmed` — Keycloak user + MAS account found
- `Inferred` — Keycloak only; Matrix ID derived by convention (`@{username}:{homeserver_domain}`)

---

## Client Traits

Each upstream client is defined as an async trait for testability with mocks:

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

---

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

Config fails fast on missing required values. See `.env.example` for full reference.

---

## Security Rules

- All mutating endpoints are POST-only with CSRF validation
- Admin role (`APP_REQUIRED_ADMIN_ROLE`) required on all protected routes
- Upstream tokens never exposed to the browser — all API calls are server-side
- All upstream `reqwest` calls must have request timeouts
- Never log secrets or tokens

---

## Audit Logging

Every mutation must write an audit log entry with:
`id`, `timestamp`, `admin_subject`, `admin_username`, `target_keycloak_user_id`, `target_matrix_user_id`, `action`, `result` (`success`/`failure`), `metadata_json`.

---

## MSC3861 / Synapse Note

In MSC3861 mode, Synapse delegates auth entirely to MAS. MAS-issued compat tokens cannot access the Synapse admin API. Revoking a MAS compat session invalidates the corresponding Matrix device. Direct Synapse admin API calls are not used.

`src/clients/synapse.rs` and `src/models/synapse.rs` are preserved on disk but NOT compiled — reserved for future Matrix client API use (e.g. room joins for invite flows).

---

## Roadmap Phases

| Phase | Focus |
|-------|-------|
| 1 — Trustworthy | Reliable invite flow, unified disable/offboard, audit logging, clear connectors, basic lifecycle state ✅ mostly done |
| 2 — Structurally sound | Extract explicit workflows, group membership reconciliation, dry-run support, better multi-step error handling |
| 3 — Extensible | Provider interfaces, policy config, swappable backends, more deployment patterns |
| 4 — Polished | Better admin UI, bulk actions, dashboards, onboarding templates |

See `building_guide.md` for detailed guidance on when to build vs refactor.
