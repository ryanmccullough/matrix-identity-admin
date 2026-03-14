# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

## What This App Is — Vision

`matrix-identity-admin` (MIA) is the **identity and lifecycle control plane for self-hosted Matrix infrastructure.**

It fills the operational gap between Matrix infrastructure (Synapse, MAS), identity providers (Keycloak), and organizational policy. The long-term goal is to give administrators a single system to manage identity, access, and user lifecycle — the equivalent of Slack Admin or Google Workspace Admin for self-hosted Matrix.

**Current state (Phase 4):** Phases 1–3 complete. Working admin console with OIDC login, user search, MAS session management, Keycloak admin actions, invite flow, audit logging, lifecycle workflows (disable/offboard/reactivate/delete), group→room reconciliation with DB-backed policy engine, Pico CSS UI, dashboard stats, and tag-triggered release pipeline. Phase 4 (polish) is in progress.

**Direction:** Read `vision.md` and `building_guide.md` for the full architectural direction. The summary:

- Evolving from identity lifecycle orchestrator → polished admin console
- Will add: bulk actions, onboarding templates
- Will not add: full moderation suite, observability platform, federation governance, SCIM

---

## Development Standards

### PR workflow (required for all non-trivial changes)

All features, fixes, refactors, and CI changes go through a branch + PR. Direct pushes to `main` are only acceptable for documentation-only or config-only changes with no code impact.

```
1. Create a branch:  git checkout -b type/short-description
2. Make changes
3. Run pre-commit gate (see below)
4. Commit using /commit skill
5. Push:             git push -u origin type/short-description
6. Open PR on GitHub — CI must be green before merging
7. Review your own diff, then merge
```

E2E tests run on every PR — review the results before merging.

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

**PR workflow:**
```bash
git checkout -b type/short-description   # never work directly on main
# make changes, run pre-commit gate, /commit
git push -u origin type/short-description
gh pr create                             # open PR; CI + e2e must be green before merge
```

---

## Stack

- **Runtime**: `tokio` (async)
- **HTTP server**: `axum` with `tower`/`tower-http` middleware
- **Outbound HTTP**: `reqwest` (typed wrappers only — no large SDKs)
- **Templates**: `askama` (server-rendered HTML) + minimal HTMX/vanilla JS
- **Auth**: `openidconnect` crate for OIDC authorization code flow against Keycloak
- **Database**: `sqlx` with SQLite (audit logs + policy bindings)
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

- `clients/keycloak.rs` — KeycloakIdentityProvider trait + KeycloakClient reqwest impl
- `clients/mas.rs` — AuthService trait + MasClient reqwest impl (OAuth2 client credentials, token cache)
- `clients/synapse.rs` — MatrixService trait + SynapseClient reqwest impl
- `clients/identity_provider.rs` — IdentityProvider generic trait (returns CanonicalUser)

**Never leak raw upstream payloads into handlers or services.**

### 3. Workflow layer (`services/`)
Multi-step business logic that coordinates connectors and domain state.

Current: `user_service.rs`, `identity_mapper.rs`, `audit_service.rs`, `policy_service.rs`, `lifecycle_steps.rs`, `invite_user.rs`, `disable_user.rs`, `reactivate_user.rs`, `offboard_user.rs`, `delete_user.rs`, `reconcile_membership.rs`

### 4. Interface layer (`handlers/`, `templates/`)
Thin HTTP handlers that call workflows and render templates. **No business logic here.**

