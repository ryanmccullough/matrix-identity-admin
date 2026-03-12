use sqlx::SqlitePool;

use crate::{error::AppError, models::audit::AuditLog};

/// Filter parameters for audit log queries.
#[derive(Default)]
pub struct AuditFilter<'a> {
    pub action: Option<&'a str>,
    pub result: Option<&'a str>,
    pub admin_username: Option<&'a str>,
    pub from: Option<&'a str>,
    pub to: Option<&'a str>,
}

/// Build a WHERE clause and bind values from the filter.
/// Returns (where_clause, bind_values) where where_clause includes "WHERE" if non-empty.
fn build_where_clause(filter: &AuditFilter<'_>) -> (String, Vec<String>) {
    let mut conditions = Vec::new();
    let mut values = Vec::new();

    if let Some(action) = filter.action {
        conditions.push("action = ?".to_string());
        values.push(action.to_string());
    }
    if let Some(result) = filter.result {
        conditions.push("result = ?".to_string());
        values.push(result.to_string());
    }
    if let Some(admin) = filter.admin_username {
        conditions.push("admin_username = ?".to_string());
        values.push(admin.to_string());
    }
    if let Some(from) = filter.from {
        conditions.push("timestamp >= ?".to_string());
        values.push(from.to_string());
    }
    if let Some(to) = filter.to {
        // Add 'T23:59:59Z' to make the date inclusive of the full day.
        conditions.push("timestamp <= ?".to_string());
        values.push(format!("{to}T23:59:59Z"));
    }

    if conditions.is_empty() {
        (String::new(), values)
    } else {
        (format!("WHERE {}", conditions.join(" AND ")), values)
    }
}

pub async fn insert(pool: &SqlitePool, log: &AuditLog) -> Result<(), AppError> {
    sqlx::query(
        r#"
        INSERT INTO audit_logs
            (id, timestamp, admin_subject, admin_username,
             target_keycloak_user_id, target_matrix_user_id,
             action, result, metadata_json)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
        "#,
    )
    .bind(&log.id)
    .bind(&log.timestamp)
    .bind(&log.admin_subject)
    .bind(&log.admin_username)
    .bind(&log.target_keycloak_user_id)
    .bind(&log.target_matrix_user_id)
    .bind(&log.action)
    .bind(&log.result)
    .bind(&log.metadata_json)
    .execute(pool)
    .await?;

    Ok(())
}

