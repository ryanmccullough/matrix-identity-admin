use crate::{
    clients::SynapseApi,
    error::AppError,
    models::{audit::AuditResult, group_mapping::GroupMapping, workflow::WorkflowOutcome},
    services::audit_service::AuditService,
};

/// Reconcile a single user's Matrix room membership against their Keycloak
/// group membership, using the provided group → room policy.
///
/// For each mapping:
/// - If the user is in the Keycloak group but not the room → force-join.
/// - If `remove_from_rooms` is true and the user is in the room but not the
///   group → kick.
///
/// Per-room failures are non-fatal: they are collected as warnings in the
/// returned `WorkflowOutcome` and the workflow continues to the next room.
#[allow(clippy::too_many_arguments)]
pub async fn reconcile_membership(
    keycloak_id: &str,
    matrix_user_id: &str,
    group_mappings: &[GroupMapping],
    keycloak_groups: &[String],
    synapse: &dyn SynapseApi,
    audit: &AuditService,
    actor_subject: &str,
    actor_username: &str,
    remove_from_rooms: bool,
) -> Result<WorkflowOutcome, AppError> {
    let mut outcome = WorkflowOutcome::ok();

    for mapping in group_mappings {
        let in_group = keycloak_groups.contains(&mapping.keycloak_group);

        let members = match synapse
            .get_joined_room_members(&mapping.matrix_room_id)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                outcome.add_warning(format!(
                    "Could not fetch members of {}: {}",
                    mapping.matrix_room_id, e
                ));
                continue;
            }
        };

        let in_room = members.contains(&matrix_user_id.to_string());

        if in_group && !in_room {
            let result = synapse
                .force_join_user(matrix_user_id, &mapping.matrix_room_id)
                .await;
            let audit_result = if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            };
            // NOTE: Audit failures are intentionally non-fatal here. Per-room reconciliation
            // already surfaces partial failures via WorkflowOutcome warnings; losing an
            // audit entry is less harmful than aborting the entire reconciliation run.
            let _ = audit
                .log(
                    actor_subject,
                    actor_username,
                    Some(keycloak_id),
                    Some(matrix_user_id),
                    "join_room_on_reconcile",
                    audit_result,
                    serde_json::json!({
                        "room_id": mapping.matrix_room_id,
                        "keycloak_group": mapping.keycloak_group,
                    }),
                )
                .await;
            if let Err(e) = result {
                outcome.add_warning(format!(
                    "Could not join {} to {}: {}",
                    matrix_user_id, mapping.matrix_room_id, e
                ));
            }
        } else if remove_from_rooms && !in_group && in_room {
            let result = synapse
                .kick_user_from_room(
                    matrix_user_id,
                    &mapping.matrix_room_id,
                    "Removed from Keycloak group",
                )
                .await;
            let audit_result = if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            };
            // NOTE: Audit failures are intentionally non-fatal here. Per-room reconciliation
            // already surfaces partial failures via WorkflowOutcome warnings; losing an
            // audit entry is less harmful than aborting the entire reconciliation run.
            let _ = audit
                .log(
                    actor_subject,
                    actor_username,
                    Some(keycloak_id),
                    Some(matrix_user_id),
                    "kick_room_on_reconcile",
                    audit_result,
                    serde_json::json!({
                        "room_id": mapping.matrix_room_id,
                        "keycloak_group": mapping.keycloak_group,
                    }),
                )
                .await;
            if let Err(e) = result {
                outcome.add_warning(format!(
                    "Could not kick {} from {}: {}",
                    matrix_user_id, mapping.matrix_room_id, e
                ));
            }
        }
    }

    Ok(outcome)
}

/// A single room action in a preview (join, kick, or already-correct).
#[derive(Debug)]
pub struct RoomAction {
    pub room_id: String,
    pub keycloak_group: String,
}

/// The result of a dry-run preview of `reconcile_membership`.
///
/// Contains what would happen if reconciliation were executed — without
/// making any changes. No audit entries are written.
#[derive(Debug, Default)]
pub struct ReconcilePreview {
    pub joins: Vec<RoomAction>,
    pub kicks: Vec<RoomAction>,
    pub already_correct: Vec<RoomAction>,
    pub warnings: Vec<String>,
}

