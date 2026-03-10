# Dynamic Policy Engine Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Replace static `GROUP_MAPPINGS` config with a database-backed, UI-managed policy engine that maps Keycloak groups/roles to Matrix rooms/spaces with optional power levels.

**Architecture:** Three new SQLite tables (`policy_bindings`, `policy_targets_cache`, `policy_bootstrap_state`). New `PolicyService` for CRUD + resolution. Existing `PolicyEngine` refactored to read from DB. New Synapse/Keycloak connector methods for room listing and group/role listing. New `/policy` UI for managing bindings. Bootstrap imports existing `GROUP_MAPPINGS` on first run.

**Tech Stack:** Rust, axum, sqlx (SQLite), askama templates, HTMX, serde

---

### Task 1: Database Migration

Add the three new tables to SQLite.

**Files:**
- Create: `migrations/0002_policy_bindings.sql`

**Step 1: Write the migration SQL**

```sql
-- Policy bindings: maps a Keycloak group or role to a Matrix room or space.
CREATE TABLE IF NOT EXISTS policy_bindings (
    id              TEXT    PRIMARY KEY NOT NULL,
    subject_type    TEXT    NOT NULL CHECK (subject_type IN ('group', 'role')),
    subject_value   TEXT    NOT NULL,
    target_type     TEXT    NOT NULL CHECK (target_type IN ('room', 'space')),
    target_room_id  TEXT    NOT NULL,
    power_level     INTEGER,
    allow_remove    INTEGER NOT NULL DEFAULT 0,
    created_at      TEXT    NOT NULL,
    updated_at      TEXT    NOT NULL,
    UNIQUE (subject_type, subject_value, target_room_id)
);

-- Cached room metadata for the policy UI. Reconciliation uses room_id only.
CREATE TABLE IF NOT EXISTS policy_targets_cache (
    room_id         TEXT    PRIMARY KEY NOT NULL,
    name            TEXT,
    canonical_alias TEXT,
    parent_space_id TEXT,
    is_space        INTEGER NOT NULL DEFAULT 0,
    last_seen_at    TEXT    NOT NULL
);

-- Tracks one-time bootstrap import from GROUP_MAPPINGS env/file.
CREATE TABLE IF NOT EXISTS policy_bootstrap_state (
    id                  INTEGER PRIMARY KEY CHECK (id = 1),
    bootstrap_source    TEXT    NOT NULL,
    bootstrap_version   INTEGER NOT NULL,
    bootstrapped_at     TEXT    NOT NULL
);
```

**Step 2: Verify migration compiles**

Run: `flox activate -- cargo check`
Expected: compiles without error (sqlx picks up new migration)

**Step 3: Commit**

```bash
git add migrations/0002_policy_bindings.sql
git commit -m "feat(db): add policy_bindings migration"
```

---

### Task 2: Domain Types — PolicyBinding Model

Add the `PolicyBinding` struct and related enums.

**Files:**
- Create: `src/models/policy_binding.rs`
- Modify: `src/models/mod.rs` — add `pub mod policy_binding;`

**Step 1: Write the model**

Create `src/models/policy_binding.rs`:

```rust
use serde::{Deserialize, Serialize};

/// The subject of a policy binding — either a Keycloak group or role name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum PolicySubject {
    Group(String),
    Role(String),
}

impl PolicySubject {
    pub fn subject_type(&self) -> &str {
        match self {
            PolicySubject::Group(_) => "group",
            PolicySubject::Role(_) => "role",
        }
    }

    pub fn value(&self) -> &str {
        match self {
            PolicySubject::Group(v) | PolicySubject::Role(v) => v,
        }
    }
}

impl std::fmt::Display for PolicySubject {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicySubject::Group(v) => write!(f, "group:{v}"),
            PolicySubject::Role(v) => write!(f, "role:{v}"),
        }
    }
}

/// The target of a policy binding — a Matrix room or space ID.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "room_id")]
pub enum PolicyTarget {
    Room(String),
    Space(String),
}

impl PolicyTarget {
    pub fn target_type(&self) -> &str {
        match self {
            PolicyTarget::Room(_) => "room",
            PolicyTarget::Space(_) => "space",
        }
    }

    pub fn room_id(&self) -> &str {
        match self {
            PolicyTarget::Room(id) | PolicyTarget::Space(id) => id,
        }
    }
}

/// A policy binding maps a Keycloak group or role to a Matrix room or space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyBinding {
    pub id: String,
    pub subject: PolicySubject,
    pub target: PolicyTarget,
    /// Optional power level to set after joining (e.g. 100 for admin).
    pub power_level: Option<i64>,
    /// Whether to kick users from this room when they lose the group/role.
    pub allow_remove: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Cached metadata about a Matrix room, used for display in the policy UI.
/// Reconciliation uses `room_id` only — never relies on cached names.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedRoom {
    pub room_id: String,
    pub name: Option<String>,
    pub canonical_alias: Option<String>,
    pub parent_space_id: Option<String>,
    pub is_space: bool,
    pub last_seen_at: String,
}
```

**Step 2: Add module to mod.rs**

In `src/models/mod.rs`, add `pub mod policy_binding;`.

**Step 3: Verify it compiles**

Run: `flox activate -- cargo check`
Expected: compiles

**Step 4: Commit**

```bash
git add src/models/policy_binding.rs src/models/mod.rs
git commit -m "feat(models): add PolicyBinding and CachedRoom domain types"
```

---

### Task 3: Policy Repository — CRUD Queries

Add sqlx queries for policy bindings and room cache.

**Files:**
- Create: `src/db/policy.rs`
- Modify: `src/db/mod.rs` — add `pub mod policy;`

