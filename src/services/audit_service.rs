use serde_json::Value;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{
    db,
    error::AppError,
    models::audit::{AuditLog, AuditResult},
};

pub struct AuditService {
    pool: SqlitePool,
}

impl AuditService {
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn log(
        &self,
        admin_subject: &str,
        admin_username: &str,
        target_keycloak_user_id: Option<&str>,
        target_matrix_user_id: Option<&str>,
        action: &str,
        result: AuditResult,
        metadata: Value,
    ) -> Result<(), AppError> {
        let entry = AuditLog {
            id: Uuid::new_v4().to_string(),
            timestamp: chrono_now(),
            admin_subject: admin_subject.to_string(),
            admin_username: admin_username.to_string(),
            target_keycloak_user_id: target_keycloak_user_id.map(str::to_string),
            target_matrix_user_id: target_matrix_user_id.map(str::to_string),
            action: action.to_string(),
            result: result.to_string(),
            metadata_json: metadata.to_string(),
        };

        db::audit::insert(&self.pool, &entry).await
    }

    pub async fn recent(&self, limit: i64) -> Result<Vec<AuditLog>, AppError> {
        db::audit::recent(&self.pool, limit).await
    }

    pub async fn count(&self) -> Result<i64, AppError> {
        db::audit::count(&self.pool).await
    }

    pub async fn recent_page(&self, limit: i64, offset: i64) -> Result<Vec<AuditLog>, AppError> {
        db::audit::recent_page(&self.pool, limit, offset).await
    }

    /// Count audit entries matching the given filter.
    pub async fn count_with_filter(
        &self,
        filter: &db::audit::AuditFilter<'_>,
    ) -> Result<i64, AppError> {
        db::audit::count_with_filter(&self.pool, filter).await
    }

    /// Fetch a page of audit entries matching the given filter.
    pub async fn page_with_filter(
        &self,
        filter: &db::audit::AuditFilter<'_>,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<AuditLog>, AppError> {
        db::audit::page_with_filter(&self.pool, filter, limit, offset).await
    }

    /// Fetch all matching rows (no pagination) for export.
    pub async fn all_with_filter(
        &self,
        filter: &db::audit::AuditFilter<'_>,
    ) -> Result<Vec<AuditLog>, AppError> {
        db::audit::all_with_filter(&self.pool, filter).await
    }

    pub async fn for_user(
        &self,
        keycloak_user_id: &str,
        limit: i64,
    ) -> Result<Vec<AuditLog>, AppError> {
        db::audit::for_user(&self.pool, keycloak_user_id, limit).await
    }

    /// Count audit entries created within the last `since_seconds` seconds.
    pub async fn recent_actions_count(&self, since_seconds: i64) -> Result<i64, AppError> {
        db::audit::recent_actions_count(&self.pool, since_seconds).await
    }

    /// Count audit entries matching any of the given actions within the last `since_seconds` seconds.
    pub async fn count_actions_since(
        &self,
        actions: &[&str],
        since_seconds: i64,
    ) -> Result<i64, AppError> {
        db::audit::count_actions_since(&self.pool, actions, since_seconds).await
    }

    /// Count audit entries with result = 'failure' within the last `since_seconds` seconds.
    pub async fn count_failures_since(&self, since_seconds: i64) -> Result<i64, AppError> {
        db::audit::count_failures_since(&self.pool, since_seconds).await
    }
}

/// ISO 8601 UTC timestamp for the current moment.
/// Uses only std so we don't need an extra time crate dependency here.
fn chrono_now() -> String {
    // We store as text; format matches SQLite indexing expectations.
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    // Simple fixed-point formatting: YYYY-MM-DDThh:mm:ssZ
    let s = secs;
    let sec = s % 60;
    let min = (s / 60) % 60;
    let hour = (s / 3600) % 24;
    let days = s / 86400; // days since epoch

    // Days → date via Gregorian calendar algorithm.
    let (year, month, day) = days_to_ymd(days);

    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Rata Die algorithm for Unix epoch (1970-01-01).
    let z = days + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn setup_service() -> AuditService {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("failed to open in-memory SQLite");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("failed to run migrations");
        AuditService::new(pool)
    }

    #[tokio::test]
    async fn count_returns_total() {
        let svc = setup_service().await;
        svc.log(
            "s",
            "a",
            None,
            None,
            "action_1",
            AuditResult::Success,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        svc.log(
            "s",
            "a",
            None,
            None,
            "action_2",
            AuditResult::Success,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        svc.log(
            "s",
            "a",
            None,
            None,
            "action_3",
            AuditResult::Failure,
            serde_json::json!({}),
        )
        .await
        .unwrap();
        assert_eq!(svc.count().await.unwrap(), 3);
    }

    #[tokio::test]
    async fn recent_page_works() {
        let svc = setup_service().await;
        for i in 0..5u32 {
            svc.log(
                "s",
                "a",
                None,
                None,
                &format!("action_{i}"),
                AuditResult::Success,
                serde_json::json!({}),
            )
            .await
            .unwrap();
        }
        // Fetch page 2 (offset 2, limit 2): should return 2 entries.
        let page = svc.recent_page(2, 2).await.unwrap();
        assert_eq!(page.len(), 2);
    }

    #[test]
    fn unix_epoch_is_1970_01_01() {
        assert_eq!(days_to_ymd(0), (1970, 1, 1));
    }

    #[test]
    fn known_date_2024_01_01() {
        // 2024-01-01 00:00:00 UTC = 1704067200 s → 19723 days
        assert_eq!(days_to_ymd(19723), (2024, 1, 1));
    }

    #[test]
    fn leap_day_2000_02_29() {
        // 2000-02-29 00:00:00 UTC = 951782400 s → 11016 days
        assert_eq!(days_to_ymd(11016), (2000, 2, 29));
    }

    #[test]
    fn day_after_leap_day_is_march_1() {
        assert_eq!(days_to_ymd(11017), (2000, 3, 1));
    }

    #[test]
    fn chrono_now_looks_like_iso8601() {
        let ts = chrono_now();
        // Expected format: YYYY-MM-DDThh:mm:ssZ  (20 chars)
        assert_eq!(ts.len(), 20, "unexpected timestamp length: {ts}");
        assert!(ts.ends_with('Z'), "timestamp missing Z suffix: {ts}");
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
    }
}
