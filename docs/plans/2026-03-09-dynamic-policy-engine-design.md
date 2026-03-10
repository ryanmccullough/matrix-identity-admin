# Dynamic Policy Engine — Design Document

**Date:** 2026-03-09
**Phase:** 3 (Extensible)
**Status:** Approved

## Goal

Replace the static `GROUP_MAPPINGS` config with a database-backed, UI-managed policy engine. Admins browse the live Matrix room/space hierarchy from Synapse, assign Keycloak groups or roles to rooms with optional power levels, and reconciliation enforces those mappings.

## Architecture

MIA gets a new Policy Management subsystem. Policy mappings are MIA's own state (not a copy of upstream identity data), stored in SQLite alongside audit logs.

### Data flow

```
Synapse (rooms/spaces) ──→ Policy UI ←── Keycloak (groups/roles)
                              │
                              ▼
                     SQLite policy_bindings
                              │
                              ▼
                    Reconciliation workflow
```

### Source-of-truth boundaries

- **Identity state** (users, sessions, groups, roles): upstream systems (Keycloak, MAS, Synapse)
- **Policy mappings** (which groups/roles map to which rooms): MIA-owned, stored in SQLite
- **Audit logs**: MIA-owned, stored in SQLite

### Layered split

- **Connectors:** fetch Keycloak groups/roles + Synapse room hierarchy
- **Workflow:** CRUD policy bindings, resolve effective policy, reconciliation decisions
- **Interface:** policy management UI + CRUD endpoints

## Database Schema

Three new tables in the existing SQLite database.

### `policy_bindings`

| Column | Type | Notes |
|--------|------|-------|
| `id` | TEXT PK | UUID |
| `subject_type` | TEXT NOT NULL | `group` or `role` |
| `subject_value` | TEXT NOT NULL | e.g. `staff`, `matrix-admin` |
| `target_type` | TEXT NOT NULL | `room` or `space` |
| `target_room_id` | TEXT NOT NULL | canonical `!room:domain` ID |
| `power_level` | INTEGER NULL | optional power level to set after join |
| `allow_remove` | BOOLEAN NOT NULL DEFAULT FALSE | per-binding kick control |
| `created_at` | TEXT NOT NULL | ISO 8601 |
| `updated_at` | TEXT NOT NULL | ISO 8601 |

Unique constraint on `(subject_type, subject_value, target_room_id)` to prevent duplicate bindings.

### `policy_targets_cache`

Cached room metadata for UI display. Reconciliation uses `room_id` only — never relies on cached names.

| Column | Type | Notes |
|--------|------|-------|
| `room_id` | TEXT PK | canonical room ID |
| `name` | TEXT NULL | room display name |
| `canonical_alias` | TEXT NULL | e.g. `#staff:example.com` |
| `parent_space_id` | TEXT NULL | parent space if known |
| `is_space` | BOOLEAN NOT NULL DEFAULT FALSE | true if this is a space |
| `last_seen_at` | TEXT NOT NULL | ISO 8601 |

### `policy_bootstrap_state`

Single-row table tracking one-time import from env/file config.

| Column | Type | Notes |
|--------|------|-------|
| `id` | INTEGER PK | always 1 |
| `bootstrap_source` | TEXT NOT NULL | `env`, `file`, or `none` |
| `bootstrap_version` | INTEGER NOT NULL | incremented on re-import |
| `bootstrapped_at` | TEXT NOT NULL | ISO 8601 |

## Connector Additions

### Synapse — new `MatrixService` trait methods

```rust
async fn list_rooms(&self, limit: u32, from: Option<&str>) -> Result<RoomList, AppError>;
async fn get_room_details(&self, room_id: &str) -> Result<RoomDetails, AppError>;
async fn set_power_level(&self, room_id: &str, user_id: &str, level: i64) -> Result<(), AppError>;
```

Endpoints:
- `GET /_synapse/admin/v1/rooms?limit=N&from=TOKEN` — paginated room list
- `GET /_synapse/admin/v1/rooms/{room_id}` — room details
- `PUT /_matrix/client/v3/rooms/{room_id}/state/m.room.power_levels` — power levels (client API)

### Keycloak — new `KeycloakIdentityProvider` trait methods