**Step 1: Write the repository**

Create `src/db/policy.rs`:

```rust
use sqlx::SqlitePool;

use crate::{
    error::AppError,
    models::policy_binding::{CachedRoom, PolicyBinding, PolicySubject, PolicyTarget},
};

/// List all policy bindings ordered by subject, then target.
pub async fn list_bindings(pool: &SqlitePool) -> Result<Vec<PolicyBinding>, AppError> {
    let rows = sqlx::query_as!(
        BindingRow,
        r#"SELECT id, subject_type, subject_value, target_type, target_room_id,
                  power_level, allow_remove as "allow_remove: bool",
                  created_at, updated_at
           FROM policy_bindings
           ORDER BY subject_type, subject_value, target_room_id"#
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(e.into()))?;

    Ok(rows.into_iter().map(PolicyBinding::from).collect())
}

/// Insert a new policy binding. Returns the binding's UUID.
pub async fn create_binding(
    pool: &SqlitePool,
    id: &str,
    subject_type: &str,
    subject_value: &str,
    target_type: &str,
    target_room_id: &str,
    power_level: Option<i64>,
    allow_remove: bool,
    now: &str,
) -> Result<(), AppError> {
    sqlx::query!(
        r#"INSERT INTO policy_bindings
           (id, subject_type, subject_value, target_type, target_room_id,
            power_level, allow_remove, created_at, updated_at)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        id,
        subject_type,
        subject_value,
        target_type,
        target_room_id,
        power_level,
        allow_remove,
        now,
        now,
    )
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(e.into()))?;

    Ok(())
}

/// Update an existing policy binding by ID.
pub async fn update_binding(
    pool: &SqlitePool,
    id: &str,
    power_level: Option<i64>,
    allow_remove: bool,
    now: &str,
) -> Result<bool, AppError> {
    let result = sqlx::query!(
        r#"UPDATE policy_bindings
           SET power_level = ?, allow_remove = ?, updated_at = ?
           WHERE id = ?"#,
        power_level,
        allow_remove,
        now,
        id,
    )
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(e.into()))?;

    Ok(result.rows_affected() > 0)
}

/// Delete a policy binding by ID. Returns true if a row was deleted.
pub async fn delete_binding(pool: &SqlitePool, id: &str) -> Result<bool, AppError> {
    let result = sqlx::query!("DELETE FROM policy_bindings WHERE id = ?", id)
        .execute(pool)
        .await
        .map_err(|e| AppError::Database(e.into()))?;

    Ok(result.rows_affected() > 0)
}

/// Check if a bootstrap has already been performed.
pub async fn has_bootstrapped(pool: &SqlitePool) -> Result<bool, AppError> {
    let row = sqlx::query_scalar!("SELECT COUNT(*) FROM policy_bootstrap_state")
        .fetch_one(pool)
        .await
        .map_err(|e| AppError::Database(e.into()))?;

    Ok(row > 0)
}

/// Record that a bootstrap import was performed.
pub async fn mark_bootstrapped(
    pool: &SqlitePool,
    source: &str,
    now: &str,
) -> Result<(), AppError> {
    sqlx::query!(
        r#"INSERT OR REPLACE INTO policy_bootstrap_state
           (id, bootstrap_source, bootstrap_version, bootstrapped_at)
           VALUES (1, ?, 1, ?)"#,
        source,
        now,
    )
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(e.into()))?;

    Ok(())
}

/// Upsert a cached room entry.
pub async fn upsert_cached_room(pool: &SqlitePool, room: &CachedRoom) -> Result<(), AppError> {
    sqlx::query!(
        r#"INSERT INTO policy_targets_cache
           (room_id, name, canonical_alias, parent_space_id, is_space, last_seen_at)
           VALUES (?, ?, ?, ?, ?, ?)
           ON CONFLICT(room_id) DO UPDATE SET
             name = excluded.name,
             canonical_alias = excluded.canonical_alias,
             parent_space_id = excluded.parent_space_id,
             is_space = excluded.is_space,
             last_seen_at = excluded.last_seen_at"#,
        room.room_id,
        room.name,
        room.canonical_alias,
        room.parent_space_id,
        room.is_space,
        room.last_seen_at,
    )
    .execute(pool)
    .await
    .map_err(|e| AppError::Database(e.into()))?;

    Ok(())
}

/// List all cached rooms, ordered by name.
pub async fn list_cached_rooms(pool: &SqlitePool) -> Result<Vec<CachedRoom>, AppError> {
    let rows = sqlx::query_as!(
        CachedRoom,
        r#"SELECT room_id, name, canonical_alias, parent_space_id,
                  is_space as "is_space: bool", last_seen_at
           FROM policy_targets_cache
           ORDER BY name, room_id"#
    )
    .fetch_all(pool)
    .await
    .map_err(|e| AppError::Database(e.into()))?;

    Ok(rows)
}

// ── Internal row type for query_as! ──────────────────────────────────────────

struct BindingRow {
    id: String,
    subject_type: String,
    subject_value: String,
    target_type: String,
    target_room_id: String,
    power_level: Option<i64>,
    allow_remove: bool,
    created_at: String,
    updated_at: String,
}

impl From<BindingRow> for PolicyBinding {
    fn from(row: BindingRow) -> Self {
        let subject = match row.subject_type.as_str() {
            "role" => PolicySubject::Role(row.subject_value),
            _ => PolicySubject::Group(row.subject_value),
        };
        let target = match row.target_type.as_str() {
            "space" => PolicyTarget::Space(row.target_room_id),
            _ => PolicyTarget::Room(row.target_room_id),
        };
        PolicyBinding {
            id: row.id,
            subject,
            target,
            power_level: row.power_level,
            allow_remove: row.allow_remove,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}
```

