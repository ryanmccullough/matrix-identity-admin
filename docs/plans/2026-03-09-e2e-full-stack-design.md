# E2E Full Stack — Design

## Summary

Add Synapse to the e2e Docker stack (MSC3861 mode) and expand e2e tests to cover every feature endpoint. Dynamic room/space creation, shared secret admin registration, comprehensive lifecycle and reconciliation testing.

---

## Infrastructure Changes

### Add Synapse to `e2e/docker-compose.yml`

New service: `matrixdotorg/synapse:v1.127.1`

- SQLite database (no extra Postgres dependency)
- MSC3861 experimental delegation to MAS
- `registration_shared_secret` for admin user creation via API
- Server name: `e2e.test`
- Exposed on `localhost:8008`
- Depends on: MAS (which depends on Keycloak + Postgres)
- Healthcheck: `curl -f http://localhost:8008/health`

New config file: `e2e/homeserver.yaml`

Key Synapse config sections:
- `server_name: e2e.test`
- `experimental_features.msc3861` — delegates auth to MAS at `http://mas:8080/`
- `registration_shared_secret` — enables `/_synapse/admin/v1/register`
- `database: sqlite3` — lightweight, sufficient for e2e
- `suppress_key_server_warning: true`
- Logging: minimal (errors only)

### Update `e2e/mas.yaml`

Change `matrix.endpoint` from `http://localhost:8008/` to `http://synapse:8008/` (Docker network).

### Add Keycloak groups to `e2e/keycloak-realm.json`

Add two groups: `staff` and `engineering`.

Assign `testadmin` to the `staff` group. `testuser` gets no groups (useful for testing users without group membership).

### Update `e2e/.env`

Add:
```
SYNAPSE_BASE_URL=http://localhost:8008
SYNAPSE_ADMIN_USER=@admin:e2e.test
SYNAPSE_ADMIN_PASSWORD=AdminPass2026!
RECONCILE_REMOVE_FROM_ROOMS=true
```

`GROUP_MAPPINGS` is NOT set in `.env` — it's built dynamically by the test harness after creating rooms.

---

## Test Setup Flow

The test harness in `tests/e2e.rs` performs these steps:

1. **Register admin user** — POST to `http://localhost:8008/_synapse/admin/v1/register` with the shared secret HMAC. Creates `@admin:e2e.test` with admin privileges.

2. **Login as admin** — POST to `http://localhost:8008/_matrix/client/v3/login` with `m.login.password`. Gets an access token.

3. **Create rooms** — Use `/_matrix/client/v3/createRoom`:
   - `#staff-general:e2e.test` (regular room)
   - `#eng-general:e2e.test` (regular room)
   - `#eng-random:e2e.test` (regular room)
   - `#engineering-space:e2e.test` (space, with `eng-general` and `eng-random` as children via `m.space.child` state events)

4. **Build GROUP_MAPPINGS** — JSON array from the created room IDs:
   ```json
   [
     {"keycloak_group": "staff", "matrix_room_id": "!abc:e2e.test"},
     {"keycloak_group": "engineering", "matrix_room_id": "!space:e2e.test"}
   ]
   ```

5. **Set env var** — `GROUP_MAPPINGS` is set before `Config::from_env()`.

6. **Start app server** — same pattern as today (build state, spawn on random port).

### Shared Setup

Room creation and admin registration happen once per test run, not per test. A `OnceCell<SynapseSetup>` holds the room IDs and admin token. Individual tests reuse these.

### Cleanup

- Users created by tests are cleaned up via `cleanup_kc_user()` (existing pattern).
- Rooms persist across tests (created once, reused). No room cleanup needed since the Docker stack is ephemeral.

---

## Test Coverage (~25-30 tests)

### Auth & Navigation (3 tests)

- `dashboard_loads` — GET `/` with auth cookie returns 200
- `dashboard_unauthenticated_redirects` — GET `/` without auth returns 303 to `/auth/login`
- `audit_page_loads` — GET `/audit` with auth cookie returns 200