```rust
async fn list_groups(&self) -> Result<Vec<KeycloakGroup>, AppError>;
async fn list_roles(&self) -> Result<Vec<KeycloakRole>, AppError>;
```

## Domain Types

```rust
pub struct PolicyBinding {
    pub id: String,
    pub subject: PolicySubject,
    pub target: PolicyTarget,
    pub power_level: Option<i64>,
    pub allow_remove: bool,
    pub created_at: String,
    pub updated_at: String,
}

pub enum PolicySubject {
    Group(String),
    Role(String),
}

pub enum PolicyTarget {
    Room(String),   // room_id
    Space(String),  // space_id — expanded to child rooms at reconciliation time
}
```

## Workflow Layer

### `policy_service.rs`

- `list_bindings()` — all bindings with cached room names
- `create_binding(subject, target, power_level, allow_remove)` — validate + insert + audit
- `update_binding(id, ...)` — update + audit
- `delete_binding(id)` — delete + audit
- `effective_bindings_for_user(groups, roles)` — resolve which rooms a user should be in, expanding spaces to child rooms via Synapse
- `refresh_room_cache(synapse)` — fetch room list from Synapse, upsert into `policy_targets_cache`

### Reconciliation changes

- Input switches from `Vec<GroupMapping>` to `Vec<PolicyBinding>` from the database
- After joining a user to a room, set power level if the binding specifies one
- Per-binding `allow_remove` controls kick behavior (replaces the global `RECONCILE_REMOVE_FROM_ROOMS` flag)
- Existing `WorkflowOutcome` pattern preserved for partial failures

## Interface Layer

### New routes

| Method | Path | Purpose |
|--------|------|---------|
| GET | `/policy` | Policy management page |
| POST | `/policy/bindings` | Create binding |
| POST | `/policy/bindings/{id}/update` | Update binding |
| POST | `/policy/bindings/{id}/delete` | Delete binding |
| POST | `/policy/rooms/refresh` | Refresh room cache from Synapse |

### Policy page UI

Server-rendered Askama template with HTMX for interactivity:
- Table of current bindings (group/role, room/space, power level, remove flag)
- "Add Binding" form with dropdowns populated from Keycloak (groups/roles) and Synapse (rooms/spaces from cache)
- Edit/delete buttons per row
- "Refresh Rooms" button to update the cache

All mutations: POST-only, CSRF-validated, admin role required, audit logged.

## Bootstrap and Backward Compatibility

On startup in `build_state()`:
1. Check `policy_bootstrap_state` table
2. If no bootstrap has occurred AND `GROUP_MAPPINGS`/`GROUP_MAPPINGS_FILE` is set:
   - Import each mapping as a `policy_binding` (subject_type=group, target_type=room)
   - Write bootstrap marker (source, version=1, timestamp)
3. If bootstrap already occurred, skip — DB is source of truth
4. `GROUP_MAPPINGS` env var remains supported as a seed mechanism but is not re-read after bootstrap

After migration, remove `group_mappings` and `reconcile_remove_from_rooms` from `Config` struct. `PolicyEngine` reads from DB instead of config.

## Safety Controls

- Dry-run preview before apply (existing `preview_membership` extended to use DB policy)
- Per-binding `allow_remove` flag (default false) — kicks are opt-in per mapping
- Full audit entries for all policy mutations (create/update/delete binding)
- Full audit entries for all reconciliation actions (join/kick/power-level)
- Room references use `room_id` only — cached names are display-only

## Future Considerations

- **Repository abstraction:** SQLite queries behind a trait for future Postgres migration
- **Multi-instance:** SQLite works for single-instance; if horizontal scaling is needed, swap to Postgres via the repository trait
- **Webhook notifications:** policy changes could trigger webhooks (Phase 4)
- **Scheduled reconciliation:** periodic background reconciliation using DB policy (Phase 4)

## Delivery Order

1. DB migration + domain models
2. Policy repository (sqlx CRUD)
3. Policy service (CRUD + space expansion + effective bindings)
4. Synapse connector extensions (list_rooms, get_room_details, set_power_level)
5. Keycloak connector extensions (list_groups, list_roles)
6. Refactor reconciliation to use DB-backed policy + power levels
7. Policy handlers + UI template
8. Bootstrap import logic
9. Audit logging for policy mutations
10. Tests + docs updates
