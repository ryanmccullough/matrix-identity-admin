use crate::{
    clients::MatrixService,
    error::AppError,
    models::{
        audit::AuditResult,
        policy_binding::{PolicyBinding, PolicySubject},
        workflow::WorkflowOutcome,
    },
    services::audit_service::AuditService,
};

/// Expand a mapping's room ID into a list of target room IDs.
///
/// If the room is a space (has `m.space.child` state events), returns the
/// space ID followed by all child room IDs. Otherwise returns just the
/// room ID. If space child discovery fails, falls back to treating the
/// room as a single room and adds a warning.
async fn expand_targets(
    room_id: &str,
    synapse: &dyn MatrixService,
    warnings: &mut Vec<String>,
) -> Vec<String> {
    match synapse.get_space_children(room_id).await {
        Ok(children) if !children.is_empty() => {
            let mut targets = vec![room_id.to_string()];
            targets.extend(children);
            targets
        }
        Ok(_) => vec![room_id.to_string()],
        Err(e) => {
            warnings.push(format!("Could not check space children for {room_id}: {e}"));
            vec![room_id.to_string()]
        }
    }
}

/// Check whether a user matches a policy binding's subject, given their
/// Keycloak groups and roles.
fn user_matches_binding(
    binding: &PolicyBinding,
    keycloak_groups: &[String],
    keycloak_roles: &[String],
) -> bool {
    match &binding.subject {
        PolicySubject::Group(g) => keycloak_groups.contains(g),
        PolicySubject::Role(r) => keycloak_roles.contains(r),
    }
}