```
handlers/
  auth.rs        # /auth/login, /auth/callback, /auth/logout
  dashboard.rs   # GET /, GET /status
  users.rs       # GET /users/search, GET /users/{id}
  sessions.rs    # POST /users/{id}/sessions/{session_id}/revoke
  devices.rs     # POST /users/{id}/keycloak/logout
  disable.rs     # POST /users/{id}/disable
  reactivate.rs  # POST /users/{id}/reactivate
  offboard.rs    # POST /users/{id}/offboard
  delete.rs      # POST /users/{id}/delete
  reconcile.rs   # POST /users/{id}/reconcile, POST /users/{id}/reconcile/preview
  bulk_reconcile.rs # POST /users/reconcile/all
  invite.rs      # POST /api/v1/invites (bearer token), POST /users/invite (admin UI)
  audit.rs       # GET /audit
  policy.rs      # GET /policy, POST /policy (CRUD for policy bindings)
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
    keycloak.rs           # KeycloakIdentityProvider trait + KeycloakClient
    mas.rs                # AuthService trait + MasClient
    synapse.rs            # MatrixService trait + SynapseClient
    identity_provider.rs  # IdentityProvider generic trait

  services/       # Workflow layer
    mod.rs
    identity_mapper.rs
    user_service.rs
    audit_service.rs
    policy_service.rs     # CRUD + effective binding resolution + bootstrap from legacy config
    lifecycle_steps.rs    # Shared composable primitives for lifecycle workflows
    disable_user.rs
    reactivate_user.rs
    offboard_user.rs
    delete_user.rs
    invite_user.rs
    reconcile_membership.rs

  handlers/       # Interface layer
    mod.rs
    auth.rs / dashboard.rs / users.rs / sessions.rs
    devices.rs / disable.rs / reactivate.rs / offboard.rs / delete.rs
    reconcile.rs / bulk_reconcile.rs / invite.rs / audit.rs / policy.rs

  models/         # Domain layer
    mod.rs
    keycloak.rs       # KeycloakUser, KeycloakGroup, KeycloakRole
    mas.rs            # MasUser, MasSession
    synapse.rs        # SynapseUser, SynapseDevice
    unified.rs        # UnifiedUserSummary, UnifiedUserDetail, CanonicalUser, LifecycleState
    group_mapping.rs  # GroupMapping
    policy_binding.rs # PolicyBinding (DB-backed policy bindings)
    workflow.rs       # WorkflowOutcome
    audit.rs          # AuditLog struct

  db/
    mod.rs
    audit.rs      # sqlx queries for audit_logs table
    policy.rs     # sqlx queries for policy_bindings, policy_targets_cache, policy_bootstrap_state
    migrations/   # audit_logs + policy tables + indexes

templates/
  base.html / login.html / dashboard.html
  users_search.html / user_detail.html / audit.html / policy.html

static/
  app.css
```

---

## Key Architectural Rules

- **Handlers are thin** — orchestrate calls to services/workflows, render templates, redirect. No business logic.
- **Connectors own all upstream API logic** — auth, requests, responses, errors. Never leak raw upstream payloads.
- **Services/workflows aggregate and coordinate** — combine data from multiple connectors into unified models.
- **`identity_mapper`** derives Keycloak → MAS → Matrix ID mapping. Mark uncertain or missing correlations explicitly — never silently assert.
- **Do not persist identity state locally** — SQLite is for audit logs and policy bindings only. Upstream systems are source of truth for user identity.
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

Two layers of traits: **provider-specific** (return upstream types) and **provider-agnostic** (return domain types).

Provider-specific traits — used by lifecycle workflows:
```rust
#[async_trait]
pub trait KeycloakIdentityProvider: Send + Sync {
    async fn get_user(&self, user_id: &str) -> Result<KeycloakUser>;
    async fn disable_user(&self, user_id: &str) -> Result<()>;
    async fn enable_user(&self, user_id: &str) -> Result<()>;
    async fn logout_user(&self, user_id: &str) -> Result<()>;
    // + search_users, get_user_groups, get_user_roles, create_user, delete_user, ...
}

#[async_trait]
pub trait AuthService: Send + Sync {
    async fn get_user_by_username(&self, username: &str) -> Result<Option<MasUser>>;
    async fn list_sessions(&self, mas_user_id: &str) -> Result<Vec<MasSession>>;
    async fn finish_session(&self, session_id: &str, session_type: &str) -> Result<()>;
    async fn delete_user(&self, mas_user_id: &str) -> Result<()>;
    async fn reactivate_user(&self, mas_user_id: &str) -> Result<()>;
}

#[async_trait]
pub trait MatrixService: Send + Sync {
    async fn get_joined_room_members(&self, room_id: &str) -> Result<Vec<String>>;
    async fn force_join_user(&self, user_id: &str, room_id: &str) -> Result<()>;
    async fn kick_user_from_room(&self, user_id: &str, room_id: &str, reason: &str) -> Result<()>;
    async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>>;
    // + get_user, list_devices, delete_device
}
```

Provider-agnostic trait — used by `UserService` for pluggable backends (Phase 3):
```rust
pub trait IdentityProvider: Send + Sync {  // returns CanonicalUser, not KeycloakUser
    async fn get_user(&self, id: &str) -> Result<CanonicalUser>;
    async fn search_users(&self, query: &str, max: u32, first: u32) -> Result<Vec<CanonicalUser>>;
    // + get_user_groups, get_user_roles, logout_user, count_users
}
```

`KeycloakClient` implements both `KeycloakIdentityProvider` and `IdentityProvider`.

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