**Step 2: Add module to db/mod.rs**

Add `pub mod policy;` to `src/db/mod.rs`.

**Step 3: Verify it compiles**

Run: `flox activate -- cargo check`

**Step 4: Write repository tests**

Add tests at the bottom of `src/db/policy.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn now() -> String {
        "2026-03-09T12:00:00Z".to_string()
    }

    #[tokio::test]
    async fn create_and_list_binding() {
        let pool = test_pool().await;
        create_binding(
            &pool, "b-1", "group", "staff", "room", "!room1:test.com",
            None, false, &now(),
        ).await.unwrap();

        let bindings = list_bindings(&pool).await.unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].id, "b-1");
        assert!(matches!(&bindings[0].subject, PolicySubject::Group(g) if g == "staff"));
        assert!(matches!(&bindings[0].target, PolicyTarget::Room(r) if r == "!room1:test.com"));
        assert_eq!(bindings[0].power_level, None);
        assert!(!bindings[0].allow_remove);
    }

    #[tokio::test]
    async fn create_with_power_level_and_allow_remove() {
        let pool = test_pool().await;
        create_binding(
            &pool, "b-2", "role", "matrix-admin", "space", "!space1:test.com",
            Some(100), true, &now(),
        ).await.unwrap();

        let bindings = list_bindings(&pool).await.unwrap();
        assert_eq!(bindings[0].power_level, Some(100));
        assert!(bindings[0].allow_remove);
        assert!(matches!(&bindings[0].subject, PolicySubject::Role(r) if r == "matrix-admin"));
        assert!(matches!(&bindings[0].target, PolicyTarget::Space(_)));
    }

    #[tokio::test]
    async fn update_binding_changes_fields() {
        let pool = test_pool().await;
        create_binding(
            &pool, "b-3", "group", "staff", "room", "!room1:test.com",
            None, false, &now(),
        ).await.unwrap();

        let updated = update_binding(&pool, "b-3", Some(50), true, &now()).await.unwrap();
        assert!(updated);

        let bindings = list_bindings(&pool).await.unwrap();
        assert_eq!(bindings[0].power_level, Some(50));
        assert!(bindings[0].allow_remove);
    }

    #[tokio::test]
    async fn update_nonexistent_returns_false() {
        let pool = test_pool().await;
        let updated = update_binding(&pool, "nope", Some(50), true, &now()).await.unwrap();
        assert!(!updated);
    }

    #[tokio::test]
    async fn delete_binding_removes_row() {
        let pool = test_pool().await;
        create_binding(
            &pool, "b-4", "group", "staff", "room", "!room1:test.com",
            None, false, &now(),
        ).await.unwrap();

        let deleted = delete_binding(&pool, "b-4").await.unwrap();
        assert!(deleted);
        assert!(list_bindings(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_false() {
        let pool = test_pool().await;
        let deleted = delete_binding(&pool, "nope").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn duplicate_binding_rejected() {
        let pool = test_pool().await;
        create_binding(
            &pool, "b-5", "group", "staff", "room", "!room1:test.com",
            None, false, &now(),
        ).await.unwrap();

        let result = create_binding(
            &pool, "b-6", "group", "staff", "room", "!room1:test.com",
            None, false, &now(),
        ).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn bootstrap_state_tracks_import() {
        let pool = test_pool().await;
        assert!(!has_bootstrapped(&pool).await.unwrap());

        mark_bootstrapped(&pool, "env", &now()).await.unwrap();
        assert!(has_bootstrapped(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn upsert_cached_room_inserts_and_updates() {
        let pool = test_pool().await;
        let room = CachedRoom {
            room_id: "!room1:test.com".to_string(),
            name: Some("Staff Room".to_string()),
            canonical_alias: Some("#staff:test.com".to_string()),
            parent_space_id: None,
            is_space: false,
            last_seen_at: now(),
        };
        upsert_cached_room(&pool, &room).await.unwrap();

        let rooms = list_cached_rooms(&pool).await.unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].name, Some("Staff Room".to_string()));

        // Update name
        let updated = CachedRoom {
            name: Some("Staff Room (Updated)".to_string()),
            ..room
        };
        upsert_cached_room(&pool, &updated).await.unwrap();
        let rooms = list_cached_rooms(&pool).await.unwrap();
        assert_eq!(rooms[0].name, Some("Staff Room (Updated)".to_string()));
    }
}
```

**Step 5: Run tests**

Run: `flox activate -- cargo test db::policy`
Expected: all tests pass

**Step 6: Commit**

```bash
git add src/db/policy.rs src/db/mod.rs
git commit -m "feat(db): add policy binding repository with CRUD queries"
```

---

### Task 4: Synapse Connector Extensions

Add `list_rooms`, `get_room_details`, and `set_power_level` to the `MatrixService` trait.

**Files:**
- Modify: `src/models/synapse.rs` — add `RoomListEntry`, `RoomList`, `RoomDetails`
- Modify: `src/clients/synapse.rs` — add trait methods + impl
- Modify: `src/test_helpers.rs` — add mock implementations

**Step 1: Add new model types to `src/models/synapse.rs`**

