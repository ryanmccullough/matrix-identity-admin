use sqlx::SqlitePool;

use crate::{
    clients::MatrixService,
    db::policy as policy_db,
    error::AppError,
    models::policy_binding::{CachedRoom, PolicyBinding, PolicySubject, PolicyTarget},
    services::audit_service::AuditService,
};

/// Service layer for policy binding CRUD, effective binding resolution,
/// room cache refresh, and one-time bootstrap from legacy GROUP_MAPPINGS.
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

    /// Create a new policy binding and write an audit log entry.
    #[allow(clippy::too_many_arguments)]
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
        let id = uuid::Uuid::new_v4().to_string();
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

    /// Update an existing binding's power_level and allow_remove fields.
    /// Returns `true` if the row existed and was updated.
    #[allow(clippy::too_many_arguments)]
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
        let updated =
            policy_db::update_binding(&self.pool, id, power_level, allow_remove, &now).await?;

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

    /// Delete a policy binding by id and write an audit log entry on success.
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

    /// Resolve effective bindings for a user given their groups and roles.
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

    /// Refresh the room cache from Synapse, upserting each room's metadata.
    /// Returns the number of rooms cached.
    pub async fn refresh_room_cache(&self, synapse: &dyn MatrixService) -> Result<usize, AppError> {
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

    /// List all cached rooms.
    pub async fn list_cached_rooms(&self) -> Result<Vec<CachedRoom>, AppError> {
        policy_db::list_cached_rooms(&self.pool).await
    }

    /// Bootstrap policy bindings from legacy GROUP_MAPPINGS if not already done.
    /// Returns the number of bindings created (0 if already bootstrapped).
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
            let id = uuid::Uuid::new_v4().to_string();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::group_mapping::GroupMapping;
    use sqlx::sqlite::SqlitePoolOptions;

    // ── Test helpers ────────────────────────────────────────────────

    async fn test_service() -> (PolicyService, AuditService) {
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        let audit = AuditService::new(pool.clone());
        (PolicyService::new(pool), audit)
    }

    // ── CRUD ────────────────────────────────────────────────────────

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
    async fn update_binding_changes_fields() {
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

        let updated = svc
            .update_binding(&binding.id, Some(50), true, &audit, "sub", "admin")
            .await
            .unwrap();
        assert!(updated);

        let all = svc.list_bindings().await.unwrap();
        assert_eq!(all[0].power_level, Some(50));
        assert!(all[0].allow_remove);
    }

    #[tokio::test]
    async fn delete_binding_works() {
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

        let deleted = svc
            .delete_binding(&binding.id, &audit, "sub", "admin")
            .await
            .unwrap();
        assert!(deleted);
        assert!(svc.list_bindings().await.unwrap().is_empty());
    }

    // ── Effective binding resolution ────────────────────────────────

    #[tokio::test]
    async fn effective_bindings_filters_by_group_and_role() {
        let (svc, audit) = test_service().await;
        svc.create_binding(
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
        svc.create_binding(
            &PolicySubject::Role("matrix-admin".into()),
            &PolicyTarget::Room("!room2:test.com".into()),
            Some(100),
            false,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();
        svc.create_binding(
            &PolicySubject::Group("contractors".into()),
            &PolicyTarget::Room("!room3:test.com".into()),
            None,
            false,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        let all = svc.list_bindings().await.unwrap();
        let effective =
            svc.effective_bindings_for_user(&all, &["staff".into()], &["matrix-admin".into()]);

        assert_eq!(effective.len(), 2);
    }

    #[tokio::test]
    async fn effective_bindings_empty_when_no_match() {
        let (svc, audit) = test_service().await;
        svc.create_binding(
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

        let all = svc.list_bindings().await.unwrap();
        let effective = svc.effective_bindings_for_user(&all, &["other".into()], &[]);
        assert!(effective.is_empty());
    }

    // ── Bootstrap ───────────────────────────────────────────────────

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

        // Second call is a no-op.
        let count2 = svc.bootstrap_from_env(&mappings, "env").await.unwrap();
        assert_eq!(count2, 0);

        let all = svc.list_bindings().await.unwrap();
        assert_eq!(all.len(), 2);
    }

    // ── Audit logging ───────────────────────────────────────────────

    #[tokio::test]
    async fn create_binding_writes_audit_log() {
        let (svc, audit) = test_service().await;
        svc.create_binding(
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

        let logs = audit.recent(10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "create_policy_binding");
    }

    #[tokio::test]
    async fn delete_binding_writes_audit_log() {
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

        svc.delete_binding(&binding.id, &audit, "sub", "admin")
            .await
            .unwrap();

        let logs = audit.recent(10).await.unwrap();
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().any(|l| l.action == "delete_policy_binding"));
    }

    #[tokio::test]
    async fn update_binding_writes_audit_log() {
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

        svc.update_binding(&binding.id, Some(50), true, &audit, "sub", "admin")
            .await
            .unwrap();

        let logs = audit.recent(10).await.unwrap();
        assert_eq!(logs.len(), 2);
        assert!(logs.iter().any(|l| l.action == "update_policy_binding"));
    }
}