## Synapse / MAS Auth Note

Synapse delegates auth entirely to MAS via `matrix_authentication_service` config. Key facts:

- **No static `admin_token` in homeserver.yaml.** All tokens are validated by MAS via introspection using a shared secret (`matrix.secret` in MAS / `secret` in Synapse).
- **Admin access requires `urn:synapse:admin:*` scope** on the token, not the `admin` column in Synapse's `users` table.
- **Regular `mct_` compat tokens from `m.login.password`** do not get admin scope — by design.
- **Admin tokens must be provisioned** via `mas-cli manage issue-compatibility-token <user> --yes-i-want-to-grant-synapse-admin-privileges`.
- **Synapse disables `/_matrix/client/v3/login`** — the MAS compat layer handles `m.login.password` instead (requires `compat` resource in MAS HTTP listener config).
- **Users are not auto-provisioned in Synapse** until their first OIDC login through MAS. Use `PUT /_synapse/admin/v2/users/{user_id}` with the admin token to provision programmatically.
- Revoking a MAS compat session invalidates the corresponding Matrix device.

`SynapseClient` requires `SYNAPSE_ADMIN_TOKEN` — a `mas-cli`-provisioned compat token with admin scope. The old `m.login.password` fallback has been removed.

Admin API endpoints are used for operations that have no client API equivalent (e.g. force-joining a user to a room, listing room members). Client API endpoints are used where they suffice (e.g. kicking a user from a room).

---

## Roadmap Phases

| Phase | Focus |
|-------|-------|
| 1 — Trustworthy | Reliable invite flow, unified disable/offboard, audit logging, clear connectors, basic lifecycle state ✅ done |
| 2 — Structurally sound | Extract explicit workflows, group membership reconciliation, dry-run support, better multi-step error handling ✅ done |
| 3 — Extensible | Provider interfaces, DB-backed dynamic policy engine, swappable backends, more deployment patterns ✅ done |
| 4 — Polished | Better admin UI, bulk actions, dashboards, onboarding templates — **in progress** (Pico CSS, dashboard stats, release pipeline done) |

See `building_guide.md` for detailed guidance on when to build vs refactor.

---

## Feature Plan: Group Membership Reconciliation

> Phase 2 item — reconciliation and preview shipped 2026-03-09. Phase 3 replaced static config with DB-backed dynamic policy engine.

### What it does

Compares a user's Keycloak group membership against Matrix room memberships, then:
- **Joins** the user to rooms they should be in (based on group → room policy)
- **Optionally kicks** the user from rooms they shouldn't be in (per-binding `allow_remove` flag)

Triggered manually per-user from the user detail page. No background worker initially.

### Policy model

Policy bindings are stored in SQLite (`policy_bindings` table) and managed via the `/policy` admin UI. Each binding maps a Keycloak group to a Matrix room with per-binding options (`allow_remove`, power level).

The `GROUP_MAPPINGS` env var (JSON array) is **bootstrap-only** — imported into SQLite on first run. After bootstrap, the database is the source of truth. The old `RECONCILE_REMOVE_FROM_ROOMS` env var has been removed; use the per-binding `allow_remove` flag instead.

`PolicyService` (`src/services/policy_service.rs`) provides CRUD operations, effective binding resolution, room cache refresh, and bootstrap from legacy config. It replaces the old `PolicyEngine` struct.

New database tables: `policy_bindings`, `policy_targets_cache`, `policy_bootstrap_state`.

### Config vars

```
SYNAPSE_BASE_URL            # e.g. https://matrix.example.com
SYNAPSE_ADMIN_TOKEN         # mas-cli compat token with urn:synapse:admin:* scope
GROUP_MAPPINGS              # Bootstrap-only: JSON array of {keycloak_group, matrix_room_id}
                            # Imported into SQLite on first run; DB is source of truth after that
# RECONCILE_REMOVE_FROM_ROOMS — REMOVED: replaced by per-binding allow_remove flag in DB
```

### Synapse connector extensions

`src/clients/synapse.rs` uses a `mas-cli`-provisioned admin token and calls `/_synapse/admin/v2/` endpoints.

Trait methods on `MatrixService`:

```rust
async fn get_joined_room_members(&self, room_id: &str) -> Result<Vec<String>>; // returns Matrix IDs
async fn force_join_user(&self, user_id: &str, room_id: &str) -> Result<()>;
async fn kick_user_from_room(&self, user_id: &str, room_id: &str, reason: &str) -> Result<()>;
async fn list_rooms(&self) -> Result<Vec<RoomSummary>>;
async fn get_room_details(&self, room_id: &str) -> Result<RoomDetails>;
async fn set_power_level(&self, room_id: &str, user_id: &str, level: i64) -> Result<()>;
```

