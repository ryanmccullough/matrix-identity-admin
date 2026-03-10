# E2E Test Environment

Local Docker Compose stack for running integration tests against real infrastructure.

## Services

| Service | Image | Host Port | Purpose |
|---------|-------|-----------|---------|
| **Keycloak** | `quay.io/keycloak/keycloak:26.1` | `8081` | Identity provider — OIDC login, user management |
| **MAS** | `ghcr.io/element-hq/matrix-authentication-service:0.12.0` | `8082` | Matrix Authentication Service — OAuth2, session management |
| **Synapse** | `matrixdotorg/synapse:v1.127.1` | `8008` | Matrix homeserver — MSC3861 mode (auth delegated to MAS) |
| **Mailpit** | `axllent/mailpit:latest` | `8025` (web), `1025` (SMTP) | Captures outgoing email for invite flow testing |
| **Postgres** | `postgres:16-alpine` | not exposed | Database for MAS |

## Quick Start

```bash
# Start all services (first run takes ~60s for Keycloak realm import + MAS migrations)
docker compose -f e2e/docker-compose.yml up -d

# Copy the e2e env file and run the app
cp e2e/.env .env && flox activate -- cargo run

# Run e2e tests (app must NOT be running — tests start their own server)
flox activate -- cargo test --test e2e
```

## Test Credentials

| Account | Username | Password | Notes |
|---------|----------|----------|-------|
| Keycloak admin console | `admin` | `admin` | http://localhost:8081/admin |
| Test admin user | `testadmin` | `Admin1234!` | Has `matrix-admin` realm role |

## MSC3861 / Admin Token

Synapse runs in **MSC3861 mode** — authentication is fully delegated to MAS. Key implications:

- Synapse disables `/_matrix/client/v3/login` entirely. Login goes through MAS.
- MAS-issued compat tokens (`mct_`) **cannot** access `/_synapse/admin/*` (403 Forbidden).
- The `admin_token` in `homeserver.yaml` is a static bearer token that bypasses MAS token introspection. It works for both admin API and client API calls.
- This token **must match** `matrix.secret` in `mas.yaml`. Both are set to `e2e-matrix-shared-secret-not-real`.
- Users don't exist in Synapse until first OIDC login. Use `PUT /_synapse/admin/v2/users/{user_id}` to provision programmatically.

## Config Files

| File | Purpose |
|------|---------|
| `.env` | Environment variables for the admin app — copy to repo root |
| `docker-compose.yml` | Service definitions, ports, healthchecks |
| `homeserver.yaml` | Synapse config — MSC3861 settings, SQLite storage |
| `mas.yaml` | MAS config — upstream OIDC (Keycloak), admin OAuth2 client, compat layer |
| `keycloak-realm.json` | Pre-imported realm with test users, clients, and roles |
| `e2e.test.signing.key` | Synapse signing key for the `e2e.test` domain |
| `synapse-log.config` | Python logging config for Synapse |

## Environment Variables

The `.env` file configures the admin app to connect to the Docker services. Key values:

| Variable | Value | Notes |
|----------|-------|-------|
| `SYNAPSE_BASE_URL` | `http://localhost:8008` | |
| `SYNAPSE_ADMIN_TOKEN` | `e2e-matrix-shared-secret-not-real` | Must match `matrix.secret` in `mas.yaml` |
| `SYNAPSE_ADMIN_USER` | `@admin:e2e.test` | Fallback for non-MSC3861 mode |
| `MAS_BASE_URL` | `http://localhost:8082` | |
| `KEYCLOAK_BASE_URL` | `http://localhost:8081` | |
| `HOMESERVER_DOMAIN` | `e2e.test` | |
| `BOT_API_SECRET` | `e2e-bot-secret` | Bearer token for `POST /api/v1/invites` |

All credentials in `.env` are intentional test values, not real secrets.

## Troubleshooting

**Synapse fails on restart with permission errors:**
The `tmpfs` mount uses `uid=991,gid=991` to match the Synapse container user. If you see `PermissionError`, verify the tmpfs options in `docker-compose.yml`.

**MAS returns 500 on admin API calls:**
Check that `admin_token` in `homeserver.yaml` matches `matrix.secret` in `mas.yaml`. A mismatch causes MAS to fail token introspection.

**Room creation fails with "alias already exists":**
Synapse uses tmpfs for storage, so rooms persist until the container restarts. Run `docker compose -f e2e/docker-compose.yml restart synapse` to clear state.

**MAS healthcheck:**
MAS uses a distroless image with no shell. Readiness is polled from the host in CI. Locally, wait ~30s after `docker compose up` for MAS to be ready.