/// Compute what `reconcile_membership` would do, without executing any changes.
///
/// Reads current room membership from Synapse and compares against group policy.
/// Returns a `ReconcilePreview` describing joins, kicks, and rooms already correct.
/// Member-fetch failures are non-fatal: recorded in `warnings`, room is skipped.
pub async fn preview_membership(
    matrix_user_id: &str,
    group_mappings: &[GroupMapping],
    keycloak_groups: &[String],
    synapse: &dyn SynapseApi,
    remove_from_rooms: bool,
) -> Result<ReconcilePreview, AppError> {
    let mut preview = ReconcilePreview::default();

    for mapping in group_mappings {
        let in_group = keycloak_groups.contains(&mapping.keycloak_group);

        let members = match synapse
            .get_joined_room_members(&mapping.matrix_room_id)
            .await
        {
            Ok(m) => m,
            Err(e) => {
                preview.warnings.push(format!(
                    "Could not fetch members of {}: {}",
                    mapping.matrix_room_id, e
                ));
                continue;
            }
        };

        let in_room = members.contains(&matrix_user_id.to_string());
        let action = RoomAction {
            room_id: mapping.matrix_room_id.clone(),
            keycloak_group: mapping.keycloak_group.clone(),
        };

        if in_group && !in_room {
            preview.joins.push(action);
        } else if remove_from_rooms && !in_group && in_room {
            preview.kicks.push(action);
        } else if in_group && in_room {
            preview.already_correct.push(action);
        }
    }

    Ok(preview)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::{
        models::{
            group_mapping::GroupMapping,
            synapse::{SynapseDevice, SynapseUser},
        },
        services::audit_service::AuditService,
    };

    // ── Mock Synapse ─────────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockSynapse {
        /// Members already in the room.
        pub members: Vec<String>,
        pub fail_get_members: bool,
        pub fail_force_join: bool,
        pub fail_kick: bool,
        /// Track calls.
        pub joined: std::sync::Mutex<Vec<String>>,
        pub kicked: std::sync::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl SynapseApi for MockSynapse {
        async fn get_user(&self, _: &str) -> Result<Option<SynapseUser>, AppError> {
            unimplemented!()
        }
        async fn list_devices(&self, _: &str) -> Result<Vec<SynapseDevice>, AppError> {
            unimplemented!()
        }
        async fn delete_device(&self, _: &str, _: &str) -> Result<(), AppError> {
            unimplemented!()
        }

        async fn get_joined_room_members(&self, _room_id: &str) -> Result<Vec<String>, AppError> {
            if self.fail_get_members {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock member fetch failure".into(),
                });
            }
            Ok(self.members.clone())
        }

        async fn force_join_user(&self, user_id: &str, _room_id: &str) -> Result<(), AppError> {
            if self.fail_force_join {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock force_join failure".into(),
                });
            }
            self.joined.lock().unwrap().push(user_id.to_string());
            Ok(())
        }

        async fn kick_user_from_room(
            &self,
            user_id: &str,
            _room_id: &str,
            _reason: &str,
        ) -> Result<(), AppError> {
            if self.fail_kick {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock kick failure".into(),
                });
            }
            self.kicked.lock().unwrap().push(user_id.to_string());
            Ok(())
        }
    }

    async fn audit() -> AuditService {
        use sqlx::sqlite::SqlitePoolOptions;
        let pool = SqlitePoolOptions::new()
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        AuditService::new(pool)
    }

    fn mapping(group: &str, room: &str) -> GroupMapping {
        GroupMapping {
            keycloak_group: group.to_string(),
            matrix_room_id: room.to_string(),
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn user_in_group_not_in_room_is_force_joined() {
        let synapse = MockSynapse::default(); // members = []
        let audit = audit().await;
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &mappings,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            false,
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        assert_eq!(*synapse.joined.lock().unwrap(), vec!["@alice:test.com"]);
        assert!(synapse.kicked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn user_already_in_room_is_not_rejoined() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let audit = audit().await;
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &mappings,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            false,
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        assert!(synapse.joined.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn user_not_in_group_in_room_kicked_when_remove_enabled() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let audit = audit().await;
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups: Vec<String> = vec![]; // not in group

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &mappings,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            true, // remove_from_rooms = true
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        assert_eq!(*synapse.kicked.lock().unwrap(), vec!["@alice:test.com"]);
    }

    #[tokio::test]
    async fn user_not_in_group_in_room_not_kicked_when_remove_disabled() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let audit = audit().await;
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups: Vec<String> = vec![];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &mappings,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            false, // remove_from_rooms = false
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        assert!(synapse.kicked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn member_fetch_failure_is_warning_not_error() {
        let synapse = MockSynapse {
            fail_get_members: true,
            ..Default::default()
        };
        let audit = audit().await;
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &mappings,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            false,
        )
        .await
        .unwrap();

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Could not fetch members"));
    }

    #[tokio::test]
    async fn force_join_failure_is_warning_and_continues() {
        let synapse = MockSynapse {
            fail_force_join: true,
            ..Default::default()
        };
        let audit = audit().await;
        // Two mappings — first fails, second should still run.
        let mappings = vec![
            mapping("staff", "!room1:test.com"),
            mapping("staff", "!room2:test.com"),
        ];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &mappings,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            false,
        )
        .await
        .unwrap();

        assert_eq!(outcome.warnings.len(), 2); // both rooms failed
        assert!(outcome.warnings[0].contains("Could not join"));
    }

    #[tokio::test]
    async fn kick_failure_is_warning_not_error() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            fail_kick: true,
            ..Default::default()
        };
        let audit = audit().await;
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups: Vec<String> = vec![];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &mappings,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            true,
        )
        .await
        .unwrap();

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Could not kick"));
    }

    #[tokio::test]
    async fn no_mappings_returns_ok_with_no_warnings() {
        let synapse = MockSynapse::default();
        let audit = audit().await;

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &[],
            &["staff".to_string()],
            &synapse,
            &audit,
            "sub",
            "admin",
            false,
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
    }

    // ── Preview tests ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn preview_user_in_group_not_in_room_lists_join() {
        let synapse = MockSynapse::default(); // members = []
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let preview = preview_membership("@alice:test.com", &mappings, &groups, &synapse, false)
            .await
            .unwrap();

        assert_eq!(preview.joins.len(), 1);
        assert_eq!(preview.joins[0].room_id, "!room1:test.com");
        assert!(preview.kicks.is_empty());
        assert!(preview.already_correct.is_empty());
        assert!(preview.warnings.is_empty());
    }

    #[tokio::test]
    async fn preview_user_already_in_room_lists_correct() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let preview = preview_membership("@alice:test.com", &mappings, &groups, &synapse, false)
            .await
            .unwrap();

        assert!(preview.joins.is_empty());
        assert_eq!(preview.already_correct.len(), 1);
        assert!(preview.warnings.is_empty());
    }

    #[tokio::test]
    async fn preview_kick_listed_when_remove_enabled() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups: Vec<String> = vec![]; // not in group

        let preview = preview_membership("@alice:test.com", &mappings, &groups, &synapse, true)
            .await
            .unwrap();

        assert_eq!(preview.kicks.len(), 1);
        assert!(preview.joins.is_empty());
    }

    #[tokio::test]
    async fn preview_no_kick_when_remove_disabled() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups: Vec<String> = vec![];

        let preview = preview_membership("@alice:test.com", &mappings, &groups, &synapse, false)
            .await
            .unwrap();

        assert!(preview.kicks.is_empty());
        assert!(preview.joins.is_empty());
        assert!(preview.already_correct.is_empty());
    }

    #[tokio::test]
    async fn preview_member_fetch_failure_is_warning() {
        let synapse = MockSynapse {
            fail_get_members: true,
            ..Default::default()
        };
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let preview = preview_membership("@alice:test.com", &mappings, &groups, &synapse, false)
            .await
            .unwrap();

        assert_eq!(preview.warnings.len(), 1);
        assert!(preview.warnings[0].contains("Could not fetch members"));
    }

    #[tokio::test]
    async fn preview_no_mappings_returns_empty() {
        let synapse = MockSynapse::default();

        let preview = preview_membership(
            "@alice:test.com",
            &[],
            &["staff".to_string()],
            &synapse,
            false,
        )
        .await
        .unwrap();

        assert!(preview.joins.is_empty());
        assert!(preview.kicks.is_empty());
        assert!(preview.already_correct.is_empty());
        assert!(preview.warnings.is_empty());
    }

    #[tokio::test]
    async fn reconcile_writes_audit_logs() {
        let synapse = MockSynapse::default(); // members = []
        let audit = audit().await;
        let mappings = vec![mapping("staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &mappings,
            &groups,
            &synapse,
            &audit,
            "sub",
            "admin",
            false,
        )
        .await
        .unwrap();

        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "join_room_on_reconcile");
        assert_eq!(logs[0].result, "success");
    }
}