```rust
/// A single room entry from the Synapse admin room list.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomListEntry {
    pub room_id: String,
    pub name: Option<String>,
    pub canonical_alias: Option<String>,
    /// Number of joined members.
    pub joined_members: Option<i64>,
}

/// Paginated response from GET /_synapse/admin/v1/rooms
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomList {
    pub rooms: Vec<RoomListEntry>,
    /// Opaque pagination token. `None` means no more pages.
    pub next_batch: Option<String>,
    pub total_rooms: Option<i64>,
}

/// Detailed room info from GET /_synapse/admin/v1/rooms/{room_id}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomDetails {
    pub room_id: String,
    pub name: Option<String>,
    pub canonical_alias: Option<String>,
    pub topic: Option<String>,
    pub joined_members: Option<i64>,
    /// Not directly returned by Synapse — inferred from space child events.
    /// We set this in the connector after checking space children.
    #[serde(default)]
    pub is_space: bool,
}
```

**Step 2: Add trait methods to `MatrixService` in `src/clients/synapse.rs`**

Add these to the `MatrixService` trait:

```rust
/// List rooms known to the server (paginated).
async fn list_rooms(&self, limit: u32, from: Option<&str>) -> Result<RoomList, AppError>;

/// Get details for a specific room.
async fn get_room_details(&self, room_id: &str) -> Result<RoomDetails, AppError>;

/// Set a user's power level in a room.
async fn set_power_level(
    &self,
    room_id: &str,
    user_id: &str,
    level: i64,
) -> Result<(), AppError>;
```

**Step 3: Implement on `SynapseClient`**

Add to `impl MatrixService for SynapseClient`:

```rust
async fn list_rooms(&self, limit: u32, from: Option<&str>) -> Result<RoomList, AppError> {
    let token = self.admin_token().await?;
    let mut url = format!("{}/_synapse/admin/v1/rooms?limit={limit}", self.config.base_url);
    if let Some(from) = from {
        url.push_str(&format!("&from={from}"));
    }

    let resp: RoomList = self
        .http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| upstream_error("synapse", e))?
        .error_for_status()
        .map_err(|e| upstream_error("synapse", e))?
        .json()
        .await
        .map_err(|e| upstream_error("synapse", e))?;

    Ok(resp)
}

async fn get_room_details(&self, room_id: &str) -> Result<RoomDetails, AppError> {
    let token = self.admin_token().await?;
    let encoded = urlencoded(room_id);
    let url = self.url(&format!("/_synapse/admin/v1/rooms/{encoded}"));

    let mut details: RoomDetails = self
        .http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| upstream_error("synapse", e))?
        .error_for_status()
        .map_err(|e| upstream_error("synapse", e))?
        .json()
        .await
        .map_err(|e| upstream_error("synapse", e))?;

    // Check if this room is a space by looking for m.space.child events.
    if let Ok(children) = self.get_space_children(room_id).await {
        details.is_space = !children.is_empty();
    }

    Ok(details)
}

async fn set_power_level(
    &self,
    room_id: &str,
    user_id: &str,
    level: i64,
) -> Result<(), AppError> {
    let token = self.admin_token().await?;
    let encoded = urlencoded(room_id);
    let url = self.url(&format!(
        "/_matrix/client/v3/rooms/{encoded}/state/m.room.power_levels"
    ));

    // First, get the current power levels so we can modify them.
    let mut power_levels: serde_json::Value = self
        .http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| upstream_error("synapse", e))?
        .error_for_status()
        .map_err(|e| upstream_error("synapse", e))?
        .json()
        .await
        .map_err(|e| upstream_error("synapse", e))?;

    // Update the user's power level.
    let users = power_levels
        .as_object_mut()
        .and_then(|o| o.entry("users").or_insert(serde_json::json!({})).as_object_mut());
    if let Some(users) = users {
        users.insert(user_id.to_string(), serde_json::json!(level));
    }

    // PUT back the updated power levels.
    self.http
        .put(&url)
        .bearer_auth(&token)
        .json(&power_levels)
        .send()
        .await
        .map_err(|e| upstream_error("synapse", e))?
        .error_for_status()
        .map_err(|e| upstream_error("synapse", e))?;

    Ok(())
}
```

**Step 4: Add mock implementations to `src/test_helpers.rs`**

Add to `MockSynapse` struct:

```rust
pub room_list: Vec<RoomListEntry>,
pub room_details: Option<RoomDetails>,
pub fail_list_rooms: bool,
pub fail_set_power_level: bool,
```

Add to `impl MatrixService for MockSynapse`:

```rust
async fn list_rooms(&self, _limit: u32, _from: Option<&str>) -> Result<RoomList, AppError> {
    if self.fail_list_rooms {
        return Err(AppError::Upstream {
            service: "synapse".into(),
            message: "mock list_rooms failure".into(),
        });
    }
    Ok(RoomList {
        rooms: self.room_list.clone(),
        next_batch: None,
        total_rooms: Some(self.room_list.len() as i64),
    })
}

async fn get_room_details(&self, _room_id: &str) -> Result<RoomDetails, AppError> {
    self.room_details.clone().ok_or_else(|| AppError::NotFound("room not found".into()))
}

async fn set_power_level(
    &self,
    _room_id: &str,
    _user_id: &str,
    _level: i64,
) -> Result<(), AppError> {
    if self.fail_set_power_level {
        return Err(AppError::Upstream {
            service: "synapse".into(),
            message: "mock set_power_level failure".into(),
        });
    }
    Ok(())
}
```

Also add the unimplemented stubs to the `MockSynapse` in `src/services/reconcile_membership.rs` tests (that module has its own local mock).

**Step 5: Verify all tests pass**

Run: `flox activate -- cargo test`
Expected: all 271+ tests pass

**Step 6: Commit**

```bash
git add src/models/synapse.rs src/clients/synapse.rs src/test_helpers.rs src/services/reconcile_membership.rs
git commit -m "feat(synapse): add list_rooms, get_room_details, set_power_level to MatrixService"
```

---