/// Reconcile a single user's Matrix room membership against the provided
/// policy bindings (group or role → room).
///
/// For each binding:
/// - If the user matches the binding's subject but is not in the room → force-join.
///   After a successful join, if the binding specifies a `power_level`, set it
///   (non-fatal on failure).
/// - If `binding.allow_remove` is true and the user does not match the
///   subject but is in the room → kick.
///
/// When a binding target is a space, the space is expanded: the user is joined
/// to the space first, then each child room. Kicks proceed in reverse order
/// (children first, then space).
///
/// Per-room failures are non-fatal: they are collected as warnings in the
/// returned `WorkflowOutcome` and the workflow continues to the next room.
#[allow(clippy::too_many_arguments)]
pub async fn reconcile_membership(
    keycloak_id: &str,
    matrix_user_id: &str,
    bindings: &[PolicyBinding],
    keycloak_groups: &[String],
    keycloak_roles: &[String],
    synapse: &dyn MatrixService,
    audit: &AuditService,
    actor_subject: &str,
    actor_username: &str,
) -> Result<WorkflowOutcome, AppError> {
    let mut outcome = WorkflowOutcome::ok();

    for binding in bindings {
        let matches = user_matches_binding(binding, keycloak_groups, keycloak_roles);
        let room_id = binding.target.room_id();
        let targets = expand_targets(room_id, synapse, &mut outcome.warnings).await;

        if matches {
            // Join in forward order: space first, then children.
            for target_room_id in &targets {
                let members = match synapse.get_joined_room_members(target_room_id).await {
                    Ok(m) => m,
                    Err(e) => {
                        outcome.add_warning(format!(
                            "Could not fetch members of {}: {}",
                            target_room_id, e
                        ));
                        continue;
                    }
                };

                if members.contains(&matrix_user_id.to_string()) {
                    continue;
                }

                let result = synapse
                    .force_join_user(matrix_user_id, target_room_id)
                    .await;
                let audit_result = if result.is_ok() {
                    AuditResult::Success
                } else {
                    AuditResult::Failure
                };
                if let Err(e) = audit
                    .log(
                        actor_subject,
                        actor_username,
                        Some(keycloak_id),
                        Some(matrix_user_id),
                        "join_room_on_reconcile",
                        audit_result,
                        serde_json::json!({
                            "room_id": target_room_id,
                            "subject": binding.subject.to_string(),
                        }),
                    )
                    .await
                {
                    tracing::warn!(error = %e, "Audit log write failed during reconciliation");
                    outcome.add_warning(format!("Audit log write failed: {e}"));
                }
                if let Err(e) = result {
                    outcome.add_warning(format!(
                        "Could not join {} to {}: {}",
                        matrix_user_id, target_room_id, e
                    ));
                    continue;
                }

                // Set power level after successful join, if configured.
                if let Some(level) = binding.power_level {
                    if let Err(e) = synapse
                        .set_power_level(target_room_id, matrix_user_id, level)
                        .await
                    {
                        outcome.add_warning(format!(
                            "Joined {} to {} but could not set power level {}: {}",
                            matrix_user_id, target_room_id, level, e
                        ));
                    }
                }
            }
        } else if binding.allow_remove {
            // Kick in reverse order: children first, then space.
            for target_room_id in targets.iter().rev() {
                let members = match synapse.get_joined_room_members(target_room_id).await {
                    Ok(m) => m,
                    Err(e) => {
                        outcome.add_warning(format!(
                            "Could not fetch members of {}: {}",
                            target_room_id, e
                        ));
                        continue;
                    }
                };

                if !members.contains(&matrix_user_id.to_string()) {
                    continue;
                }

                let result = synapse
                    .kick_user_from_room(
                        matrix_user_id,
                        target_room_id,
                        &format!("Removed: no longer matches {}", binding.subject),
                    )
                    .await;
                let audit_result = if result.is_ok() {
                    AuditResult::Success
                } else {
                    AuditResult::Failure
                };
                if let Err(e) = audit
                    .log(
                        actor_subject,
                        actor_username,
                        Some(keycloak_id),
                        Some(matrix_user_id),
                        "kick_room_on_reconcile",
                        audit_result,
                        serde_json::json!({
                            "room_id": target_room_id,
                            "subject": binding.subject.to_string(),
                        }),
                    )
                    .await
                {
                    tracing::warn!(error = %e, "Audit log write failed during reconciliation");
                    outcome.add_warning(format!("Audit log write failed: {e}"));
                }
                if let Err(e) = result {
                    outcome.add_warning(format!(
                        "Could not kick {} from {}: {}",
                        matrix_user_id, target_room_id, e
                    ));
                }
            }
        }
    }

    Ok(outcome)
}

