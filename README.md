# Matrix Identity Admin

[![CI](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/ci.yml/badge.svg)](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/ci.yml)
[![E2E](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/e2e.yml/badge.svg)](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/e2e.yml)
[![Security](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/security.yml/badge.svg)](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/security.yml)
[![codecov](https://codecov.io/gh/ryanmccullough/matrix-identity-admin/graph/badge.svg)](https://codecov.io/gh/ryanmccullough/matrix-identity-admin)
[![deps.rs](https://deps.rs/repo/github/ryanmccullough/matrix-identity-admin/status.svg)](https://deps.rs/repo/github/ryanmccullough/matrix-identity-admin)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

**The identity and lifecycle control plane for self-hosted Matrix.**

Matrix Identity Admin (MIA) bridges the gap between your identity provider and your Matrix infrastructure. It gives administrators a single place to manage users, sessions, access, and the full account lifecycle — across [Keycloak](https://www.keycloak.org/), [MAS](https://matrix-authentication-service.pages.dev/), and [Synapse](https://github.com/element-hq/synapse) — without juggling multiple admin consoles.

Built for [MSC3861](https://github.com/matrix-org/matrix-spec-proposals/pull/3861) deployments where Synapse delegates authentication entirely to MAS.

---

## Table of Contents

- [Features](#features)
- [Architecture](#architecture)
- [Quick Start](#quick-start)
- [Configuration](#configuration)
- [Keycloak Setup](#keycloak-setup)
- [MAS Setup](#mas-setup)
- [Group Membership Reconciliation](#group-membership-reconciliation)
- [Bot Invite Flow](#bot-invite-flow)
- [Element Web — SSO](#element-web--sso)
- [Development](#development)
- [Security](#security)
- [Contributing](#contributing)

---

## Features

- **User search and detail** — unified view across Keycloak and MAS with lifecycle state, correlation status, groups, roles, and active sessions
- **Session management** — revoke individual MAS sessions (compat and OAuth2); force-logout all Keycloak sessions
- **Lifecycle actions** — disable accounts (revokes sessions + disables Keycloak), delete users from both Keycloak and MAS
- **Group → room reconciliation** — enforce Matrix room membership based on Keycloak group policy; force-join users into mapped rooms, optionally kick users who have left the group
- **Bot-driven invites** — REST API for maubot to create Keycloak accounts and send invite emails; users pick their own Matrix username on first login
- **Audit log** — every mutation is recorded with admin identity, target user, action, result, and metadata
- **OIDC login** — admins authenticate via Keycloak; role-based access control enforced on every route
- **CSRF protection** — all mutating endpoints are POST-only with per-session CSRF tokens

---

## Architecture

```
            Keycloak
               │
               │ identity
               ▼
     matrix-identity-admin
               │
               │ lifecycle orchestration
               ▼
    ┌──────────┼──────────┐
    │          │          │
   MAS       Synapse     email
    │          │
    ▼          ▼
 Maubot    Synapse Admin
    │
    ▼
 Hookshot events
```

MIA sits between your identity provider and the Matrix ecosystem. It does not replace Synapse Admin or Maubot — it orchestrates them.

**Tech stack:** Rust · Axum · Tokio · Askama templates · SQLx + SQLite · `openidconnect` crate · Reqwest

---

## Quick Start

### Prerequisites

- Rust toolchain (`cargo`, `rustc`)
- A running Keycloak instance with a configured realm
- A running MAS instance with an admin OAuth2 client
- [Flox](https://flox.dev) (macOS only — provides `libiconv` at build time)

### 1. Configure environment

```bash
cp .env.example .env
# Fill in your values — see Configuration below
```

### 2. Run

```bash
# macOS (inside Flox)
flox activate -- cargo run

# Linux
cargo run
```

The app starts at `http://127.0.0.1:3000` by default. Navigate there — you will be redirected to Keycloak to log in.

### E2E environment (Docker)

A local Docker Compose stack (Postgres + Keycloak + MAS) is in `e2e/`:

```bash
cd e2e
cp .env.example .env   # already pre-filled for local use
docker compose up
```

See `e2e/README.md` for test credentials and setup notes.

---

## Configuration

All configuration is via environment variables. The app exits immediately on startup if any required variable is missing.

### Core

| Variable | Required | Default | Description |
|---|---|---|---|
| `APP_BIND_ADDR` | No | `127.0.0.1:3000` | Listen address — use `0.0.0.0:3000` to expose on the network |
| `APP_BASE_URL` | Yes | — | Public base URL (e.g. `https://admin.example.com`) |
| `APP_SESSION_SECRET` | Yes | — | Secret for cookie signing — generate with `openssl rand -hex 32` |
| `APP_REQUIRED_ADMIN_ROLE` | No | `matrix-admin` | Keycloak realm role required to access the app |
| `HOMESERVER_DOMAIN` | Yes | — | Matrix homeserver domain (e.g. `example.com`) |
| `DATABASE_URL` | Yes | — | SQLite path (e.g. `sqlite://data/app.db`) |
| `RUST_LOG` | No | `info` | Log level |

### OIDC (admin login via Keycloak)

| Variable | Required | Description |
|---|---|---|
| `OIDC_ISSUER_URL` | Yes | Keycloak realm URL (e.g. `https://keycloak.example.com/realms/myrealm`) |
| `OIDC_CLIENT_ID` | Yes | OIDC client ID |
| `OIDC_CLIENT_SECRET` | Yes | OIDC client secret |
| `OIDC_REDIRECT_URL` | Yes | Callback URL (e.g. `https://admin.example.com/auth/callback`) |

### Keycloak admin API

| Variable | Required | Description |
|---|---|---|
| `KEYCLOAK_BASE_URL` | Yes | Keycloak base URL |
| `KEYCLOAK_REALM` | Yes | Realm name (**case-sensitive**) |
| `KEYCLOAK_ADMIN_CLIENT_ID` | Yes | Service account client ID |
| `KEYCLOAK_ADMIN_CLIENT_SECRET` | Yes | Service account client secret |

### MAS admin API

| Variable | Required | Description |
|---|---|---|
| `MAS_BASE_URL` | Yes | MAS base URL (e.g. `https://matrix.example.com/auth`) |
| `MAS_ADMIN_CLIENT_ID` | Yes | MAS OAuth2 admin client ID |
| `MAS_ADMIN_CLIENT_SECRET` | Yes | MAS OAuth2 admin client secret |

### Invite flow

| Variable | Required | Description |
|---|---|---|
| `BOT_API_SECRET` | Yes | Bearer secret for `POST /api/v1/invites` — generate with `openssl rand -hex 32` |
| `INVITE_ALLOWED_DOMAINS` | No | Comma-separated allowed email domains (unset = any domain) |

### Group membership reconciliation (optional)

All three Synapse variables must be set together. If any is absent, the Reconcile button is hidden and the feature is disabled.

| Variable | Required | Description |
|---|---|---|
| `SYNAPSE_BASE_URL` | No | Synapse base URL (e.g. `https://matrix.example.com`) |
| `SYNAPSE_ADMIN_USER` | No | Matrix ID of the admin user (e.g. `@admin:example.com`) |
| `SYNAPSE_ADMIN_PASSWORD` | No | Admin user password — used for `m.login.password` token |
| `GROUP_MAPPINGS` | No | JSON array mapping Keycloak groups to Matrix rooms (see below) |
| `GROUP_MAPPINGS_FILE` | No | Path to a JSON file containing the mappings array (takes precedence over `GROUP_MAPPINGS` if set) |
| `RECONCILE_REMOVE_FROM_ROOMS` | No | `true` to kick users from rooms when removed from the group (default: `false`) |

See [Group Membership Reconciliation](#group-membership-reconciliation) for details.

---

## Keycloak Setup

> `KEYCLOAK_REALM` is case-sensitive — it must exactly match the realm name in the Keycloak console.

### 1. OIDC client (admin app login)

Create a client with ID matching `OIDC_CLIENT_ID`:

- **Client authentication:** ON
- **Valid redirect URIs:** `{APP_BASE_URL}/auth/callback`
- **Client scopes → roles → Mappers:** find the `realm roles` mapper and set **"Add to ID token" = ON**

This is required for the app to read realm roles from the ID token and enforce `APP_REQUIRED_ADMIN_ROLE`.

### 2. Service account client (Keycloak admin API)

Create a client with ID matching `KEYCLOAK_ADMIN_CLIENT_ID`:

- **Client authentication:** ON
- **Service accounts enabled:** ON
- **Service account roles** (Clients → your client → Service accounts roles tab):
  - From `realm-management`: assign **`view-users`** and **`manage-users`**

### 3. Admin role

Create a realm role named `matrix-admin` (or your `APP_REQUIRED_ADMIN_ROLE`) and assign it to your admin users.

### 4. Login settings

In **Realm settings → Login tab**, enable **Edit username**. This lets invited users choose their Matrix username during onboarding, before MAS provisions their account on first login.

### 5. Email (required for invite flow)

Configure SMTP in **Realm settings → Email tab**. The invite flow triggers Keycloak's native `execute-actions-email` — if SMTP is not configured, invites will fail.

---

## MAS Setup

Create an OAuth2 client in MAS with:

- Grant type: `client_credentials`
- Scope: `urn:mas:admin`

Use the resulting client ID and secret as `MAS_ADMIN_CLIENT_ID` / `MAS_ADMIN_CLIENT_SECRET`.

---

## Group Membership Reconciliation

MIA can enforce Matrix room membership based on Keycloak group membership. When an admin clicks **Reconcile Room Membership** on a user detail page:

1. MIA fetches the user's current Keycloak groups
2. For each configured group → room mapping, it checks whether the user is in the room
3. If the user is in the group but not the room → force-joins them
4. If `RECONCILE_REMOVE_FROM_ROOMS=true` and the user is in the room but no longer in the group → kicks them

Partial failures (e.g. one room unreachable) produce a warning flash but do not abort the reconciliation. All actions are audit-logged.

### Configuring mappings

**Option A — inline JSON** (suitable for small deployments):

```bash
GROUP_MAPPINGS='[
  {"keycloak_group": "staff",      "matrix_room_id": "!abc123:example.com"},
  {"keycloak_group": "engineers",  "matrix_room_id": "!xyz789:example.com"},
  {"keycloak_group": "engineers",  "matrix_room_id": "!eng-private:example.com"}
]'
```

**Option B — JSON file** (recommended for larger deployments):

```bash
GROUP_MAPPINGS_FILE=/etc/mia/group_mappings.json
```

The file must contain the same JSON array format. `GROUP_MAPPINGS_FILE` takes precedence over `GROUP_MAPPINGS` when both are set. The app exits on startup if the file cannot be read or contains invalid JSON.

One group can map to multiple rooms. The reconcile button only appears in the UI when all three `SYNAPSE_*` variables are configured.

> **Note on kicks:** The Synapse admin user must be a member of any room you want to kick from (kicks use the client API). Force-joins do not require room membership.

---

## Bot Invite Flow

The app exposes `POST /api/v1/invites` for bot-driven user invitations. A maubot plugin (`maubot-invite/`) provides the `!invite <email>` Matrix command.

**How it works:**

1. Bot sends `POST /api/v1/invites` with `Authorization: Bearer <BOT_API_SECRET>` and `{"email": "user@example.com"}`
2. MIA checks for an existing Keycloak account with that email
3. Creates a Keycloak user with required actions: `UPDATE_PASSWORD`, `UPDATE_PROFILE`, `VERIFY_EMAIL`
4. Triggers Keycloak's native invite email
5. User clicks the link, sets their password, and picks their Matrix username
6. On first login via Element Web, MAS auto-provisions the account with their chosen username

**Build and deploy the maubot plugin:**

```bash
cd maubot-invite
./build.sh          # produces invite-bot.mbp
# Upload invite-bot.mbp via the maubot admin UI
```

Plugin config:

| Key | Description |
|-----|-------------|
| `admin_url` | Base URL of MIA reachable from the maubot host |
| `bot_api_secret` | Must match `BOT_API_SECRET` in MIA's `.env` |
| `ops_room_id` | Room ID where `!invite` is accepted (empty = any room) |

---

## Element Web — SSO

To skip the password login screen and redirect users straight to Keycloak, add to your Element Web `config.json`:

```json
"sso_redirect_options": {
  "immediate": true
}
```

Without this, users see a Matrix password field by default and may attempt to log in with credentials instead of SSO.

---

## Development

```bash
# Check
cargo check

# Test
cargo test

# Lint
cargo clippy --all-targets -- -D warnings

# Format
cargo fmt

# Run (macOS — inside Flox)
flox activate -- cargo run

# Run (Linux)
cargo run
```

### Project layout

```
src/
  main.rs          # bootstrap and routes
  config.rs        # env config — fails fast on missing vars
  state.rs         # shared app state
  error.rs         # AppError and HTTP status mapping
  auth/            # OIDC flow, session cookies, CSRF
  handlers/        # thin route handlers — delegate to services
  services/        # lifecycle workflows (invite, disable, delete, reconcile)
  clients/         # Keycloak, MAS, and Synapse API wrappers
  models/          # domain types + upstream-specific structs
  db/              # audit log persistence (SQLx + SQLite)
migrations/        # SQLx migrations
templates/         # Askama HTML templates
static/            # CSS
maubot-invite/     # maubot plugin for the !invite command
e2e/               # Docker Compose stack for local end-to-end testing
```

See [CLAUDE.md](CLAUDE.md) for architecture decisions, layer rules, and contribution standards.

---

## Security

- All mutating endpoints are POST-only with per-session CSRF tokens
- Admin access requires a Keycloak realm role on every route
- Upstream API tokens are server-side only — never sent to the browser
- All upstream calls use explicit timeouts
- Secrets are read from environment variables only — never hardcoded
- Every mutation writes an audit log entry (including failures)
- CI runs `cargo audit` and gitleaks secret scanning on every PR; CodeQL (GitHub default setup) and OWASP ZAP run on schedule

---

## Contributing

Contributions are welcome. Please read [CLAUDE.md](CLAUDE.md) for architecture guardrails and [AGENTS.md](AGENTS.md) for the roadmap and decision rules.

**Before submitting a PR:**

```bash
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test
```

All three must pass. CI enforces the same checks and blocks merge on failure.

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/): `type(scope): description`.