### Task 5: Keycloak Connector Extensions

Add `list_groups` and `list_roles` to the `KeycloakIdentityProvider` trait.

**Files:**
- Modify: `src/clients/keycloak.rs` — add trait methods + impl
- Modify: `src/test_helpers.rs` — add mock implementations

**Step 1: Add trait methods**

Add to `KeycloakIdentityProvider` trait in `src/clients/keycloak.rs`:

```rust
/// List all groups in the realm.
async fn list_groups(&self) -> Result<Vec<KeycloakGroup>, AppError>;

/// List all realm-level roles.
async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>, AppError>;
```

**Step 2: Implement on `KeycloakClient`**

```rust
async fn list_groups(&self) -> Result<Vec<KeycloakGroup>, AppError> {
    let token = self.admin_token().await?;
    let url = format!(
        "{}/admin/realms/{}/groups?briefRepresentation=true",
        self.config.base_url, self.config.realm
    );

    let groups: Vec<KeycloakGroup> = self
        .http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| upstream_error("keycloak", e))?
        .error_for_status()
        .map_err(|e| upstream_error("keycloak", e))?
        .json()
        .await
        .map_err(|e| upstream_error("keycloak", e))?;

    Ok(groups)
}

async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>, AppError> {
    let token = self.admin_token().await?;
    let url = format!(
        "{}/admin/realms/{}/roles?briefRepresentation=true",
        self.config.base_url, self.config.realm
    );

    let roles: Vec<KeycloakRole> = self
        .http
        .get(&url)
        .bearer_auth(&token)
        .send()
        .await
        .map_err(|e| upstream_error("keycloak", e))?
        .error_for_status()
        .map_err(|e| upstream_error("keycloak", e))?
        .json()
        .await
        .map_err(|e| upstream_error("keycloak", e))?;

    Ok(roles)
}
```

**Step 3: Add mock implementations**

In `src/test_helpers.rs`, add to `MockKeycloak`:

```rust
pub all_groups: Vec<KeycloakGroup>,
pub all_roles: Vec<KeycloakRole>,
```

And implement:

```rust
async fn list_groups(&self) -> Result<Vec<KeycloakGroup>, AppError> {
    Ok(self.all_groups.clone())
}

async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>, AppError> {
    Ok(self.all_roles.clone())
}
```

**Step 4: Verify tests pass**

Run: `flox activate -- cargo test`

**Step 5: Commit**

```bash
git add src/clients/keycloak.rs src/test_helpers.rs
git commit -m "feat(keycloak): add list_groups and list_realm_roles to KeycloakIdentityProvider"
```

---

### Task 6: Policy Service

Create the service that provides CRUD + effective binding resolution.

**Files:**
- Create: `src/services/policy_service.rs`
- Modify: `src/services/mod.rs` — add `pub mod policy_service;`

**Step 1: Write the service**

Create `src/services/policy_service.rs`:

