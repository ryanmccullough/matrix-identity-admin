use sqlx::SqlitePool;

use crate::{error::AppError, models::audit::AuditLog};

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

pub async fn count_filtered(
    pool: &SqlitePool,
    action: Option<&str>,
    result: Option<&str>,
) -> Result<i64, AppError> {
    let row: (i64,) = match (action, result) {
        (Some(a), Some(r)) => {
            sqlx::query_as("SELECT COUNT(*) FROM audit_logs WHERE action = ? AND result = ?")
                .bind(a)
                .bind(r)
                .fetch_one(pool)
                .await?
        }
        (Some(a), None) => {
            sqlx::query_as("SELECT COUNT(*) FROM audit_logs WHERE action = ?")
                .bind(a)
                .fetch_one(pool)
                .await?
        }
        (None, Some(r)) => {
            sqlx::query_as("SELECT COUNT(*) FROM audit_logs WHERE result = ?")
                .bind(r)
                .fetch_one(pool)
                .await?
        }
        (None, None) => {
            sqlx::query_as("SELECT COUNT(*) FROM audit_logs")
                .fetch_one(pool)
                .await?
        }
    };
    Ok(row.0)
}

pub async fn recent_page_filtered(
    pool: &SqlitePool,
    limit: i64,
    offset: i64,
    action: Option<&str>,
    result: Option<&str>,
) -> Result<Vec<AuditLog>, AppError> {
    let rows: Vec<AuditLogRow> = match (action, result) {
        (Some(a), Some(r)) => {
            sqlx::query_as::<_, AuditLogRow>(
                r#"
                SELECT id, timestamp, admin_subject, admin_username,
                       target_keycloak_user_id, target_matrix_user_id,
                       action, result, metadata_json
                FROM audit_logs
                WHERE action = ? AND result = ?
                ORDER BY timestamp DESC
                LIMIT ? OFFSET ?
                "#,
            )
            .bind(a)
            .bind(r)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        (Some(a), None) => {
            sqlx::query_as::<_, AuditLogRow>(
                r#"
                SELECT id, timestamp, admin_subject, admin_username,
                       target_keycloak_user_id, target_matrix_user_id,
                       action, result, metadata_json
                FROM audit_logs
                WHERE action = ?
                ORDER BY timestamp DESC
                LIMIT ? OFFSET ?
                "#,
            )
            .bind(a)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        (None, Some(r)) => {
            sqlx::query_as::<_, AuditLogRow>(
                r#"
                SELECT id, timestamp, admin_subject, admin_username,
                       target_keycloak_user_id, target_matrix_user_id,
                       action, result, metadata_json
                FROM audit_logs
                WHERE result = ?
                ORDER BY timestamp DESC
                LIMIT ? OFFSET ?
                "#,
            )
            .bind(r)
            .bind(limit)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        (None, None) => {
            sqlx::query_as::<_, AuditLogRow>(
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
            .await?
        }
    };

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

    #[tokio::test]
    async fn count_filtered_by_action() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log_with_result("1", "2024-01-01T00:00:00Z", "invite_user", "success"),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log_with_result("2", "2024-01-01T00:00:01Z", "revoke_session", "success"),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log_with_result("3", "2024-01-01T00:00:02Z", "invite_user", "failure"),
        )
        .await
        .unwrap();

        assert_eq!(
            count_filtered(&pool, Some("invite_user"), None)
                .await
                .unwrap(),
            2
        );
        assert_eq!(
            count_filtered(&pool, Some("revoke_session"), None)
                .await
                .unwrap(),
            1
        );
        assert_eq!(count_filtered(&pool, None, None).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn count_filtered_by_result() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log_with_result("1", "2024-01-01T00:00:00Z", "invite_user", "success"),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log_with_result("2", "2024-01-01T00:00:01Z", "invite_user", "failure"),
        )
        .await
        .unwrap();

        assert_eq!(
            count_filtered(&pool, None, Some("success")).await.unwrap(),
            1
        );
        assert_eq!(
            count_filtered(&pool, None, Some("failure")).await.unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn count_filtered_by_action_and_result() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log_with_result("1", "2024-01-01T00:00:00Z", "invite_user", "success"),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log_with_result("2", "2024-01-01T00:00:01Z", "invite_user", "failure"),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log_with_result("3", "2024-01-01T00:00:02Z", "revoke_session", "success"),
        )
        .await
        .unwrap();

        assert_eq!(
            count_filtered(&pool, Some("invite_user"), Some("success"))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            count_filtered(&pool, Some("invite_user"), Some("failure"))
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            count_filtered(&pool, Some("revoke_session"), Some("failure"))
                .await
                .unwrap(),
            0
        );
    }

    #[tokio::test]
    async fn recent_page_filtered_returns_correct_rows() {
        let pool = setup_db().await;
        insert(
            &pool,
            &make_log_with_result("1", "2024-01-01T00:00:00Z", "invite_user", "success"),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log_with_result("2", "2024-01-01T00:00:01Z", "revoke_session", "success"),
        )
        .await
        .unwrap();
        insert(
            &pool,
            &make_log_with_result("3", "2024-01-01T00:00:02Z", "invite_user", "failure"),
        )
        .await
        .unwrap();

        let rows = recent_page_filtered(&pool, 10, 0, Some("invite_user"), None)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r.action == "invite_user"));

        let rows = recent_page_filtered(&pool, 10, 0, Some("invite_user"), Some("success"))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "1");

        let rows = recent_page_filtered(&pool, 10, 0, None, Some("failure"))
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "3");
    }
}
