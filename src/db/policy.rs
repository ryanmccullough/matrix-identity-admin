use sqlx::SqlitePool;

use crate::{
    error::AppError,
    models::policy_binding::{CachedRoom, PolicyBinding, PolicySubject, PolicyTarget},
};

/// Internal flat struct matching the policy_bindings SQLite columns.
#[derive(sqlx::FromRow)]
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
    fn from(r: BindingRow) -> Self {
        let subject = match r.subject_type.as_str() {
            "role" => PolicySubject::Role(r.subject_value),
            _ => PolicySubject::Group(r.subject_value),
        };
        let target = match r.target_type.as_str() {
            "space" => PolicyTarget::Space(r.target_room_id),
            _ => PolicyTarget::Room(r.target_room_id),
        };
        PolicyBinding {
            id: r.id,
            subject,
            target,
            power_level: r.power_level,
            allow_remove: r.allow_remove,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

/// Internal flat struct matching the policy_targets_cache SQLite columns.
#[derive(sqlx::FromRow)]
struct CachedRoomRow {
    room_id: String,
    name: Option<String>,
    canonical_alias: Option<String>,
    parent_space_id: Option<String>,
    is_space: bool,
    last_seen_at: String,
}

impl From<CachedRoomRow> for CachedRoom {
    fn from(r: CachedRoomRow) -> Self {
        CachedRoom {
            room_id: r.room_id,
            name: r.name,
            canonical_alias: r.canonical_alias,
            parent_space_id: r.parent_space_id,
            is_space: r.is_space,
            last_seen_at: r.last_seen_at,
        }
    }
}

/// List all policy bindings, ordered by subject then target.
pub async fn list_bindings(pool: &SqlitePool) -> Result<Vec<PolicyBinding>, AppError> {
    let rows: Vec<BindingRow> = sqlx::query_as(
        r#"
        SELECT id, subject_type, subject_value, target_type, target_room_id,
               power_level, allow_remove, created_at, updated_at
        FROM policy_bindings
        ORDER BY subject_type, subject_value, target_room_id
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

/// Create a new policy binding.
#[allow(clippy::too_many_arguments)]
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
    sqlx::query(
        r#"
        INSERT INTO policy_bindings
            (id, subject_type, subject_value, target_type, target_room_id,
             power_level, allow_remove, created_at, updated_at)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(id)
    .bind(subject_type)
    .bind(subject_value)
    .bind(target_type)
    .bind(target_room_id)
    .bind(power_level)
    .bind(allow_remove)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

/// Update an existing binding's power_level and allow_remove fields.
/// Returns `true` if the row existed and was updated.
pub async fn update_binding(
    pool: &SqlitePool,
    id: &str,
    power_level: Option<i64>,
    allow_remove: bool,
    now: &str,
) -> Result<bool, AppError> {
    let result = sqlx::query(
        r#"
        UPDATE policy_bindings
        SET power_level = ?, allow_remove = ?, updated_at = ?
        WHERE id = ?
        "#,
    )
    .bind(power_level)
    .bind(allow_remove)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    Ok(result.rows_affected() > 0)
}

/// Delete a policy binding by id.
/// Returns `true` if the row existed and was deleted.
pub async fn delete_binding(pool: &SqlitePool, id: &str) -> Result<bool, AppError> {
    let result = sqlx::query("DELETE FROM policy_bindings WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() > 0)
}

/// Check if the bootstrap state row exists (one-time import already done).
pub async fn has_bootstrapped(pool: &SqlitePool) -> Result<bool, AppError> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM policy_bootstrap_state WHERE id = 1")
        .fetch_one(pool)
        .await?;

    Ok(row.0 > 0)
}

/// Insert or replace the bootstrap state row, marking the import as done.
pub async fn mark_bootstrapped(pool: &SqlitePool, source: &str, now: &str) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT OR REPLACE INTO policy_bootstrap_state
            (id, bootstrap_source, bootstrap_version, bootstrapped_at)
        VALUES (1, ?, 1, ?)
        "#,
    )
    .bind(source)
    .bind(now)
    .execute(pool)
    .await?;

    Ok(())
}

/// Insert or update a cached room entry for the policy UI.
pub async fn upsert_cached_room(pool: &SqlitePool, room: &CachedRoom) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO policy_targets_cache
            (room_id, name, canonical_alias, parent_space_id, is_space, last_seen_at)
        VALUES (?, ?, ?, ?, ?, ?)
        ON CONFLICT(room_id) DO UPDATE SET
            name = excluded.name,
            canonical_alias = excluded.canonical_alias,
            parent_space_id = excluded.parent_space_id,
            is_space = excluded.is_space,
            last_seen_at = excluded.last_seen_at
        "#,
    )
    .bind(&room.room_id)
    .bind(&room.name)
    .bind(&room.canonical_alias)
    .bind(&room.parent_space_id)
    .bind(room.is_space)
    .bind(&room.last_seen_at)
    .execute(pool)
    .await?;

    Ok(())
}

/// List all cached rooms, ordered by name (nulls last).
pub async fn list_cached_rooms(pool: &SqlitePool) -> Result<Vec<CachedRoom>, AppError> {
    let rows: Vec<CachedRoomRow> = sqlx::query_as(
        r#"
        SELECT room_id, name, canonical_alias, parent_space_id, is_space, last_seen_at
        FROM policy_targets_cache
        ORDER BY COALESCE(name, 'zzz'), room_id
        "#,
    )
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    // ── Test helpers ────────────────────────────────────────────────

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    // ── Binding CRUD ────────────────────────────────────────────────

    #[tokio::test]
    async fn create_and_list_binding() {
        let pool = test_pool().await;

        create_binding(
            &pool,
            "b-1",
            "group",
            "staff",
            "room",
            "!abc:example.com",
            None,
            false,
            "2026-01-01T00:00:00Z",
        )
        .await
        .unwrap();

        let bindings = list_bindings(&pool).await.unwrap();
        assert_eq!(bindings.len(), 1);

        let b = &bindings[0];
        assert_eq!(b.id, "b-1");
        assert_eq!(b.subject, PolicySubject::Group("staff".to_string()));
        assert_eq!(b.target, PolicyTarget::Room("!abc:example.com".to_string()));
        assert_eq!(b.power_level, None);
        assert!(!b.allow_remove);
        assert_eq!(b.created_at, "2026-01-01T00:00:00Z");
        assert_eq!(b.updated_at, "2026-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn create_with_power_level_and_allow_remove() {
        let pool = test_pool().await;

        create_binding(
            &pool,
            "b-2",
            "role",
            "matrix-admin",
            "space",
            "!space:example.com",
            Some(100),
            true,
            "2026-02-01T00:00:00Z",
        )
        .await
        .unwrap();

        let bindings = list_bindings(&pool).await.unwrap();
        assert_eq!(bindings.len(), 1);

        let b = &bindings[0];
        assert_eq!(b.subject, PolicySubject::Role("matrix-admin".to_string()));
        assert_eq!(
            b.target,
            PolicyTarget::Space("!space:example.com".to_string())
        );
        assert_eq!(b.power_level, Some(100));
        assert!(b.allow_remove);
    }

    #[tokio::test]
    async fn update_binding_changes_fields() {
        let pool = test_pool().await;

        create_binding(
            &pool,
            "b-3",
            "group",
            "devs",
            "room",
            "!dev:example.com",
            None,
            false,
            "2026-01-01T00:00:00Z",
        )
        .await
        .unwrap();

        let updated = update_binding(&pool, "b-3", Some(50), true, "2026-03-01T00:00:00Z")
            .await
            .unwrap();
        assert!(updated);

        let bindings = list_bindings(&pool).await.unwrap();
        let b = &bindings[0];
        assert_eq!(b.power_level, Some(50));
        assert!(b.allow_remove);
        assert_eq!(b.updated_at, "2026-03-01T00:00:00Z");
        // created_at should not change
        assert_eq!(b.created_at, "2026-01-01T00:00:00Z");
    }

    #[tokio::test]
    async fn update_nonexistent_returns_false() {
        let pool = test_pool().await;

        let updated = update_binding(&pool, "no-such-id", Some(50), true, "2026-03-01T00:00:00Z")
            .await
            .unwrap();
        assert!(!updated);
    }

    #[tokio::test]
    async fn delete_binding_removes_row() {
        let pool = test_pool().await;

        create_binding(
            &pool,
            "b-del",
            "group",
            "staff",
            "room",
            "!rm:example.com",
            None,
            false,
            "2026-01-01T00:00:00Z",
        )
        .await
        .unwrap();

        let deleted = delete_binding(&pool, "b-del").await.unwrap();
        assert!(deleted);

        let bindings = list_bindings(&pool).await.unwrap();
        assert!(bindings.is_empty());
    }

    #[tokio::test]
    async fn delete_nonexistent_returns_false() {
        let pool = test_pool().await;

        let deleted = delete_binding(&pool, "no-such-id").await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn duplicate_binding_rejected() {
        let pool = test_pool().await;

        create_binding(
            &pool,
            "b-dup1",
            "group",
            "staff",
            "room",
            "!abc:example.com",
            None,
            false,
            "2026-01-01T00:00:00Z",
        )
        .await
        .unwrap();

        // Same (subject_type, subject_value, target_room_id) — should fail due to UNIQUE constraint
        let result = create_binding(
            &pool,
            "b-dup2",
            "group",
            "staff",
            "room",
            "!abc:example.com",
            Some(50),
            true,
            "2026-02-01T00:00:00Z",
        )
        .await;

        assert!(result.is_err());
    }

    // ── Bootstrap state ─────────────────────────────────────────────

    #[tokio::test]
    async fn bootstrap_state_tracks_import() {
        let pool = test_pool().await;

        assert!(!has_bootstrapped(&pool).await.unwrap());

        mark_bootstrapped(&pool, "env:GROUP_MAPPINGS", "2026-03-09T00:00:00Z")
            .await
            .unwrap();

        assert!(has_bootstrapped(&pool).await.unwrap());
    }

    // ── Cached rooms ────────────────────────────────────────────────

    #[tokio::test]
    async fn upsert_cached_room_inserts_and_updates() {
        let pool = test_pool().await;

        let room = CachedRoom {
            room_id: "!room1:example.com".to_string(),
            name: Some("General".to_string()),
            canonical_alias: Some("#general:example.com".to_string()),
            parent_space_id: None,
            is_space: false,
            last_seen_at: "2026-01-01T00:00:00Z".to_string(),
        };

        upsert_cached_room(&pool, &room).await.unwrap();

        let rooms = list_cached_rooms(&pool).await.unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].name.as_deref(), Some("General"));

        // Update the name
        let updated_room = CachedRoom {
            name: Some("General Chat".to_string()),
            last_seen_at: "2026-02-01T00:00:00Z".to_string(),
            ..room
        };

        upsert_cached_room(&pool, &updated_room).await.unwrap();

        let rooms = list_cached_rooms(&pool).await.unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].name.as_deref(), Some("General Chat"));
        assert_eq!(rooms[0].last_seen_at, "2026-02-01T00:00:00Z");
    }
}