```rust
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    clients::MatrixService,
    db::policy as policy_db,
    error::AppError,
    models::policy_binding::{CachedRoom, PolicyBinding, PolicySubject, PolicyTarget},
    services::audit_service::AuditService,
};

/// Service for managing policy bindings and resolving effective policy.
pub struct PolicyService {
    pool: SqlitePool,
}

impl PolicyService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    /// List all policy bindings.
    pub async fn list_bindings(&self) -> Result<Vec<PolicyBinding>, AppError> {
        policy_db::list_bindings(&self.pool).await
    }

    /// Create a new policy binding.
    pub async fn create_binding(
        &self,
        subject: &PolicySubject,
        target: &PolicyTarget,
        power_level: Option<i64>,
        allow_remove: bool,
        audit: &AuditService,
        actor_subject: &str,
        actor_username: &str,
    ) -> Result<PolicyBinding, AppError> {
        let id = Uuid::now_v7().to_string();
        let now = chrono::Utc::now().to_rfc3339();

        policy_db::create_binding(
            &self.pool,
            &id,
            subject.subject_type(),
            subject.value(),
            target.target_type(),
            target.room_id(),
            power_level,
            allow_remove,
            &now,
        )
        .await?;

        // Audit the policy change.
        let _ = audit
            .log(
                actor_subject,
                actor_username,
                None,
                None,
                "create_policy_binding",
                crate::models::audit::AuditResult::Success,
                serde_json::json!({
                    "binding_id": id,
                    "subject": subject.to_string(),
                    "target_room_id": target.room_id(),
                    "power_level": power_level,
                    "allow_remove": allow_remove,
                }),
            )
            .await;

        Ok(PolicyBinding {
            id,
            subject: subject.clone(),
            target: target.clone(),
            power_level,
            allow_remove,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    /// Update an existing binding's power level and allow_remove flag.
    pub async fn update_binding(
        &self,
        id: &str,
        power_level: Option<i64>,
        allow_remove: bool,
        audit: &AuditService,
        actor_subject: &str,
        actor_username: &str,
    ) -> Result<bool, AppError> {
        let now = chrono::Utc::now().to_rfc3339();
        let updated = policy_db::update_binding(&self.pool, id, power_level, allow_remove, &now)
            .await?;

        if updated {
            let _ = audit
                .log(
                    actor_subject,
                    actor_username,
                    None,
                    None,
                    "update_policy_binding",
                    crate::models::audit::AuditResult::Success,
                    serde_json::json!({
                        "binding_id": id,
                        "power_level": power_level,
                        "allow_remove": allow_remove,
                    }),
                )
                .await;
        }

        Ok(updated)
    }

    /// Delete a policy binding.
    pub async fn delete_binding(
        &self,
        id: &str,
        audit: &AuditService,
        actor_subject: &str,
        actor_username: &str,
    ) -> Result<bool, AppError> {
        let deleted = policy_db::delete_binding(&self.pool, id).await?;

        if deleted {
            let _ = audit
                .log(
                    actor_subject,
                    actor_username,
                    None,
                    None,
                    "delete_policy_binding",
                    crate::models::audit::AuditResult::Success,
                    serde_json::json!({ "binding_id": id }),
                )
                .await;
        }

        Ok(deleted)
    }

    /// Resolve the effective bindings for a user given their groups and roles.
    ///
    /// Returns all bindings where the user has the matching group or role.
    /// Space targets are NOT expanded here — that happens at reconciliation time.
    pub fn effective_bindings_for_user<'a>(
        &self,
        bindings: &'a [PolicyBinding],
        user_groups: &[String],
        user_roles: &[String],
    ) -> Vec<&'a PolicyBinding> {
        bindings
            .iter()
            .filter(|b| match &b.subject {
                PolicySubject::Group(g) => user_groups.iter().any(|ug| ug == g),
                PolicySubject::Role(r) => user_roles.iter().any(|ur| ur == r),
            })
            .collect()
    }

    /// Refresh the room cache from Synapse.
    pub async fn refresh_room_cache(
        &self,
        synapse: &dyn MatrixService,
    ) -> Result<usize, AppError> {
        let now = chrono::Utc::now().to_rfc3339();
        let mut count = 0usize;
        let mut from: Option<String> = None;

        loop {
            let page = synapse.list_rooms(100, from.as_deref()).await?;
            for entry in &page.rooms {
                let is_space = synapse
                    .get_space_children(&entry.room_id)
                    .await
                    .map(|c| !c.is_empty())
                    .unwrap_or(false);

                let cached = CachedRoom {
                    room_id: entry.room_id.clone(),
                    name: entry.name.clone(),
                    canonical_alias: entry.canonical_alias.clone(),
                    parent_space_id: None,
                    is_space,
                    last_seen_at: now.clone(),
                };
                policy_db::upsert_cached_room(&self.pool, &cached).await?;
                count += 1;
            }

            match page.next_batch {
                Some(token) => from = Some(token),
                None => break,
            }
        }

        Ok(count)
    }

    /// List cached rooms for the policy UI.
    pub async fn list_cached_rooms(&self) -> Result<Vec<CachedRoom>, AppError> {
        policy_db::list_cached_rooms(&self.pool).await
    }

    /// Bootstrap policy from GROUP_MAPPINGS env var if not already done.
    pub async fn bootstrap_from_env(
        &self,
        mappings: &[crate::models::group_mapping::GroupMapping],
        source: &str,
    ) -> Result<usize, AppError> {
        if policy_db::has_bootstrapped(&self.pool).await? {
            return Ok(0);
        }

        let now = chrono::Utc::now().to_rfc3339();
        let mut count = 0;

        for mapping in mappings {
            let id = Uuid::now_v7().to_string();
            // Ignore errors from duplicates during bootstrap.
            let result = policy_db::create_binding(
                &self.pool,
                &id,
                "group",
                &mapping.keycloak_group,
                "room",
                &mapping.matrix_room_id,
                None,
                false,
                &now,
            )
            .await;

            if result.is_ok() {
                count += 1;
            }
        }

        policy_db::mark_bootstrapped(&self.pool, source, &now).await?;
        Ok(count)
    }
}
```

**Step 2: Add `uuid` and `chrono` dependencies**

Run: `flox activate -- cargo add uuid --features v7` and `flox activate -- cargo add chrono`

Note: `chrono` may already be a dependency. Check `Cargo.toml` first.

**Step 3: Add module to services/mod.rs**

Add `pub mod policy_service;` and `pub use policy_service::PolicyService;` to `src/services/mod.rs`.

**Step 4: Write tests**

Add tests at the bottom of `src/services/policy_service.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::group_mapping::GroupMapping;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_service() -> (PolicyService, AuditService) {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let audit = AuditService::new(pool.clone());
        (PolicyService::new(pool), audit)
    }

    #[tokio::test]
    async fn create_and_list_bindings() {
        let (svc, audit) = test_service().await;
        let binding = svc
            .create_binding(
                &PolicySubject::Group("staff".into()),
                &PolicyTarget::Room("!room1:test.com".into()),
                None,
                false,
                &audit,
                "sub",
                "admin",
            )
            .await
            .unwrap();

        assert_eq!(binding.subject, PolicySubject::Group("staff".into()));

        let all = svc.list_bindings().await.unwrap();
        assert_eq!(all.len(), 1);
    }

    #[tokio::test]
    async fn effective_bindings_filters_by_group_and_role() {
        let (svc, audit) = test_service().await;
        svc.create_binding(
            &PolicySubject::Group("staff".into()),
            &PolicyTarget::Room("!room1:test.com".into()),
            None, false, &audit, "sub", "admin",
        ).await.unwrap();
        svc.create_binding(
            &PolicySubject::Role("matrix-admin".into()),
            &PolicyTarget::Room("!room2:test.com".into()),
            Some(100), false, &audit, "sub", "admin",
        ).await.unwrap();
        svc.create_binding(
            &PolicySubject::Group("contractors".into()),
            &PolicyTarget::Room("!room3:test.com".into()),
            None, false, &audit, "sub", "admin",
        ).await.unwrap();

        let all = svc.list_bindings().await.unwrap();
        let effective = svc.effective_bindings_for_user(
            &all,
            &["staff".into()],
            &["matrix-admin".into()],
        );

        assert_eq!(effective.len(), 2);
        // Should include staff group and matrix-admin role, not contractors
    }

    #[tokio::test]
    async fn bootstrap_imports_mappings_once() {
        let (svc, _) = test_service().await;
        let mappings = vec![
            GroupMapping {
                keycloak_group: "staff".into(),
                matrix_room_id: "!room1:test.com".into(),
            },
            GroupMapping {
                keycloak_group: "admins".into(),
                matrix_room_id: "!room2:test.com".into(),
            },
        ];

        let count = svc.bootstrap_from_env(&mappings, "env").await.unwrap();
        assert_eq!(count, 2);

        // Second call should be a no-op.
        let count2 = svc.bootstrap_from_env(&mappings, "env").await.unwrap();
        assert_eq!(count2, 0);

        let all = svc.list_bindings().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    #[tokio::test]
    async fn delete_binding_works() {
        let (svc, audit) = test_service().await;
        let binding = svc
            .create_binding(
                &PolicySubject::Group("staff".into()),
                &PolicyTarget::Room("!room1:test.com".into()),
                None, false, &audit, "sub", "admin",
            )
            .await
            .unwrap();

        let deleted = svc.delete_binding(&binding.id, &audit, "sub", "admin").await.unwrap();
        assert!(deleted);
        assert!(svc.list_bindings().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn create_binding_writes_audit_log() {
        let (svc, audit) = test_service().await;
        svc.create_binding(
            &PolicySubject::Group("staff".into()),
            &PolicyTarget::Room("!room1:test.com".into()),
            None, false, &audit, "sub", "admin",
        ).await.unwrap();

        let logs = audit.recent(10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "create_policy_binding");
    }
}
```