`KeycloakIdentityProvider` also gained:
```rust
async fn list_groups(&self) -> Result<Vec<KeycloakGroup>>;
async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>>;
```

Endpoints:
- `GET /_synapse/admin/v1/rooms/{room_id}/members` — list all room members (admin API)
- `POST /_synapse/admin/v1/join/{room_id}` — force-join a user (body: `{"user_id": "@user:domain"}`)
- `POST /_matrix/client/v3/rooms/{room_id}/kick` — kick user from room (client API)

**Why force-join, not invite:** The vision requires *enforcement* of room membership from group policy — "members automatically join". An invite requires user acceptance and cannot enforce policy. Force-join via the admin API is the correct semantic. This is consistent with the existing `SynapseClient` pattern (password login → admin API calls).

Wire `SynapseClient` into `AppState` once config vars are present. Make it optional (`Option<Arc<dyn MatrixService>>`) so the app still boots without Synapse config — reconcile button is hidden when `None`.

### Reconciliation workflow

`src/services/reconcile_membership.rs`

Reconciliation now uses `PolicyBinding` records from SQLite (via `PolicyService`) instead of static config. Each binding carries its own `allow_remove` flag and optional power level, replacing the global `RECONCILE_REMOVE_FROM_ROOMS` env var.

Logic per binding:
1. Check if user is already in the room (`get_joined_room_members`)
2. If user is in the group but not the room → `force_join_user` → audit `join_room_on_reconcile`
3. If the binding's `allow_remove` is true and user is in the room but not the group → `kick_user_from_room` → audit `kick_room_on_reconcile`
4. Per-room failures are non-fatal → `outcome.add_warning(...)`, continue to next room

Returns `WorkflowOutcome` (warnings for any per-room failures).

### Handler and UI

`src/handlers/reconcile.rs` — `POST /users/{id}/reconcile`
- CSRF validated
- Fetches Keycloak groups for user (already available via `keycloak.get_user_groups`)
- Derives `matrix_user_id` via `identity_mapper`
- Calls `reconcile_membership` workflow
- Redirects to `/users/{id}?notice=Membership+reconciled` or `?warning=...` if partial failures

User detail page (`templates/user_detail.html`) — new card or button in the Identity card:
```html
<form method="post" action="/users/{{ user.keycloak_id }}/reconcile" ...>
  <input type="hidden" name="_csrf" value="{{ csrf_token }}">
  <button type="submit" class="btn btn-primary">Reconcile Room Membership</button>
</form>
```
Only rendered when Synapse is configured (pass `synapse_enabled: bool` to template).

### Implementation order

1. `models/group_mapping.rs` — `GroupMapping` struct, parse from JSON
2. `config.rs` — add `group_mappings: Vec<GroupMapping>`, `synapse_*` fields, `reconcile_remove_from_rooms: bool`
3. `clients/synapse.rs` — compile it in; add `get_joined_room_members`, `force_join_user`, `kick_user_from_room` to trait + impl
4. `state.rs` — add `synapse: Option<Arc<dyn MatrixService>>`
5. `services/reconcile_membership.rs` — workflow (unit-testable with mock `MatrixService`)
6. `handlers/reconcile.rs` — thin handler
7. `lib.rs` — wire route `POST /users/{id}/reconcile`
8. `templates/user_detail.html` — Reconcile button (conditional)
9. Tests — mock `MatrixService`; cover force-join, skip-already-member, kick (when enabled), per-room failure → warning

### Resolved decisions

| Decision | Resolution | Notes |
|----------|-----------|-------|
| Kicks opt-in or opt-out? | Per-binding `allow_remove` flag (default false) | Replaced global `RECONCILE_REMOVE_FROM_ROOMS` env var |
| Config format for mappings | DB-backed via `/policy` admin UI | `GROUP_MAPPINGS` env var is bootstrap-only (imported on first run) |
| Preview/dry-run mode | Shipped in Phase 2 | HTMX inline panel via `preview_membership` + `POST /users/{id}/reconcile/preview` |
| Synapse required at startup? | No — optional | App boots without Synapse config; reconcile is hidden if not configured |
| Admin user in mapped rooms for kicks? | Yes | `kick` uses client API; the admin user must be a member of each mapped room. `get_joined_room_members` and `force_join_user` use admin API and have no room-membership requirement. |