pub async fn recent(pool: &SqlitePool, limit: i64) -> Result<Vec<AuditLog>, AppError> {
    let rows: Vec<AuditLogRow> = sqlx::query_as::<_, AuditLogRow>(
        r#"
        SELECT id, timestamp, admin_subject, admin_username,
               target_keycloak_user_id, target_matrix_user_id,
               action, result, metadata_json
        FROM audit_logs
        ORDER BY timestamp DESC
        LIMIT ?
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn count(pool: &SqlitePool) -> Result<i64, AppError> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM audit_logs")
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

/// Count audit entries created within the last `since_seconds` seconds.
pub async fn recent_actions_count(pool: &SqlitePool, since_seconds: i64) -> Result<i64, AppError> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM audit_logs
        WHERE unixepoch(timestamp) > unixepoch('now') - ?
        "#,
    )
    .bind(since_seconds)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

pub async fn recent_page(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
) -> Result<Vec<AuditLog>, AppError> {
    let rows: Vec<AuditLogRow> = sqlx::query_as::<_, AuditLogRow>(
        r#"
        SELECT id, timestamp, admin_subject, admin_username,
               target_keycloak_user_id, target_matrix_user_id,
               action, result, metadata_json
        FROM audit_logs
        ORDER BY timestamp DESC
        LIMIT ? OFFSET ?
        "#,
    )
    .bind(limit)
    .bind(offset)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

pub async fn for_user(
    pool: &SqlitePool,
    keycloak_user_id: &str,
    limit: i64,
) -> Result<Vec<AuditLog>, AppError> {
    let rows: Vec<AuditLogRow> = sqlx::query_as::<_, AuditLogRow>(
        r#"
        SELECT id, timestamp, admin_subject, admin_username,
               target_keycloak_user_id, target_matrix_user_id,
               action, result, metadata_json
        FROM audit_logs
        WHERE target_keycloak_user_id = ?
        ORDER BY timestamp DESC
        LIMIT ?
        "#,
    )
    .bind(keycloak_user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows.into_iter().map(Into::into).collect())
}

/// Count audit entries matching the given filter.
pub async fn count_with_filter(
    pool: &SqlitePool,
    filter: &AuditFilter<'_>,
) -> Result<i64, AppError> {
    let (where_clause, values) = build_where_clause(filter);
    let sql = format!("SELECT COUNT(*) FROM audit_logs {where_clause}");
    let mut query = sqlx::query_as::<_, (i64,)>(&sql);
    for v in &values {
        query = query.bind(v);
    }
    let row = query.fetch_one(pool).await?;
    Ok(row.0)
}

/// Fetch a page of audit entries matching the given filter.
pub async fn page_with_filter(
    pool: &SqlitePool,
    filter: &AuditFilter<'_>,
    limit: i64,
    offset: i64,
) -> Result<Vec<AuditLog>, AppError> {
    let (where_clause, values) = build_where_clause(filter);
    let sql = format!(
        "SELECT id, timestamp, admin_subject, admin_username, \
         target_keycloak_user_id, target_matrix_user_id, \
         action, result, metadata_json \
         FROM audit_logs {where_clause} ORDER BY timestamp DESC LIMIT ? OFFSET ?"
    );
    let mut query = sqlx::query_as::<_, AuditLogRow>(&sql);
    for v in &values {
        query = query.bind(v);
    }
    query = query.bind(limit).bind(offset);
    let rows: Vec<AuditLogRow> = query.fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Fetch all matching rows (no pagination) for export.
pub async fn all_with_filter(
    pool: &SqlitePool,
    filter: &AuditFilter<'_>,
) -> Result<Vec<AuditLog>, AppError> {
    let (where_clause, values) = build_where_clause(filter);
    let sql = format!(
        "SELECT id, timestamp, admin_subject, admin_username, \
         target_keycloak_user_id, target_matrix_user_id, \
         action, result, metadata_json \
         FROM audit_logs {where_clause} ORDER BY timestamp DESC"
    );
    let mut query = sqlx::query_as::<_, AuditLogRow>(&sql);
    for v in &values {
        query = query.bind(v);
    }
    let rows: Vec<AuditLogRow> = query.fetch_all(pool).await?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Count audit entries matching any of the given actions within the last `since_seconds` seconds.
pub async fn count_actions_since(
    pool: &SqlitePool,
    actions: &[&str],
    since_seconds: i64,
) -> Result<i64, AppError> {
    if actions.is_empty() {
        return Ok(0);
    }
    let placeholders: Vec<&str> = actions.iter().map(|_| "?").collect();
    let sql = format!(
        "SELECT COUNT(*) FROM audit_logs WHERE action IN ({}) AND unixepoch(timestamp) > unixepoch('now') - ?",
        placeholders.join(", ")
    );
    let mut query = sqlx::query_as::<_, (i64,)>(&sql);
    for action in actions {
        query = query.bind(*action);
    }
    query = query.bind(since_seconds);
    let row = query.fetch_one(pool).await?;
    Ok(row.0)
}

/// Count audit entries with result = 'failure' within the last `since_seconds` seconds.
pub async fn count_failures_since(pool: &SqlitePool, since_seconds: i64) -> Result<i64, AppError> {
    let row: (i64,) = sqlx::query_as(
        r#"
        SELECT COUNT(*)
        FROM audit_logs
        WHERE result = 'failure' AND unixepoch(timestamp) > unixepoch('now') - ?
        "#,
    )
    .bind(since_seconds)
    .fetch_one(pool)
    .await?;
    Ok(row.0)
}

/// Internal flat struct matching the SQLite columns.
#[derive(sqlx::FromRow)]
struct AuditLogRow {
    id: String,
    timestamp: String,
    admin_subject: String,
    admin_username: String,
    target_keycloak_user_id: Option<String>,
    target_matrix_user_id: Option<String>,
    action: String,
    result: String,
    metadata_json: String,
}

impl From<AuditLogRow> for AuditLog {
    fn from(r: AuditLogRow) -> Self {
        AuditLog {
            id: r.id,
            timestamp: r.timestamp,
            admin_subject: r.admin_subject,
            admin_username: r.admin_username,
            target_keycloak_user_id: r.target_keycloak_user_id,
            target_matrix_user_id: r.target_matrix_user_id,
            action: r.action,
            result: r.result,
            metadata_json: r.metadata_json,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::audit::AuditLog;

    async fn setup_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        pool
    }

    fn make_log(
        id: &str,
        timestamp: &str,
        action: &str,
        keycloak_user_id: Option<&str>,
    ) -> AuditLog {
        AuditLog {
            id: id.to_string(),
            timestamp: timestamp.to_string(),
            admin_subject: "admin-subject".to_string(),
            admin_username: "admin".to_string(),
            target_keycloak_user_id: keycloak_user_id.map(str::to_string),
            target_matrix_user_id: None,
            action: action.to_string(),
            result: "success".to_string(),
            metadata_json: "{}".to_string(),
        }
    }

    #[tokio::test]
    async fn insert_and_retrieve_via_recent() {
        let pool = setup_db().await;
        let log = make_log(
            "log-1",
            "2024-01-01T00:00:00Z",
            "test_action",
            Some("kc-001"),
        );

        insert(&pool, &log).await.unwrap();

        let results = recent(&pool, 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "log-1");
        assert_eq!(results[0].action, "test_action");
        assert_eq!(results[0].admin_username, "admin");
        assert_eq!(
            results[0].target_keycloak_user_id.as_deref(),
            Some("kc-001")
        );
    }

    #[tokio::test]
    async fn recent_respects_limit() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log("a", "2024-01-01T00:00:01Z", "action_a", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("b", "2024-01-01T00:00:02Z", "action_b", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("c", "2024-01-01T00:00:03Z", "action_c", None),
        )
        .await
        .unwrap();

        let results = recent(&pool, 2).await.unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn recent_returns_newest_first() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log("old", "2024-01-01T00:00:00Z", "old_action", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("new", "2024-06-01T00:00:00Z", "new_action", None),
        )
        .await
        .unwrap();

        let results = recent(&pool, 10).await.unwrap();
        assert_eq!(results[0].id, "new");
        assert_eq!(results[1].id, "old");
    }

    #[tokio::test]
    async fn for_user_filters_by_keycloak_id() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log("1", "2024-01-01T00:00:00Z", "action_a", Some("kc-alice")),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("2", "2024-01-01T00:00:01Z", "action_b", Some("kc-bob")),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("3", "2024-01-01T00:00:02Z", "action_c", Some("kc-alice")),
        )
        .await
        .unwrap();

        let alice = for_user(&pool, "kc-alice", 10).await.unwrap();
        assert_eq!(alice.len(), 2);
        assert!(alice
            .iter()
            .all(|l| l.target_keycloak_user_id.as_deref() == Some("kc-alice")));

        let bob = for_user(&pool, "kc-bob", 10).await.unwrap();
        assert_eq!(bob.len(), 1);
        assert_eq!(bob[0].id, "2");
    }

    #[tokio::test]
    async fn for_user_respects_limit() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log("1", "2024-01-01T00:00:00Z", "action_a", Some("kc-001")),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("2", "2024-01-01T00:00:01Z", "action_b", Some("kc-001")),
        )
        .await
        .unwrap();

        let results = for_user(&pool, "kc-001", 1).await.unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn nullable_target_fields_round_trip() {
        let pool = setup_db().await;
        let log = AuditLog {
            id: "null-test".to_string(),
            timestamp: "2024-01-01T00:00:00Z".to_string(),
            admin_subject: "subj".to_string(),
            admin_username: "admin".to_string(),
            target_keycloak_user_id: None,
            target_matrix_user_id: None,
            action: "global_action".to_string(),
            result: "success".to_string(),
            metadata_json: r#"{"key":"value"}"#.to_string(),
        };

        insert(&pool, &log).await.unwrap();

        let results = recent(&pool, 1).await.unwrap();
        assert_eq!(results[0].target_keycloak_user_id, None);
        assert_eq!(results[0].target_matrix_user_id, None);
        assert_eq!(results[0].metadata_json, r#"{"key":"value"}"#);
    }

    #[tokio::test]
    async fn empty_db_returns_empty_vecs() {
        let pool = setup_db().await;
        assert!(recent(&pool, 10).await.unwrap().is_empty());
        assert!(for_user(&pool, "kc-001", 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn count_returns_total_entries() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log("1", "2024-01-01T00:00:01Z", "action_a", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("2", "2024-01-01T00:00:02Z", "action_b", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("3", "2024-01-01T00:00:03Z", "action_c", None),
        )
        .await
        .unwrap();
        assert_eq!(count(&pool).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn count_returns_zero_on_empty_db() {
        let pool = setup_db().await;
        assert_eq!(count(&pool).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn recent_page_with_offset() {
        let pool = setup_db().await;
        // Insert newest-last so DESC ordering gives: 3, 2, 1
        insert(
            &pool,
            &make_log("1", "2024-01-01T00:00:01Z", "action_a", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("2", "2024-01-01T00:00:02Z", "action_b", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("3", "2024-01-01T00:00:03Z", "action_c", None),
        )
        .await
        .unwrap();

        // Limit 2, offset 1 → should skip the newest and return the 2nd and 3rd newest.
        let page = recent_page(&pool, 2, 1).await.unwrap();
        assert_eq!(page.len(), 2);
        assert_eq!(page[0].id, "2");
        assert_eq!(page[1].id, "1");
    }

    #[tokio::test]
    async fn recent_page_no_offset() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log("1", "2024-01-01T00:00:01Z", "action_a", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("2", "2024-01-01T00:00:02Z", "action_b", None),
        )
        .await
        .unwrap();

        let page = recent_page(&pool, 10, 0).await.unwrap();
        assert_eq!(page.len(), 2);
        // Newest first.
        assert_eq!(page[0].id, "2");
        assert_eq!(page[1].id, "1");
    }

    fn make_log_with_result(id: &str, timestamp: &str, action: &str, result: &str) -> AuditLog {
        AuditLog {
            id: id.to_string(),
            timestamp: timestamp.to_string(),
            admin_subject: "admin-subject".to_string(),
            admin_username: "admin".to_string(),
            target_keycloak_user_id: None,
            target_matrix_user_id: None,
            action: action.to_string(),
            result: result.to_string(),
            metadata_json: "{}".to_string(),
        }
    }

    // ── AuditFilter-based query tests ──────────────────────────────────────

    #[tokio::test]
    async fn count_with_filter_no_filters() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log("1", "2024-01-01T00:00:00Z", "invite_user", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("2", "2024-01-02T00:00:00Z", "revoke_session", None),
        )
        .await
        .unwrap();
        let filter = AuditFilter::default();
        assert_eq!(count_with_filter(&pool, &filter).await.unwrap(), 2);
    }

    #[tokio::test]
    async fn count_with_filter_by_action() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log("1", "2024-01-01T00:00:00Z", "invite_user", None),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log("2", "2024-01-02T00:00:00Z", "revoke_session", None),
        )
        .await
        .unwrap();
        let filter = AuditFilter {
            action: Some("invite_user"),
            ..Default::default()
        };
        assert_eq!(count_with_filter(&pool, &filter).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn page_with_filter_date_range() {
        let pool = setup_db().await;
        insert(&pool, &make_log("1", "2024-01-01T00:00:00Z", "a", None))
            .await
            .unwrap();
        insert(&pool, &make_log("2", "2024-01-15T00:00:00Z", "b", None))
            .await
            .unwrap();
        insert(&pool, &make_log("3", "2024-02-01T00:00:00Z", "c", None))
            .await
            .unwrap();
        let filter = AuditFilter {
            from: Some("2024-01-10"),
            to: Some("2024-01-31"),
            ..Default::default()
        };
        let rows = page_with_filter(&pool, &filter, 100, 0).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "2");
    }

    #[tokio::test]
    async fn page_with_filter_by_admin() {
        let pool = setup_db().await;
        let log1 = AuditLog {
            admin_username: "alice".to_string(),
            ..make_log("1", "2024-01-01T00:00:00Z", "invite_user", None)
        };
        let log2 = AuditLog {
            admin_username: "bob".to_string(),
            ..make_log("2", "2024-01-02T00:00:00Z", "invite_user", None)
        };
        insert(&pool, &log1).await.unwrap();
        insert(&pool, &log2).await.unwrap();
        let filter = AuditFilter {
            admin_username: Some("alice"),
            ..Default::default()
        };
        let rows = page_with_filter(&pool, &filter, 100, 0).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].admin_username, "alice");
    }

    #[tokio::test]
    async fn all_with_filter_returns_all_rows() {
        let pool = setup_db().await;
        for i in 0..5 {
            insert(
                &pool,
                &make_log(
                    &format!("{i}"),
                    &format!("2024-01-0{}T00:00:00Z", i + 1),
                    "a",
                    None,
                ),
            )
            .await
            .unwrap();
        }
        let filter = AuditFilter::default();
        let rows = all_with_filter(&pool, &filter).await.unwrap();
        assert_eq!(rows.len(), 5);
    }

    #[tokio::test]
    async fn combined_filters_narrow_results() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log_with_result("1", "2024-01-01T00:00:00Z", "invite_user", "success"),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log_with_result("2", "2024-01-15T00:00:00Z", "invite_user", "failure"),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log_with_result("3", "2024-02-01T00:00:00Z", "invite_user", "success"),
        )
        .await
        .unwrap();
        let filter = AuditFilter {
            action: Some("invite_user"),
            result: Some("success"),
            from: Some("2024-01-01"),
            to: Some("2024-01-31"),
            ..Default::default()
        };
        assert_eq!(count_with_filter(&pool, &filter).await.unwrap(), 1);
    }

    #[tokio::test]
    async fn recent_actions_count_filters_by_bound_interval() {
        let pool = setup_db().await;

        sqlx::query(
            r#"
            INSERT INTO audit_logs
                (id, timestamp, admin_subject, admin_username,
                 target_keycloak_user_id, target_matrix_user_id,
                 action, result, metadata_json)
            VALUES
                ('recent', datetime('now', '-30 seconds'), 'sub', 'admin', NULL, NULL, 'invite_user', 'success', '{}'),
                ('old', datetime('now', '-2 hours'), 'sub', 'admin', NULL, NULL, 'invite_user', 'success', '{}')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(recent_actions_count(&pool, 60).await.unwrap(), 1);
        assert_eq!(recent_actions_count(&pool, 60 * 60 * 3).await.unwrap(), 2);
    }

    // ── count_actions_since / count_failures_since ─────────────────────────

    #[tokio::test]
    async fn count_actions_since_filters_by_action_and_time() {
        let pool = setup_db().await;
        sqlx::query(
            r#"
            INSERT INTO audit_logs
                (id, timestamp, admin_subject, admin_username,
                 target_keycloak_user_id, target_matrix_user_id,
                 action, result, metadata_json)
            VALUES
                ('1', datetime('now', '-30 seconds'), 'sub', 'admin', NULL, NULL, 'invite_user', 'success', '{}'),
                ('2', datetime('now', '-30 seconds'), 'sub', 'admin', NULL, NULL, 'disable_identity_account_on_disable', 'success', '{}'),
                ('3', datetime('now', '-2 hours'), 'sub', 'admin', NULL, NULL, 'invite_user', 'success', '{}')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(
            count_actions_since(&pool, &["invite_user"], 60)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            count_actions_since(
                &pool,
                &["invite_user", "disable_identity_account_on_disable"],
                60
            )
            .await
            .unwrap(),
            2
        );
        assert_eq!(
            count_actions_since(&pool, &["invite_user"], 60 * 60 * 3)
                .await
                .unwrap(),
            2
        );
    }

    #[tokio::test]
    async fn count_actions_since_returns_zero_on_empty_db() {
        let pool = setup_db().await;
        assert_eq!(
            count_actions_since(&pool, &["invite_user"], 86400)
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn count_failures_since_counts_only_failures() {
        let pool = setup_db().await;
        sqlx::query(
            r#"
            INSERT INTO audit_logs
                (id, timestamp, admin_subject, admin_username,
                 target_keycloak_user_id, target_matrix_user_id,
                 action, result, metadata_json)
            VALUES
                ('1', datetime('now', '-30 seconds'), 'sub', 'admin', NULL, NULL, 'invite_user', 'success', '{}'),
                ('2', datetime('now', '-30 seconds'), 'sub', 'admin', NULL, NULL, 'invite_user', 'failure', '{}'),
                ('3', datetime('now', '-2 hours'), 'sub', 'admin', NULL, NULL, 'invite_user', 'failure', '{}')
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();

        assert_eq!(count_failures_since(&pool, 60).await.unwrap(), 1);
        assert_eq!(count_failures_since(&pool, 60 * 60 * 3).await.unwrap(), 2);
    }
}
