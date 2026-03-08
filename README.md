# Matrix Identity Admin

[![CI](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/ci.yml/badge.svg)](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/ci.yml)
[![Security audit](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/security.yml/badge.svg)](https://github.com/ryanmccullough/matrix-identity-admin/actions/workflows/security.yml)
[![codecov](https://codecov.io/gh/ryanmccullough/matrix-identity-admin/graph/badge.svg)](https://codecov.io/gh/ryanmccullough/matrix-identity-admin)
[![deps.rs](https://deps.rs/repo/github/ryanmccullough/matrix-identity-admin/status.svg)](https://deps.rs/repo/github/ryanmccullough/matrix-identity-admin)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE)

Thin internal admin console for a self-hosted Matrix stack running [MSC3861](https://github.com/matrix-org/matrix-spec-proposals/pull/3861) (Synapse delegates auth to MAS).

It provides a unified admin view over:
- **Keycloak** — identity source of truth
- **MAS (Matrix Authentication Service)** — auth and session layer

Supported admin actions:
- Revoke MAS sessions (compat and OAuth2)
- Force Keycloak logout
- Delete users (from Keycloak and MAS atomically)
- Invite users via a bot-driven API (maubot plugin included)

All mutations are audit-logged to SQLite.

> **Note on Synapse devices:** In MSC3861 mode, compat tokens cannot access the Synapse admin API. MAS sessions are the source of truth — revoking a MAS compat session invalidates the corresponding Matrix device. Direct Synapse admin API integration is not used.

## Tech Stack

- Rust 2021
- Axum + Tokio
- Askama templates (server-rendered HTML)
- Reqwest clients for upstream APIs
- SQLx + SQLite (audit logs only)
- OIDC auth via `openidconnect` crate

## Project Layout

```text
src/
  main.rs                # app bootstrap + routes
  config.rs              # env config (fails fast on missing vars)
  state.rs               # shared state
  error.rs               # AppError and HTTP mapping
  auth/                  # OIDC flow, session cookies, CSRF
  handlers/              # route handlers (thin — delegate to services)
  services/              # orchestration/business logic
  clients/               # Keycloak and MAS API wrappers
  models/                # typed models (upstream + unified)
  db/                    # audit log persistence (SQLx)
migrations/              # SQLx migrations
templates/               # Askama HTML templates
static/                  # CSS
maubot-invite/           # maubot plugin for the !invite command
```

## Quick Start

### Prerequisites

- Rust toolchain (`cargo`, `rustc`)
- Access to Keycloak and MAS admin endpoints
- [Flox](https://flox.dev) (provides `libiconv` on macOS — required at build time)

### Configure environment

```bash
cp .env.example .env
# Edit .env with your values
```

### Run locally

```bash
flox activate -- cargo run
```

Default bind: `127.0.0.1:3000`

### Docker

```bash
docker compose up --build
```

## Environment Variables

| Variable | Required | Description |
|---|---|---|
| `APP_BIND_ADDR` | No (default: `127.0.0.1:3000`) | Listen address. Use `0.0.0.0:3000` to listen on all interfaces (localhost + LAN). |
| `APP_BASE_URL` | Yes | Public base URL (e.g. `http://localhost:3000`) |
| `APP_SESSION_SECRET` | Yes | Secret for cookie signing — use `openssl rand -hex 32` |
| `APP_REQUIRED_ADMIN_ROLE` | No (default: `matrix-admin`) | Keycloak realm role required to access the app |
| `HOMESERVER_DOMAIN` | Yes | Matrix homeserver domain (e.g. `example.com`) |
| `OIDC_ISSUER_URL` | Yes | Keycloak realm URL (e.g. `https://keycloak.example.com/realms/myrealm`) |
| `OIDC_CLIENT_ID` | Yes | OIDC client ID |
| `OIDC_CLIENT_SECRET` | Yes | OIDC client secret |
| `OIDC_REDIRECT_URL` | Yes | Callback URL (e.g. `http://localhost:3000/auth/callback`) |
| `KEYCLOAK_BASE_URL` | Yes | Keycloak base URL |
| `KEYCLOAK_REALM` | Yes | Keycloak realm name (**case-sensitive**) |
| `KEYCLOAK_ADMIN_CLIENT_ID` | Yes | Service account client ID |
| `KEYCLOAK_ADMIN_CLIENT_SECRET` | Yes | Service account client secret |
| `MAS_BASE_URL` | Yes | MAS base URL (e.g. `https://matrix.example.com/auth`) |
| `MAS_ADMIN_CLIENT_ID` | Yes | MAS OAuth2 admin client ID |
| `MAS_ADMIN_CLIENT_SECRET` | Yes | MAS OAuth2 admin client secret |
| `DATABASE_URL` | Yes | SQLite path (e.g. `sqlite://data/app.db`) |
| `BOT_API_SECRET` | Yes | Bearer secret for `POST /api/v1/invites` — use `openssl rand -hex 32` |
| `INVITE_ALLOWED_DOMAINS` | No | Comma-separated allowed email domains (unset = any domain) |
| `RUST_LOG` | No (default: `info`) | Log level |

See `.env.example` for a commented template.

## Keycloak Setup

> **Important:** `KEYCLOAK_REALM` is case-sensitive. Ensure it exactly matches the realm name as shown in the Keycloak admin console.

### 1. OIDC client (for admin app login)

Create a client with ID matching `OIDC_CLIENT_ID`:

- **Client authentication:** ON
- **Valid redirect URIs:** `{APP_BASE_URL}/auth/callback`
- **Client scopes → roles scope → Mappers tab:**
  - Find the `realm roles` mapper
  - Set **"Add to ID token"** = ON
  - This is required for the app to read realm roles from the ID token and enforce `APP_REQUIRED_ADMIN_ROLE`

### 2. Service account client (for Keycloak admin API)

Create a client with ID matching `KEYCLOAK_ADMIN_CLIENT_ID`:

- **Client authentication:** ON
- **Service accounts enabled:** ON
- **Assign roles to the service account** (Clients → your client → Service accounts roles tab):
  - From `realm-management` client: assign **`view-users`** and **`manage-users`**
  - `view-users` is needed for search/detail/lookup
  - `manage-users` is needed for creating users (invite flow) and deleting users

### 3. Admin role

Create a realm role named `matrix-admin` (or your configured `APP_REQUIRED_ADMIN_ROLE`) and assign it to any users who need access to the admin console.

### 4. Realm settings — Login tab

- **Edit username:** ON — required so that invited users can choose their Matrix username during onboarding (before MAS provisions their account on first login). Without this, the username is locked to the email local part.

### 5. Realm settings — Email tab (required for invite flow)

Configure your SMTP server so Keycloak can send invite emails. The `!invite` bot command triggers Keycloak's native `execute-actions-email` endpoint, which sends the set-password link. If SMTP is not configured, Keycloak returns a 500 error and the invite fails.

Required fields: Host, Port, From address. Enable SSL/TLS as appropriate for your mail provider.

## MAS Setup

Create an OAuth2 client in MAS with:
- `client_credentials` grant type
- `urn:mas:admin` scope

Use the resulting client ID and secret as `MAS_ADMIN_CLIENT_ID` / `MAS_ADMIN_CLIENT_SECRET`.

## Bot Invite Flow

The app exposes `POST /api/v1/invites` for bot-driven user invitations. A maubot plugin (`maubot-invite/`) provides the `!invite <email>` Matrix command that calls this endpoint.

**How it works:**
1. Maubot sends `POST /api/v1/invites` with `Authorization: Bearer <BOT_API_SECRET>` and `{"email": "user@example.com"}`
2. The app checks for an existing Keycloak account with that email
3. Creates a Keycloak user with required actions: `UPDATE_PASSWORD`, `UPDATE_PROFILE`, `VERIFY_EMAIL`
4. Triggers Keycloak's native invite email (requires SMTP configured — see above)
5. The user clicks the link, sets their password, and picks their username (`UPDATE_PROFILE`)
6. On first login via Element Web, MAS auto-provisions the account using their chosen username
7. Every invite attempt is audit-logged

**Maubot plugin:**
```bash
cd maubot-invite
./build.sh          # produces invite-bot.mbp
# Upload invite-bot.mbp via the maubot admin UI
```

Plugin config keys: `admin_url`, `bot_api_secret`, `ops_room_id`.

- `admin_url` — base URL of this app reachable from the maubot host (e.g. `http://192.168.1.x:3000`)
- `bot_api_secret` — must match `BOT_API_SECRET` in the app's `.env`
- `ops_room_id` — Matrix room ID where the `!invite` command is accepted (leave empty to allow any room)

## Element Web — SSO by default

To skip the password login screen and send users directly to Keycloak, add to your Element Web `config.json`:

```json
"sso_redirect_options": {
  "immediate": true
}
```

Without this, users see a password field by default and may attempt to log in with Matrix credentials instead of SSO.

## Auth Flow

1. Admin visits the app → redirected to Keycloak OIDC
2. After login, the app validates the ID token and checks for the required realm role
3. Authenticated session is stored in a secure signed cookie
4. All mutating actions are POST-only with CSRF token validation

## Common Commands

```bash
cargo build
cargo check
cargo test
cargo clippy
cargo fmt

# Inside Flox environment (required on macOS)
flox activate -- cargo run
```

## Security Notes

- All mutating endpoints are POST-only with CSRF protection
- Upstream tokens are never exposed to the browser
- All upstream API calls are server-side only
- Secrets are read from environment variables only — never hardcoded
- Every mutation writes an audit log entry

## Notes for Contributors

- Keep handlers thin; orchestration goes in services
- Upstream API details stay inside `clients/`
- Do not persist identity state locally (SQLite is audit-only)
- Do not log secrets or tokens
- Every new mutation endpoint needs an audit log entry
- Run inside Flox on macOS: `flox activate -- cargo run`
