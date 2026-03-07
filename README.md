# Matrix Identity Admin

Thin internal admin console for a self-hosted Matrix stack running [MSC3861](https://github.com/matrix-org/matrix-spec-proposals/pull/3861) (Synapse delegates auth to MAS).

It provides a unified admin view over:
- **Keycloak** — identity source of truth
- **MAS (Matrix Authentication Service)** — auth and session layer

Supported admin actions:
- Revoke MAS sessions (compat and OAuth2)
- Force Keycloak logout

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
```

## Quick Start

### Prerequisites

- Rust toolchain (`cargo`, `rustc`)
- Access to Keycloak and MAS admin endpoints

### Configure environment

```bash
cp .env.example .env
# Edit .env with your values
```

### Run locally

```bash
cargo run
```

Default bind: `127.0.0.1:3000`

### Docker

```bash
docker compose up --build
```

## Environment Variables

| Variable | Required | Description |
|---|---|---|
| `APP_BIND_ADDR` | No (default: `127.0.0.1:3000`) | Listen address |
| `APP_BASE_URL` | Yes | Public base URL (e.g. `http://localhost:3000`) |
| `APP_SESSION_SECRET` | Yes | Secret for cookie signing — use `openssl rand -hex 32` |
| `APP_REQUIRED_ADMIN_ROLE` | No (default: `matrix-admin`) | Keycloak realm role required to access the app |
| `HOMESERVER_DOMAIN` | Yes | Matrix homeserver domain (e.g. `example.com`) |
| `OIDC_ISSUER_URL` | Yes | Keycloak realm URL (e.g. `https://keycloak.example.com/realms/matrix`) |
| `OIDC_CLIENT_ID` | Yes | OIDC client ID |
| `OIDC_CLIENT_SECRET` | Yes | OIDC client secret |
| `OIDC_REDIRECT_URL` | Yes | Callback URL (e.g. `http://localhost:3000/auth/callback`) |
| `KEYCLOAK_BASE_URL` | Yes | Keycloak base URL |
| `KEYCLOAK_REALM` | Yes | Keycloak realm name (case-sensitive) |
| `KEYCLOAK_ADMIN_CLIENT_ID` | Yes | Service account client ID |
| `KEYCLOAK_ADMIN_CLIENT_SECRET` | Yes | Service account client secret |
| `MAS_BASE_URL` | Yes | MAS base URL (e.g. `https://matrix.example.com/auth`) |
| `MAS_ADMIN_CLIENT_ID` | Yes | MAS OAuth2 admin client ID |
| `MAS_ADMIN_CLIENT_SECRET` | Yes | MAS OAuth2 admin client secret |
| `DATABASE_URL` | Yes | SQLite path (e.g. `sqlite://data/app.db`) |
| `RUST_LOG` | No (default: `info`) | Log level |

See `.env.example` for a commented template.

## Keycloak Setup

1. Create an OIDC client for the admin app (`OIDC_CLIENT_ID`):
   - Client authentication: ON
   - Valid redirect URIs: `{APP_BASE_URL}/auth/callback`
   - Add a `realm roles` mapper to the ID token (required for role-based access control)

2. Create a service account client (`KEYCLOAK_ADMIN_CLIENT_ID`):
   - Service accounts enabled: ON
   - Assign the `view-users` role from `realm-management` to the service account

3. Assign the `matrix-admin` realm role (or your configured `APP_REQUIRED_ADMIN_ROLE`) to any admin users who need access.

## MAS Setup

Create an OAuth2 client in MAS with:
- `client_credentials` grant type
- `urn:mas:admin` scope

Use the resulting client ID and secret as `MAS_ADMIN_CLIENT_ID` / `MAS_ADMIN_CLIENT_SECRET`.

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