**Step 5: Run tests**

Run: `flox activate -- cargo test services::policy_service`
Expected: all tests pass

**Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock src/services/policy_service.rs src/services/mod.rs
git commit -m "feat(services): add PolicyService with CRUD, resolution, and bootstrap"
```

---

### Task 7: Refactor Reconciliation to Use PolicyBinding

Update `reconcile_membership` and `preview_membership` to accept `&[PolicyBinding]` instead of `&PolicyEngine`, adding role support and per-binding `allow_remove`.

**Files:**
- Modify: `src/services/reconcile_membership.rs`
- Modify: `src/handlers/reconcile.rs`
- Modify: `src/handlers/bulk_reconcile.rs`

**Step 1: Update `reconcile_membership` signature and logic**

Change the function to accept `&[PolicyBinding]` and user roles. The key changes:

- Replace `policy: &PolicyEngine` with `bindings: &[PolicyBinding]`
- Add `keycloak_roles: &[String]` parameter
- Replace `remove_from_rooms: bool` with per-binding `allow_remove`
- After force-join, set power level if specified
- Match on `PolicySubject::Group` or `PolicySubject::Role`

The iteration logic stays the same (expand targets, join/kick), but the matching changes from `keycloak_groups.contains(&mapping.keycloak_group)` to checking whether the binding's subject matches the user's groups or roles.

**Step 2: Update `preview_membership` similarly**

Same signature changes. Add power level info to `RoomAction`.

**Step 3: Update `ReconcilePreview` and `RoomAction`**

```rust
pub struct RoomAction {
    pub room_id: String,
    pub subject: String,      // was: keycloak_group
    pub power_level: Option<i64>,  // new
}
```

**Step 4: Update handlers**

In `handlers/reconcile.rs`:
- Load bindings from `PolicyService` instead of `state.policy`
- Get user roles from Keycloak
- Pass roles to reconcile/preview functions
- Remove `state.config.reconcile_remove_from_rooms` usage

In `handlers/bulk_reconcile.rs`:
- Same changes

**Step 5: Update all tests**

Update tests in `reconcile_membership.rs`, `handlers/reconcile.rs`, and `handlers/bulk_reconcile.rs` to use `PolicyBinding` instead of `GroupMapping`/`PolicyEngine`.

**Step 6: Run tests**

Run: `flox activate -- cargo test`
Expected: all tests pass

**Step 7: Commit**

```bash
git add src/services/reconcile_membership.rs src/handlers/reconcile.rs src/handlers/bulk_reconcile.rs
git commit -m "refactor(reconcile): switch from PolicyEngine to PolicyBinding with role+power level support"
```

---

### Task 8: Wire PolicyService into AppState

Connect the new service to the app and add bootstrap logic.

**Files:**
- Modify: `src/state.rs` — add `policy_service` field
- Modify: `src/lib.rs` — create `PolicyService`, run bootstrap in `build_state`
- Modify: `src/test_helpers.rs` — add `PolicyService` to test state builders

**Step 1: Add to AppState**

```rust
pub policy_service: Arc<PolicyService>,
```

**Step 2: Create and bootstrap in `build_state`**

In `src/lib.rs`:

```rust
let policy_service = Arc::new(PolicyService::new(pool.clone()));