### User Search & Detail (3 tests)

- `search_finds_existing_user` — GET `/users/search?q=testadmin` returns 200, body contains `testadmin`
- `search_empty_query_returns_page` — GET `/users/search?q=nonexistent` returns 200, no results
- `user_detail_renders` — GET `/users/{testadmin_id}` returns 200, body contains username and groups

### Invite Flow (already exists — 8 tests)

Keep existing tests. No changes needed.

### Session Management (2 tests)

- `force_keycloak_logout_succeeds` — POST `/users/{id}/keycloak/logout` returns 303 redirect
- `session_revoke_succeeds` — requires a user with an active MAS session. Create user via invite, simulate OIDC login (or use MAS admin API to check for sessions), revoke one.

### Disable & Reactivate (4 tests)

- `disable_user_succeeds` — POST `/users/{id}/disable`, verify redirect, verify Keycloak user `enabled=false`
- `disable_writes_audit_log` — after disable, GET `/audit` shows `disable_identity_account_on_disable`
- `reactivate_user_succeeds` — POST `/users/{id}/reactivate`, verify redirect with `notice=User+reactivated`, verify Keycloak user `enabled=true`
- `reactivate_writes_audit_log` — GET `/audit` shows `enable_identity_account_on_reactivate`

### Offboard (2 tests)

- `offboard_user_succeeds` — POST `/users/{id}/offboard`, verify redirect, verify Keycloak `enabled=false`
- `offboard_writes_audit_log` — GET `/audit` shows disable + deactivate actions

### Delete (2 tests)

- `delete_user_succeeds` — POST `/users/{id}/delete`, verify redirect, verify user gone from Keycloak
- `delete_writes_audit_log` — GET `/audit` shows delete action

### Reconciliation (6 tests)

- `reconcile_preview_shows_actions` — POST `/users/{id}/reconcile/preview`, body contains room IDs and "join" actions
- `reconcile_joins_user_to_mapped_rooms` — POST `/users/{id}/reconcile`, verify user is now in the staff room (check via Synapse admin API)
- `reconcile_expands_space_to_children` — user in `engineering` group gets joined to space + both child rooms
- `reconcile_kicks_from_unmatched_rooms` — user NOT in a group but in a mapped room gets kicked (requires `RECONCILE_REMOVE_FROM_ROOMS=true`)
- `reconcile_writes_audit_logs` — GET `/audit` shows `join_room_on_reconcile` entries
- `bulk_reconcile_processes_users` — POST `/users/reconcile/all`, verify multiple users processed

---

## MAS User Provisioning

Several tests (session revoke, disable, offboard) need a MAS user to exist. In MSC3861 mode, MAS auto-provisions on first OIDC login. For e2e tests, we handle this by:

- Using the MAS admin API (`GET /api/admin/v1/users?filter[user]=username`) to check if the user exists
- If the user doesn't have a MAS account yet, the test verifies the non-MAS path (disable still works — it just skips session revocation)
- Tests that specifically need MAS sessions (session revoke) may be skipped if MAS provisioning isn't triggered

This is a pragmatic approach — the unit tests already cover the MAS interaction paths thoroughly.

---

## Container Startup Order

```
Postgres → Keycloak → MAS → Synapse
                              ↓
                         App (test harness)
```

Each service waits for its dependencies via healthchecks:
- Postgres: `pg_isready`
- Keycloak: `bash /dev/tcp/127.0.0.1/8080`
- MAS: `curl http://localhost:8080/health` (if available) or startup delay
- Synapse: `curl http://localhost:8008/health`

---

## Not In Scope

- Failure mode e2e tests (unit tests cover these)
- Performance or load testing
- Multi-node or federation testing
- CI pipeline changes (tests stay `#[ignore]`, run manually with `cargo test --test e2e -- --include-ignored`)
- OIDC login flow simulation (would need browser automation)
