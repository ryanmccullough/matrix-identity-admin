# E2E Test Environment

Local Docker Compose stack for running integration tests against real infrastructure.

## Services

| Service | Image | Host Port | Purpose |
|---------|-------|-----------|---------|
| **Keycloak** | `quay.io/keycloak/keycloak:26.5` | `8081` | Identity provider — OIDC login, user management |
| **MAS** | `ghcr.io/element-hq/matrix-authentication-service:1.13.0` | `8082` | Matrix Authentication Service — OAuth2, session management |
| **Synapse** | `matrixdotorg/synapse:v1.149.0` | `8008` | Matrix homeserver — auth delegated to MAS via `matrix_authentication_service` config |
| **Mailpit** | `axllent/mailpit:latest` | `8025` (web), `1025` (SMTP) | Captures outgoing email for invite flow testing |
| **Postgres** | `postgres:17-alpine` | not exposed | Database for MAS |

## Quick Start

```bash
# Start all services (first run takes ~60s for Keycloak realm import + MAS migrations)
docker compose -f e2e/docker-compose.yml up -d

# Provision the Synapse admin token (required once after MAS is ready)
./e2e/provision-admin-token.sh

# Copy the e2e env file and run the app
cp e2e/.env .env && flox activate -- cargo run

# Run e2e tests (app must NOT be running — tests start their own server)
flox activate -- cargo test --test e2e -- --include-ignored
```

## Test Credentials

| Account | Username | Password | Notes |
|---------|----------|----------|-------|
| Keycloak admin console | `admin` | `admin` | http://localhost:8081/admin |
| Test admin user | `testadmin` | `Admin1234!` | Has `matrix-admin` realm role |

## Synapse Admin Token

Synapse v1.147.1 uses the `matrix_authentication_service` config — authentication is fully delegated to MAS. Key implications:

- There is **no static `admin_token`** in homeserver.yaml. All tokens are validated by MAS via introspection.
- Synapse determines admin status from the `urn:synapse:admin:*` scope on the token, **not** the `admin` column in its `users` table.
- Regular `mct_` compat tokens from `m.login.password` do **not** get admin scope — by design.
- Admin tokens must be provisioned via `mas-cli manage issue-compatibility-token --yes-i-want-to-grant-synapse-admin-privileges`.
- The provisioned token is written to `e2e/shared/synapse-admin-token` and read by the test runner at startup.
- Users don't exist in Synapse until first OIDC login. Use `PUT /_synapse/admin/v2/users/{user_id}` to provision programmatically.

### Provisioning the token

```bash
# Automatic (recommended)
./e2e/provision-admin-token.sh

# Manual
docker compose -f e2e/docker-compose.yml exec mas \
  /usr/local/bin/mas-cli manage register-user --username testadmin --admin
docker compose -f e2e/docker-compose.yml exec mas \
  /usr/local/bin/mas-cli manage issue-compatibility-token testadmin \
    --yes-i-want-to-grant-synapse-admin-privileges
# Copy the mct_ token from the output to e2e/shared/synapse-admin-token
```

## Config Files

| File | Purpose |
|------|---------|
| `.env` | Environment variables for the admin app — copy to repo root |
| `docker-compose.yml` | Service definitions, ports, healthchecks |
| `homeserver.yaml` | Synapse config — `matrix_authentication_service` settings, SQLite storage |
| `mas.yaml` | MAS config — upstream OIDC (Keycloak), admin OAuth2 client, compat layer |
| `keycloak-realm.json` | Pre-imported realm with test users, clients, and roles |
| `e2e.test.signing.key` | Synapse signing key for the `e2e.test` domain |
| `synapse-log.config` | Python logging config for Synapse |
| `provision-admin-token.sh` | Helper script to provision the Synapse admin token via `mas-cli` |
| `shared/synapse-admin-token` | Generated admin token file (gitignored) |

## Environment Variables

The `.env` file configures the admin app to connect to the Docker services. Key values:

| Variable | Value | Notes |
|----------|-------|-------|
| `SYNAPSE_BASE_URL` | `http://localhost:8008` | |
| `SYNAPSE_ADMIN_TOKEN` | (from file) | Set after running `./e2e/provision-admin-token.sh` |
| `MAS_BASE_URL` | `http://localhost:8082` | |
| `KEYCLOAK_BASE_URL` | `http://localhost:8081` | |
| `HOMESERVER_DOMAIN` | `e2e.test` | |
| `BOT_API_SECRET` | `e2e-bot-secret` | Bearer token for `POST /api/v1/invites` |

The `SYNAPSE_ADMIN_TOKEN` is **not** in `.env` — it's read from `e2e/shared/synapse-admin-token` at test startup. You can override it by setting `SYNAPSE_ADMIN_TOKEN` in the environment.

All credentials in `.env` are intentional test values, not real secrets.

## Troubleshooting

**Synapse fails on restart with permission errors:**
The `tmpfs` mount uses `uid=991,gid=991` to match the Synapse container user. If you see `PermissionError`, verify the tmpfs options in `docker-compose.yml`.

**Synapse admin API returns 403 "You are not a server admin":**
The admin token must have `urn:synapse:admin:*` scope. Re-provision with `./e2e/provision-admin-token.sh`. Regular `mct_` tokens from password login will not work.

**Room creation fails with "alias already exists":**
Synapse uses tmpfs for storage, so rooms persist until the container restarts. Run `docker compose -f e2e/docker-compose.yml restart synapse` to clear state.

**MAS healthcheck:**
MAS uses a distroless image with no shell. Readiness is polled from the host in CI. Locally, wait ~30s after `docker compose up` for MAS to be ready.