/// A single room action in a preview (join, kick, or already-correct).
#[derive(Debug)]
pub struct RoomAction {
    pub room_id: String,
    pub subject: String,
    pub power_level: Option<i64>,
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
/// Reads current room membership from Synapse and compares against policy bindings.
/// Returns a `ReconcilePreview` describing joins, kicks, and rooms already correct.
/// Space mappings are expanded into child rooms. Member-fetch failures are
/// non-fatal: recorded in `warnings`, room is skipped.
pub async fn preview_membership(
    matrix_user_id: &str,
    bindings: &[PolicyBinding],
    keycloak_groups: &[String],
    keycloak_roles: &[String],
    synapse: &dyn MatrixService,
) -> Result<ReconcilePreview, AppError> {
    let mut preview = ReconcilePreview::default();

    for binding in bindings {
        let matches = user_matches_binding(binding, keycloak_groups, keycloak_roles);
        let room_id = binding.target.room_id();
        let targets = expand_targets(room_id, synapse, &mut preview.warnings).await;

        for target_room_id in &targets {
            let members = match synapse.get_joined_room_members(target_room_id).await {
                Ok(m) => m,
                Err(e) => {
                    preview.warnings.push(format!(
                        "Could not fetch members of {}: {}",
                        target_room_id, e
                    ));
                    continue;
                }
            };

            let in_room = members.contains(&matrix_user_id.to_string());
            let action = RoomAction {
                room_id: target_room_id.clone(),
                subject: binding.subject.to_string(),
                power_level: binding.power_level,
            };

            if matches && !in_room {
                preview.joins.push(action);
            } else if binding.allow_remove && !matches && in_room {
                preview.kicks.push(action);
            } else if matches && in_room {
                preview.already_correct.push(action);
            }
        }
    }

    Ok(preview)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;

    use super::*;
    use crate::{
        clients::MatrixService,
        models::{
            policy_binding::{PolicyBinding, PolicySubject, PolicyTarget},
            synapse::{RoomDetails, RoomList, SynapseDevice, SynapseUser},
        },
        services::audit_service::AuditService,
    };

    // ── Mock MatrixService ────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockSynapse {
        /// Members already in the room.
        pub members: Vec<String>,
        pub fail_get_members: bool,
        pub fail_force_join: bool,
        pub fail_kick: bool,
        /// Child room IDs returned by `get_space_children`. Keyed by space ID.
        pub space_children: std::collections::HashMap<String, Vec<String>>,
        pub fail_get_space_children: bool,
        /// Track calls as (user_id, room_id) tuples.
        pub joined: std::sync::Mutex<Vec<(String, String)>>,
        pub kicked: std::sync::Mutex<Vec<(String, String)>>,
        pub room_list: Vec<crate::models::synapse::RoomListEntry>,
        pub room_details: Option<crate::models::synapse::RoomDetails>,
        pub fail_list_rooms: bool,
        pub fail_set_power_level: bool,
        /// Track set_power_level calls as (room_id, user_id, level).
        pub power_level_calls: std::sync::Mutex<Vec<(String, String, i64)>>,
    }

    #[async_trait]
    impl MatrixService for MockSynapse {
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