// Bootstrap: import GROUP_MAPPINGS into DB on first run.
if !config.group_mappings.is_empty() {
    let source = if std::env::var("GROUP_MAPPINGS_FILE").is_ok() {
        "file"
    } else {
        "env"
    };
    let imported = policy_service
        .bootstrap_from_env(&config.group_mappings, source)
        .await?;
    if imported > 0 {
        tracing::info!("Bootstrapped {imported} policy bindings from {source}");
    }
}
```

**Step 3: Update test state builders**

Add `PolicyService` to test state construction in `test_helpers.rs`.

**Step 4: Run tests**

Run: `flox activate -- cargo test`

**Step 5: Commit**

```bash
git add src/state.rs src/lib.rs src/test_helpers.rs
git commit -m "feat(state): wire PolicyService into AppState with bootstrap"
```

---

### Task 9: Policy Handlers and UI

Create the policy management page and CRUD endpoints.

**Files:**
- Create: `src/handlers/policy.rs`
- Create: `templates/policy.html`
- Modify: `src/handlers/mod.rs` — add `pub mod policy;`
- Modify: `src/lib.rs` — add routes
- Modify: `templates/base.html` — add nav link

**Step 1: Create the policy handler**

Create `src/handlers/policy.rs` with:
- `GET /policy` — renders the policy page with current bindings, groups, roles, cached rooms
- `POST /policy/bindings` — create a new binding
- `POST /policy/bindings/{id}/update` — update a binding
- `POST /policy/bindings/{id}/delete` — delete a binding
- `POST /policy/rooms/refresh` — refresh room cache from Synapse

All mutations: CSRF-validated, admin role required, audit logged.

**Step 2: Create the template**

Create `templates/policy.html` extending `base.html`:
- Table of current bindings with edit/delete buttons
- "Add Binding" form with dropdowns for subject (groups/roles) and target (rooms from cache)
- Power level input, allow_remove checkbox
- "Refresh Rooms" button

**Step 3: Add routes to lib.rs**

```rust
.route("/policy", get(handlers::policy::list))
.route("/policy/bindings", post(handlers::policy::create))
.route("/policy/bindings/{id}/update", post(handlers::policy::update))
.route("/policy/bindings/{id}/delete", post(handlers::policy::delete))
.route("/policy/rooms/refresh", post(handlers::policy::refresh_rooms))
```

**Step 4: Add nav link**

In `templates/base.html`, add "Policy" to the nav bar (alongside Dashboard, Users, Audit).

**Step 5: Write handler tests**

Standard test coverage:
- Unauthenticated → redirect to login
- Invalid CSRF → 400
- Success → redirect with notice
- Missing Synapse → 404 for refresh

**Step 6: Run tests**

Run: `flox activate -- cargo test`

**Step 7: Commit**

```bash
git add src/handlers/policy.rs src/handlers/mod.rs templates/policy.html templates/base.html src/lib.rs
git commit -m "feat(policy): add policy management page with CRUD handlers"
```

---

### Task 10: Clean Up Legacy Policy Code

Remove the old `PolicyEngine`, `GroupMapping` references, and `reconcile_remove_from_rooms` config.

**Files:**
- Modify: `src/models/policy.rs` — remove or repurpose `PolicyEngine`
- Modify: `src/config.rs` — keep `group_mappings` for bootstrap only, remove `reconcile_remove_from_rooms`
- Modify: `src/state.rs` — remove `policy: Arc<PolicyEngine>` field
- Various handlers/tests that reference the old types

**Step 1: Remove PolicyEngine from AppState**

Keep `PolicyEngine` as a lightweight wrapper if needed for backward compat in tests, or remove entirely if all callers have been migrated to `PolicyService`.

**Step 2: Remove `reconcile_remove_from_rooms` from Config**

The per-binding `allow_remove` replaces the global flag.

**Step 3: Update all remaining references**

Search for `PolicyEngine`, `reconcile_remove_from_rooms`, and `group_mappings` across the codebase. Update or remove as appropriate.

**Step 4: Run tests**

Run: `flox activate -- cargo test`

**Step 5: Run pre-commit checks**

```bash
flox activate -- cargo fmt
flox activate -- cargo clippy --all-targets -- -D warnings
flox activate -- cargo test
```

**Step 6: Commit**

```bash
git add -u
git commit -m "refactor(policy): remove legacy PolicyEngine and global reconcile flag"
```

---

### Task 11: Documentation Updates

Update CLAUDE.md, AGENTS.md, and README.md to reflect the new policy system.

**Files:**
- Modify: `CLAUDE.md` — update policy section, config vars, architecture
- Modify: `AGENTS.md` — update policy references
- Modify: `README.md` — update config table, add policy section

**Step 1: Update CLAUDE.md**

- Remove `GROUP_MAPPINGS` from "new config vars" section (now bootstrap-only)
- Remove `RECONCILE_REMOVE_FROM_ROOMS` (replaced by per-binding flag)
- Update architecture section to describe `PolicyService` and SQLite policy storage
- Update roadmap: Phase 3 → done

**Step 2: Update README.md**

- Add policy management section
- Update config table (mark `GROUP_MAPPINGS` as bootstrap/migration only)
- Add `/policy` to the routes list

**Step 3: Update AGENTS.md**

- Update policy section

**Step 4: Commit**

```bash
git add CLAUDE.md AGENTS.md README.md
git commit -m "docs: update for dynamic policy engine"
```

---

## Dependencies Between Tasks

```
Task 1 (migration) ──────────────┐
Task 2 (domain types) ───────────┤
                                  ├──→ Task 3 (repository)
                                  │         │
Task 4 (synapse connector) ──────┤         │
Task 5 (keycloak connector) ─────┤         ▼
                                  ├──→ Task 6 (policy service)
                                  │         │
                                  │         ▼
                                  ├──→ Task 7 (refactor reconcile)
                                  │         │
                                  │         ▼
                                  └──→ Task 8 (wire into state)
                                            │
                                            ▼
                                      Task 9 (handlers + UI)
                                            │
                                            ▼
                                      Task 10 (cleanup)
                                            │
                                            ▼
                                      Task 11 (docs)
```

**Parallelizable groups:**
- Tasks 1, 2, 4, 5 can run in parallel (no dependencies between them)
- Task 3 depends on Tasks 1 + 2
- Task 6 depends on Task 3
- Task 7 depends on Tasks 4, 5, 6
- Tasks 8-11 are sequential