        async fn force_join_user(&self, user_id: &str, room_id: &str) -> Result<(), AppError> {
            if self.fail_force_join {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock force_join failure".into(),
                });
            }
            self.joined
                .lock()
                .unwrap()
                .push((user_id.to_string(), room_id.to_string()));
            Ok(())
        }

        async fn kick_user_from_room(
            &self,
            user_id: &str,
            room_id: &str,
            _reason: &str,
        ) -> Result<(), AppError> {
            if self.fail_kick {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock kick failure".into(),
                });
            }
            self.kicked
                .lock()
                .unwrap()
                .push((user_id.to_string(), room_id.to_string()));
            Ok(())
        }

        async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError> {
            if self.fail_get_space_children {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock get_space_children failure".into(),
                });
            }
            Ok(self
                .space_children
                .get(space_id)
                .cloned()
                .unwrap_or_default())
        }

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
            self.room_details
                .clone()
                .ok_or_else(|| AppError::NotFound("room not found".into()))
        }

        async fn set_power_level(
            &self,
            room_id: &str,
            user_id: &str,
            level: i64,
        ) -> Result<(), AppError> {
            if self.fail_set_power_level {
                return Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock set_power_level failure".into(),
                });
            }
            self.power_level_calls.lock().unwrap().push((
                room_id.to_string(),
                user_id.to_string(),
                level,
            ));
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

    fn binding(subject_type: &str, subject_value: &str, room: &str) -> PolicyBinding {
        PolicyBinding {
            id: uuid::Uuid::new_v4().to_string(),
            subject: if subject_type == "role" {
                PolicySubject::Role(subject_value.to_string())
            } else {
                PolicySubject::Group(subject_value.to_string())
            },
            target: PolicyTarget::Room(room.to_string()),
            power_level: None,
            allow_remove: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn binding_with_remove(subject_type: &str, subject_value: &str, room: &str) -> PolicyBinding {
        PolicyBinding {
            allow_remove: true,
            ..binding(subject_type, subject_value, room)
        }
    }

    fn binding_with_power(
        subject_type: &str,
        subject_value: &str,
        room: &str,
        level: i64,
    ) -> PolicyBinding {
        PolicyBinding {
            power_level: Some(level),
            ..binding(subject_type, subject_value, room)
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn user_in_group_not_in_room_is_force_joined() {
        let synapse = MockSynapse::default(); // members = []
        let audit = audit().await;
        let bindings = vec![binding("group", "staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let joined = synapse.joined.lock().unwrap();
        assert_eq!(joined.len(), 1);
        assert_eq!(
            joined[0],
            ("@alice:test.com".to_string(), "!room1:test.com".to_string())
        );
        assert!(synapse.kicked.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn user_already_in_room_is_not_rejoined() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let audit = audit().await;
        let bindings = vec![binding("group", "staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        assert!(synapse.joined.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn user_not_in_group_in_room_kicked_when_allow_remove() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let audit = audit().await;
        let bindings = vec![binding_with_remove("group", "staff", "!room1:test.com")];
        let groups: Vec<String> = vec![]; // not in group

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let kicked = synapse.kicked.lock().unwrap();
        assert_eq!(kicked.len(), 1);
        assert_eq!(
            kicked[0],
            ("@alice:test.com".to_string(), "!room1:test.com".to_string())
        );
    }

    #[tokio::test]
    async fn user_not_in_group_in_room_not_kicked_when_allow_remove_false() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let audit = audit().await;
        let bindings = vec![binding("group", "staff", "!room1:test.com")]; // allow_remove = false
        let groups: Vec<String> = vec![];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
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
        let bindings = vec![binding("group", "staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
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
        // Two bindings — first fails, second should still run.
        let bindings = vec![
            binding("group", "staff", "!room1:test.com"),
            binding("group", "staff", "!room2:test.com"),
        ];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
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
        let bindings = vec![binding_with_remove("group", "staff", "!room1:test.com")];
        let groups: Vec<String> = vec![];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Could not kick"));
    }

    #[tokio::test]
    async fn no_bindings_returns_ok_with_no_warnings() {
        let synapse = MockSynapse::default();
        let audit = audit().await;
        let bindings: Vec<PolicyBinding> = vec![];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &["staff".to_string()],
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
    }

    // ── Role-based binding tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn role_binding_joins_user_when_role_matches() {
        let synapse = MockSynapse::default();
        let audit = audit().await;
        let bindings = vec![binding("role", "matrix-admin", "!admin-room:test.com")];
        let roles = vec!["matrix-admin".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &[],
            &roles,
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let joined = synapse.joined.lock().unwrap();
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].1, "!admin-room:test.com");
    }

    #[tokio::test]
    async fn role_binding_does_not_join_when_role_missing() {
        let synapse = MockSynapse::default();
        let audit = audit().await;
        let bindings = vec![binding("role", "matrix-admin", "!admin-room:test.com")];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &[],
            &[], // no roles
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        assert!(synapse.joined.lock().unwrap().is_empty());
    }

    // ── Power level tests ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn power_level_set_after_successful_join() {
        let synapse = MockSynapse::default();
        let audit = audit().await;
        let bindings = vec![binding_with_power(
            "group",
            "admins",
            "!room1:test.com",
            100,
        )];
        let groups = vec!["admins".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let joined = synapse.joined.lock().unwrap();
        assert_eq!(joined.len(), 1);
        let pl_calls = synapse.power_level_calls.lock().unwrap();
        assert_eq!(pl_calls.len(), 1);
        assert_eq!(
            pl_calls[0],
            (
                "!room1:test.com".to_string(),
                "@alice:test.com".to_string(),
                100
            )
        );
    }

    #[tokio::test]
    async fn power_level_failure_is_warning_not_error() {
        let synapse = MockSynapse {
            fail_set_power_level: true,
            ..Default::default()
        };
        let audit = audit().await;
        let bindings = vec![binding_with_power("group", "admins", "!room1:test.com", 50)];
        let groups = vec!["admins".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("could not set power level"));
        // User was still joined despite power level failure.
        let joined = synapse.joined.lock().unwrap();
        assert_eq!(joined.len(), 1);
    }

    #[tokio::test]
    async fn power_level_not_set_when_user_already_in_room() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let audit = audit().await;
        let bindings = vec![binding_with_power(
            "group",
            "admins",
            "!room1:test.com",
            100,
        )];
        let groups = vec!["admins".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        assert!(synapse.joined.lock().unwrap().is_empty());
        assert!(synapse.power_level_calls.lock().unwrap().is_empty());
    }

    // ── Preview tests ─────────────────────────────────────────────────────────

    #[tokio::test]
    async fn preview_user_in_group_not_in_room_lists_join() {
        let synapse = MockSynapse::default(); // members = []
        let bindings = vec![binding("group", "staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let preview = preview_membership("@alice:test.com", &bindings, &groups, &[], &synapse)
            .await
            .unwrap();

        assert_eq!(preview.joins.len(), 1);
        assert_eq!(preview.joins[0].room_id, "!room1:test.com");
        assert_eq!(preview.joins[0].subject, "group:staff");
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
        let bindings = vec![binding("group", "staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let preview = preview_membership("@alice:test.com", &bindings, &groups, &[], &synapse)
            .await
            .unwrap();

        assert!(preview.joins.is_empty());
        assert_eq!(preview.already_correct.len(), 1);
        assert!(preview.warnings.is_empty());
    }

    #[tokio::test]
    async fn preview_kick_listed_when_allow_remove() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let bindings = vec![binding_with_remove("group", "staff", "!room1:test.com")];
        let groups: Vec<String> = vec![]; // not in group

        let preview = preview_membership("@alice:test.com", &bindings, &groups, &[], &synapse)
            .await
            .unwrap();

        assert_eq!(preview.kicks.len(), 1);
        assert!(preview.joins.is_empty());
    }

    #[tokio::test]
    async fn preview_no_kick_when_allow_remove_false() {
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            ..Default::default()
        };
        let bindings = vec![binding("group", "staff", "!room1:test.com")]; // allow_remove = false
        let groups: Vec<String> = vec![];

        let preview = preview_membership("@alice:test.com", &bindings, &groups, &[], &synapse)
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
        let bindings = vec![binding("group", "staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        let preview = preview_membership("@alice:test.com", &bindings, &groups, &[], &synapse)
            .await
            .unwrap();

        assert_eq!(preview.warnings.len(), 1);
        assert!(preview.warnings[0].contains("Could not fetch members"));
    }

    #[tokio::test]
    async fn preview_no_bindings_returns_empty() {
        let synapse = MockSynapse::default();
        let bindings: Vec<PolicyBinding> = vec![];

        let preview = preview_membership(
            "@alice:test.com",
            &bindings,
            &["staff".to_string()],
            &[],
            &synapse,
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
        let bindings = vec![binding("group", "staff", "!room1:test.com")];
        let groups = vec!["staff".to_string()];

        reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "join_room_on_reconcile");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn preview_expands_space_children() {
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:test.com".to_string(),
            vec![
                "!child1:test.com".to_string(),
                "!child2:test.com".to_string(),
            ],
        );
        let synapse = MockSynapse {
            space_children,
            ..Default::default()
        };
        let bindings = vec![binding("group", "staff", "!space1:test.com")];
        let groups = vec!["staff".to_string()];

        let preview = preview_membership("@alice:test.com", &bindings, &groups, &[], &synapse)
            .await
            .unwrap();

        assert_eq!(preview.joins.len(), 3); // space + 2 children
        assert_eq!(preview.joins[0].room_id, "!space1:test.com");
        assert_eq!(preview.joins[1].room_id, "!child1:test.com");
        assert_eq!(preview.joins[2].room_id, "!child2:test.com");
    }

    // ── Space expansion tests ─────────────────────────────────────────────────

    #[tokio::test]
    async fn space_mapping_joins_space_and_child_rooms() {
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:test.com".to_string(),
            vec![
                "!child1:test.com".to_string(),
                "!child2:test.com".to_string(),
            ],
        );
        let synapse = MockSynapse {
            space_children,
            ..Default::default()
        };
        let audit = audit().await;
        let bindings = vec![binding("group", "staff", "!space1:test.com")];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let joined = synapse.joined.lock().unwrap();
        assert_eq!(joined.len(), 3);
        assert_eq!(joined[0].1, "!space1:test.com");
        assert_eq!(joined[1].1, "!child1:test.com");
        assert_eq!(joined[2].1, "!child2:test.com");
    }

    #[tokio::test]
    async fn space_mapping_kicks_children_before_space() {
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:test.com".to_string(),
            vec![
                "!child1:test.com".to_string(),
                "!child2:test.com".to_string(),
            ],
        );
        let synapse = MockSynapse {
            members: vec!["@alice:test.com".to_string()],
            space_children,
            ..Default::default()
        };
        let audit = audit().await;
        let bindings = vec![binding_with_remove("group", "staff", "!space1:test.com")];
        let groups: Vec<String> = vec![]; // not in group

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let kicked = synapse.kicked.lock().unwrap();
        assert_eq!(kicked.len(), 3);
        // Children kicked before space (reverse order).
        assert_eq!(kicked[0].1, "!child2:test.com");
        assert_eq!(kicked[1].1, "!child1:test.com");
        assert_eq!(kicked[2].1, "!space1:test.com");
    }

    #[tokio::test]
    async fn mixed_space_and_room_bindings() {
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:test.com".to_string(),
            vec!["!child1:test.com".to_string()],
        );
        let synapse = MockSynapse {
            space_children,
            ..Default::default()
        };
        let audit = audit().await;
        let bindings = vec![
            binding("group", "staff", "!space1:test.com"),
            binding("group", "staff", "!room1:test.com"),
        ];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());
        let joined = synapse.joined.lock().unwrap();
        assert_eq!(joined.len(), 3); // space + child + room
    }

    #[tokio::test]
    async fn space_child_discovery_failure_falls_back_to_single_room() {
        let synapse = MockSynapse {
            fail_get_space_children: true,
            ..Default::default()
        };
        let audit = audit().await;
        let bindings = vec![binding("group", "staff", "!space1:test.com")];
        let groups = vec!["staff".to_string()];

        let outcome = reconcile_membership(
            "kc-1",
            "@alice:test.com",
            &bindings,
            &groups,
            &[],
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await
        .unwrap();

        // Warning about space children failure.
        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Could not check space children"));
        // User still joined to the room itself.
        let joined = synapse.joined.lock().unwrap();
        assert_eq!(joined.len(), 1);
        assert_eq!(joined[0].1, "!space1:test.com");
    }

    // ── Preview role-based tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn preview_role_binding_lists_join() {
        let synapse = MockSynapse::default();
        let bindings = vec![binding("role", "matrix-admin", "!admin-room:test.com")];
        let roles = vec!["matrix-admin".to_string()];

        let preview = preview_membership("@alice:test.com", &bindings, &[], &roles, &synapse)
            .await
            .unwrap();

        assert_eq!(preview.joins.len(), 1);
        assert_eq!(preview.joins[0].subject, "role:matrix-admin");
    }

    #[tokio::test]
    async fn preview_includes_power_level() {
        let synapse = MockSynapse::default();
        let bindings = vec![binding_with_power(
            "group",
            "admins",
            "!room1:test.com",
            100,
        )];
        let groups = vec!["admins".to_string()];

        let preview = preview_membership("@alice:test.com", &bindings, &groups, &[], &synapse)
            .await
            .unwrap();

        assert_eq!(preview.joins.len(), 1);
        assert_eq!(preview.joins[0].power_level, Some(100));
    }
}
