//! Shared composable primitives for lifecycle workflows.
//!
//! Each function represents a single step that can be combined by higher-level
//! workflows like `disable_user` and `offboard_user`. Functions take a
//! `context` parameter (e.g. `"disable"`, `"offboard"`) used to construct
//! audit action names like `revoke_auth_session_on_{context}`.

use serde_json::json;

use crate::{
    clients::{AuthService, KeycloakIdentityProvider, MatrixService},
    error::AppError,
    models::{audit::AuditResult, policy_binding::PolicyBinding, workflow::WorkflowOutcome},
    services::AuditService,
};

/// Revoke all active auth (MAS) sessions for a user.
///
/// Non-fatal: if user lookup or session listing fails, a warning is logged and
/// an empty outcome is returned. Per-session failures add warnings but do not
/// abort the workflow.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn revoke_auth_sessions(
    context: &str,
    keycloak_id: &str,
    username: &str,
    matrix_user_id: &str,
    mas: &dyn AuthService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> WorkflowOutcome {
    let mut outcome = WorkflowOutcome::ok();

    let auth_user = match mas.get_user_by_username(username).await {
        Ok(Some(u)) => u,
        Ok(None) => return outcome,
        Err(e) => {
            tracing::warn!(error = %e, "Auth user lookup failed during {context}; skipping session revocation");
            outcome.add_warning(format!("Auth user lookup failed: {e}"));
            return outcome;
        }
    };

    let sessions = match mas.list_sessions(&auth_user.id).await {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(error = %e, "Auth session list failed during {context}; skipping session revocation");
            outcome.add_warning(format!("Auth session list failed: {e}"));
            return outcome;
        }
    };

    let action = format!("revoke_auth_session_on_{context}");

    for session in sessions.iter().filter(|s| s.finished_at.is_none()) {
        let result = mas.finish_session(&session.id, &session.session_type).await;
        let audit_result = if result.is_ok() {
            AuditResult::Success
        } else {
            AuditResult::Failure
        };

        let _ = audit
            .log(
                admin_subject,
                admin_username,
                Some(keycloak_id),
                Some(matrix_user_id),
                &action,
                audit_result,
                json!({
                    "session_id": session.id,
                    "session_type": session.session_type,
                }),
            )
            .await;

        if let Err(ref e) = result {
            tracing::warn!(
                session_id = %session.id,
                error = %e,
                "Failed to revoke auth session during {context}"
            );
            outcome.add_warning(format!(
                "Session {} ({}) could not be revoked: {}",
                session.id, session.session_type, e
            ));
        }
    }

    outcome
}

/// Force-logout a user from Keycloak.
///
/// Non-fatal: failure adds a warning to the outcome rather than returning an error.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn force_identity_logout(
    context: &str,
    keycloak_id: &str,
    matrix_user_id: &str,
    keycloak: &dyn KeycloakIdentityProvider,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> WorkflowOutcome {
    let mut outcome = WorkflowOutcome::ok();
    let action = format!("force_identity_logout_on_{context}");

    let result = keycloak.logout_user(keycloak_id).await;
    let audit_result = if result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    let _ = audit
        .log(
            admin_subject,
            admin_username,
            Some(keycloak_id),
            Some(matrix_user_id),
            &action,
            audit_result,
            json!({ "keycloak_user_id": keycloak_id }),
        )
        .await;

    if let Err(e) = result {
        tracing::warn!(error = %e, "Force identity logout failed during {context}");
        outcome.add_warning(format!("Identity logout failed: {e}"));
    }

    outcome
}

/// Disable a user account in Keycloak.
///
/// Fatal: returns `Err` on failure (after audit logging the failure).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn disable_identity_account(
    context: &str,
    keycloak_id: &str,
    username: &str,
    matrix_user_id: &str,
    keycloak: &dyn KeycloakIdentityProvider,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> Result<(), AppError> {
    let action = format!("disable_identity_account_on_{context}");

    let result = keycloak.disable_user(keycloak_id).await;
    let audit_result = if result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    let _ = audit
        .log(
            admin_subject,
            admin_username,
            Some(keycloak_id),
            Some(matrix_user_id),
            &action,
            audit_result,
            json!({
                "keycloak_user_id": keycloak_id,
                "username": username,
            }),
        )
        .await;

    result
}

/// Enable a user account in Keycloak.
///
/// Fatal: returns `Err` on failure (after audit logging the failure).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enable_identity_account(
    context: &str,
    keycloak_id: &str,
    username: &str,
    matrix_user_id: &str,
    keycloak: &dyn KeycloakIdentityProvider,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> Result<(), AppError> {
    let action = format!("enable_identity_account_on_{context}");

    let result = keycloak.enable_user(keycloak_id).await;
    let audit_result = if result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    let _ = audit
        .log(
            admin_subject,
            admin_username,
            Some(keycloak_id),
            Some(matrix_user_id),
            &action,
            audit_result,
            json!({
                "keycloak_user_id": keycloak_id,
                "username": username,
            }),
        )
        .await;

    result
}

/// Deactivate a user account in MAS.
///
/// Fatal: returns `Err` on failure (after audit logging the failure).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn deactivate_auth_account(
    context: &str,
    keycloak_id: &str,
    auth_user_id: &str,
    username: &str,
    matrix_user_id: &str,
    mas: &dyn AuthService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> Result<(), AppError> {
    let action = format!("deactivate_auth_account_on_{context}");

    let result = mas.delete_user(auth_user_id).await;
    let audit_result = if result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    let _ = audit
        .log(
            admin_subject,
            admin_username,
            Some(keycloak_id),
            Some(matrix_user_id),
            &action,
            audit_result,
            json!({
                "auth_user_id": auth_user_id,
                "username": username,
            }),
        )
        .await;

    result
}

/// Reactivate a previously deactivated MAS user account.
///
/// Non-fatal: failure adds a warning to the outcome rather than returning an
/// error. The MAS account may not exist or may already be active.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn reactivate_auth_account(
    context: &str,
    keycloak_id: &str,
    auth_user_id: &str,
    username: &str,
    matrix_user_id: &str,
    mas: &dyn AuthService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> WorkflowOutcome {
    let mut outcome = WorkflowOutcome::ok();
    let action = format!("reactivate_auth_account_on_{context}");

    let result = mas.reactivate_user(auth_user_id).await;
    let audit_result = if result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    let _ = audit
        .log(
            admin_subject,
            admin_username,
            Some(keycloak_id),
            Some(matrix_user_id),
            &action,
            audit_result,
            json!({
                "auth_user_id": auth_user_id,
                "username": username,
            }),
        )
        .await;

    if let Err(e) = result {
        tracing::warn!(error = %e, "Auth account reactivation failed during {context}");
        outcome.add_warning(format!("Auth account reactivate failed: {e}"));
    }

    outcome
}

/// Kick a user from all rooms mapped via group policy.
///
/// Non-fatal: per-room failures add warnings but do not abort. Unlike
/// `reconcile_membership`, this kicks from ALL mapped rooms unconditionally,
/// regardless of the user's current group membership. Deduplicates room
/// targets since multiple bindings may reference the same room.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn kick_from_all_mapped_rooms(
    context: &str,
    keycloak_id: &str,
    matrix_user_id: &str,
    bindings: &[PolicyBinding],
    synapse: &dyn MatrixService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
) -> WorkflowOutcome {
    let mut outcome = WorkflowOutcome::ok();
    let action = format!("kick_room_on_{context}");
    let mut seen_rooms = std::collections::HashSet::new();

    for binding in bindings {
        let room_id = binding.target.room_id();
        if !seen_rooms.insert(room_id.to_string()) {
            continue;
        }

        // Expand space mappings: discover children, then kick in reverse order
        // (children first, then the space itself).
        let targets = match synapse.get_space_children(room_id).await {
            Ok(children) if !children.is_empty() => {
                let mut t = vec![room_id.to_string()];
                t.extend(children);
                t
            }
            Ok(_) => vec![room_id.to_string()],
            Err(e) => {
                outcome.add_warning(format!(
                    "Could not check space children for {}: {}",
                    room_id, e
                ));
                vec![room_id.to_string()]
            }
        };

        // Kick in reverse order: children first, then the space/room itself.
        for target_room in targets.iter().rev() {
            let members = match synapse.get_joined_room_members(target_room).await {
                Ok(m) => m,
                Err(e) => {
                    outcome
                        .add_warning(format!("Could not fetch members of {}: {}", target_room, e));
                    continue;
                }
            };

            if !members.contains(&matrix_user_id.to_string()) {
                continue;
            }

            let result = synapse
                .kick_user_from_room(matrix_user_id, target_room, "Offboarded")
                .await;
            let audit_result = if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            };

            let _ = audit
                .log(
                    admin_subject,
                    admin_username,
                    Some(keycloak_id),
                    Some(matrix_user_id),
                    &action,
                    audit_result,
                    json!({
                        "room_id": target_room,
                        "subject": binding.subject.to_string(),
                    }),
                )
                .await;

            if let Err(e) = result {
                outcome.add_warning(format!(
                    "Could not kick {} from {}: {}",
                    matrix_user_id, target_room, e
                ));
            }
        }
    }

    outcome
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::{
        models::{
            keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
            mas::{MasSession, MasUser},
            policy_binding::{PolicyBinding, PolicySubject, PolicyTarget},
            synapse::{SynapseDevice, SynapseUser},
        },
        services::AuditService,
    };

    // ── Test helpers ───────────────────────────────────────────────────────────

    async fn audit_svc() -> AuditService {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        AuditService::new(pool)
    }

    fn active_session(id: &str) -> MasSession {
        MasSession {
            id: id.to_string(),
            session_type: "compat".to_string(),
            created_at: None,
            last_active_at: None,
            user_agent: None,
            ip_address: None,
            finished_at: None,
        }
    }

    fn finished_session(id: &str) -> MasSession {
        MasSession {
            id: id.to_string(),
            session_type: "compat".to_string(),
            created_at: None,
            last_active_at: None,
            user_agent: None,
            ip_address: None,
            finished_at: Some("2026-01-01T00:00:00Z".to_string()),
        }
    }

    fn test_binding(room: &str) -> PolicyBinding {
        PolicyBinding {
            id: uuid::Uuid::new_v4().to_string(),
            subject: PolicySubject::Group("staff".to_string()),
            target: PolicyTarget::Room(room.to_string()),
            power_level: None,
            allow_remove: false,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    // ── Mock KeycloakIdentityProvider ───────────────────────────────────────────────────────

    struct MockKeycloak {
        fail_logout: bool,
        fail_disable: bool,
        fail_enable: bool,
    }

    #[async_trait]
    impl KeycloakIdentityProvider for MockKeycloak {
        async fn search_users(
            &self,
            _: &str,
            _: u32,
            _: u32,
        ) -> Result<Vec<KeycloakUser>, AppError> {
            unimplemented!()
        }
        async fn count_users(&self, _: &str) -> Result<u32, AppError> {
            unimplemented!()
        }
        async fn get_user(&self, _: &str) -> Result<KeycloakUser, AppError> {
            unimplemented!()
        }
        async fn get_user_by_email(&self, _: &str) -> Result<Option<KeycloakUser>, AppError> {
            unimplemented!()
        }
        async fn get_user_groups(&self, _: &str) -> Result<Vec<KeycloakGroup>, AppError> {
            unimplemented!()
        }
        async fn get_user_roles(&self, _: &str) -> Result<Vec<KeycloakRole>, AppError> {
            unimplemented!()
        }
        async fn logout_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_logout {
                Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock logout failure".into(),
                })
            } else {
                Ok(())
            }
        }
        async fn create_user(&self, _: &str, _: &str) -> Result<String, AppError> {
            unimplemented!()
        }
        async fn send_invite_email(&self, _: &str) -> Result<(), AppError> {
            unimplemented!()
        }
        async fn delete_user(&self, _: &str) -> Result<(), AppError> {
            unimplemented!()
        }
        async fn disable_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_disable {
                Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock disable failure".into(),
                })
            } else {
                Ok(())
            }
        }
        async fn enable_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_enable {
                Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock enable failure".into(),
                })
            } else {
                Ok(())
            }
        }
        async fn list_groups(&self) -> Result<Vec<KeycloakGroup>, AppError> {
            Ok(vec![])
        }
        async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>, AppError> {
            Ok(vec![])
        }
    }

    // ── Mock AuthService ────────────────────────────────────────────────────────────

    struct MockMas {
        user: Option<MasUser>,
        sessions: Vec<MasSession>,
        fail_finish: bool,
        fail_delete: bool,
        fail_reactivate: bool,
    }

    #[async_trait]
    impl AuthService for MockMas {
        async fn get_user_by_username(&self, _: &str) -> Result<Option<MasUser>, AppError> {
            Ok(self.user.clone())
        }
        async fn list_sessions(&self, _: &str) -> Result<Vec<MasSession>, AppError> {
            Ok(self.sessions.clone())
        }
        async fn finish_session(&self, _: &str, _: &str) -> Result<(), AppError> {
            if self.fail_finish {
                Err(AppError::Upstream {
                    service: "mas".into(),
                    message: "mock finish failure".into(),
                })
            } else {
                Ok(())
            }
        }
        async fn delete_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_delete {
                Err(AppError::Upstream {
                    service: "mas".into(),
                    message: "mock delete failure".into(),
                })
            } else {
                Ok(())
            }
        }
        async fn reactivate_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_reactivate {
                Err(AppError::Upstream {
                    service: "mas".into(),
                    message: "mock reactivate failure".into(),
                })
            } else {
                Ok(())
            }
        }
    }

    fn mas_user() -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: "alice".to_string(),
            deactivated_at: None,
        }
    }

    // ── Mock MatrixService ────────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockSynapse {
        members: Vec<String>,
        fail_get_members: bool,
        fail_kick: bool,
        space_children: std::collections::HashMap<String, Vec<String>>,
        kicked_rooms: std::sync::Mutex<Vec<String>>,
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
        async fn get_joined_room_members(&self, _: &str) -> Result<Vec<String>, AppError> {
            if self.fail_get_members {
                Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock member fetch failure".into(),
                })
            } else {
                Ok(self.members.clone())
            }
        }
        async fn force_join_user(&self, _: &str, _: &str) -> Result<(), AppError> {
            unimplemented!()
        }
        async fn kick_user_from_room(
            &self,
            _: &str,
            room_id: &str,
            _: &str,
        ) -> Result<(), AppError> {
            self.kicked_rooms.lock().unwrap().push(room_id.to_string());
            if self.fail_kick {
                Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock kick failure".into(),
                })
            } else {
                Ok(())
            }
        }
        async fn get_space_children(&self, space_id: &str) -> Result<Vec<String>, AppError> {
            Ok(self
                .space_children
                .get(space_id)
                .cloned()
                .unwrap_or_default())
        }
        async fn list_rooms(
            &self,
            _: u32,
            _: Option<&str>,
        ) -> Result<crate::models::synapse::RoomList, AppError> {
            Ok(crate::models::synapse::RoomList {
                rooms: vec![],
                next_batch: None,
                total_rooms: Some(0),
            })
        }
        async fn get_room_details(
            &self,
            room_id: &str,
        ) -> Result<crate::models::synapse::RoomDetails, AppError> {
            Ok(crate::models::synapse::RoomDetails {
                room_id: room_id.to_string(),
                name: None,
                canonical_alias: None,
                topic: None,
                joined_members: Some(0),
                is_space: false,
            })
        }
        async fn set_power_level(&self, _: &str, _: &str, _: i64) -> Result<(), AppError> {
            Ok(())
        }
    }

    // ── revoke_auth_sessions ───────────────────────────────────────────────────

    #[tokio::test]
    async fn revoke_sessions_no_auth_user_returns_ok() {
        let audit = audit_svc().await;
        let mas = MockMas {
            user: None,
            sessions: vec![],
            fail_finish: false,
            fail_delete: false,
            fail_reactivate: false,
        };

        let outcome = revoke_auth_sessions(
            "disable",
            "kc-1",
            "alice",
            "@alice:example.com",
            &mas,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert!(logs.is_empty());
    }

    #[tokio::test]
    async fn revoke_sessions_finishes_active_skips_finished() {
        let audit = audit_svc().await;
        let mas = MockMas {
            user: Some(mas_user()),
            sessions: vec![
                active_session("s1"),
                active_session("s2"),
                finished_session("s3"),
            ],
            fail_finish: false,
            fail_delete: false,
            fail_reactivate: false,
        };

        let outcome = revoke_auth_sessions(
            "disable",
            "kc-1",
            "alice",
            "@alice:example.com",
            &mas,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 2);
        assert!(logs
            .iter()
            .all(|l| l.action == "revoke_auth_session_on_disable"));
        assert!(logs.iter().all(|l| l.result == "success"));
    }

    #[tokio::test]
    async fn revoke_sessions_failure_is_non_fatal_warning() {
        let audit = audit_svc().await;
        let mas = MockMas {
            user: Some(mas_user()),
            sessions: vec![active_session("s1")],
            fail_finish: true,
            fail_delete: false,
            fail_reactivate: false,
        };

        let outcome = revoke_auth_sessions(
            "offboard",
            "kc-1",
            "alice",
            "@alice:example.com",
            &mas,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("s1"));
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "revoke_auth_session_on_offboard");
        assert_eq!(logs[0].result, "failure");
    }

    // ── force_identity_logout ──────────────────────────────────────────────────

    #[tokio::test]
    async fn force_logout_success() {
        let audit = audit_svc().await;
        let keycloak = MockKeycloak {
            fail_logout: false,
            fail_disable: false,
            fail_enable: false,
        };

        let outcome = force_identity_logout(
            "disable",
            "kc-1",
            "@alice:example.com",
            &keycloak,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "force_identity_logout_on_disable");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn force_logout_failure_is_warning() {
        let audit = audit_svc().await;
        let keycloak = MockKeycloak {
            fail_logout: true,
            fail_disable: false,
            fail_enable: false,
        };

        let outcome = force_identity_logout(
            "offboard",
            "kc-1",
            "@alice:example.com",
            &keycloak,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Identity logout failed"));
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "force_identity_logout_on_offboard");
        assert_eq!(logs[0].result, "failure");
    }

    // ── disable_identity_account ───────────────────────────────────────────────

    #[tokio::test]
    async fn disable_account_success() {
        let audit = audit_svc().await;
        let keycloak = MockKeycloak {
            fail_logout: false,
            fail_disable: false,
            fail_enable: false,
        };

        let result = disable_identity_account(
            "disable",
            "kc-1",
            "alice",
            "@alice:example.com",
            &keycloak,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.is_ok());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "disable_identity_account_on_disable");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn disable_account_failure_returns_error() {
        let audit = audit_svc().await;
        let keycloak = MockKeycloak {
            fail_logout: false,
            fail_disable: true,
            fail_enable: false,
        };

        let result = disable_identity_account(
            "offboard",
            "kc-1",
            "alice",
            "@alice:example.com",
            &keycloak,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.is_err());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "disable_identity_account_on_offboard");
        assert_eq!(logs[0].result, "failure");
    }

    // ── deactivate_auth_account ────────────────────────────────────────────────

    #[tokio::test]
    async fn deactivate_account_success() {
        let audit = audit_svc().await;
        let mas = MockMas {
            user: None,
            sessions: vec![],
            fail_finish: false,
            fail_delete: false,
            fail_reactivate: false,
        };

        let result = deactivate_auth_account(
            "offboard",
            "kc-1",
            "mas-001",
            "alice",
            "@alice:example.com",
            &mas,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.is_ok());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "deactivate_auth_account_on_offboard");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn deactivate_account_failure_returns_error() {
        let audit = audit_svc().await;
        let mas = MockMas {
            user: None,
            sessions: vec![],
            fail_finish: false,
            fail_delete: true,
            fail_reactivate: false,
        };

        let result = deactivate_auth_account(
            "offboard",
            "kc-1",
            "mas-001",
            "alice",
            "@alice:example.com",
            &mas,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.is_err());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "deactivate_auth_account_on_offboard");
        assert_eq!(logs[0].result, "failure");
    }

    // ── kick_from_all_mapped_rooms ─────────────────────────────────────────────

    #[tokio::test]
    async fn kick_present_user_from_room() {
        let audit = audit_svc().await;
        let synapse = MockSynapse {
            members: vec!["@alice:example.com".to_string()],
            ..Default::default()
        };
        let bindings = vec![test_binding("!room1:example.com")];

        let outcome = kick_from_all_mapped_rooms(
            "offboard",
            "kc-1",
            "@alice:example.com",
            &bindings,
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "kick_room_on_offboard");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn kick_skips_absent_user() {
        let audit = audit_svc().await;
        let synapse = MockSynapse {
            members: vec!["@bob:example.com".to_string()],
            ..Default::default()
        };
        let bindings = vec![test_binding("!room1:example.com")];

        let outcome = kick_from_all_mapped_rooms(
            "offboard",
            "kc-1",
            "@alice:example.com",
            &bindings,
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert!(logs.is_empty());
    }

    #[tokio::test]
    async fn kick_failure_is_non_fatal_warning() {
        let audit = audit_svc().await;
        let synapse = MockSynapse {
            members: vec!["@alice:example.com".to_string()],
            fail_kick: true,
            ..Default::default()
        };
        let bindings = vec![test_binding("!room1:example.com")];

        let outcome = kick_from_all_mapped_rooms(
            "offboard",
            "kc-1",
            "@alice:example.com",
            &bindings,
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Could not kick"));
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "kick_room_on_offboard");
        assert_eq!(logs[0].result, "failure");
    }

    #[tokio::test]
    async fn kick_expands_space_and_kicks_children_first() {
        let audit = audit_svc().await;
        let mut space_children = std::collections::HashMap::new();
        space_children.insert(
            "!space1:example.com".to_string(),
            vec![
                "!child1:example.com".to_string(),
                "!child2:example.com".to_string(),
            ],
        );
        let synapse = MockSynapse {
            members: vec!["@alice:example.com".to_string()],
            space_children,
            ..Default::default()
        };
        let bindings = vec![test_binding("!space1:example.com")];

        let outcome = kick_from_all_mapped_rooms(
            "offboard",
            "kc-1",
            "@alice:example.com",
            &bindings,
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(!outcome.has_warnings());
        let kicked = synapse.kicked_rooms.lock().unwrap();
        assert_eq!(kicked.len(), 3);
        // Children first (reverse), then space
        assert_eq!(kicked[0], "!child2:example.com");
        assert_eq!(kicked[1], "!child1:example.com");
        assert_eq!(kicked[2], "!space1:example.com");
    }

    #[tokio::test]
    async fn kick_member_fetch_failure_is_warning() {
        let audit = audit_svc().await;
        let synapse = MockSynapse {
            fail_get_members: true,
            ..Default::default()
        };
        let bindings = vec![test_binding("!room1:example.com")];

        let outcome = kick_from_all_mapped_rooms(
            "offboard",
            "kc-1",
            "@alice:example.com",
            &bindings,
            &synapse,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Could not fetch members"));
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert!(logs.is_empty());
    }

    // ── enable_identity_account ─────────────────────────────────────────────────

    #[tokio::test]
    async fn enable_account_success() {
        let audit = audit_svc().await;
        let keycloak = MockKeycloak {
            fail_logout: false,
            fail_disable: false,
            fail_enable: false,
        };

        let result = enable_identity_account(
            "reactivate",
            "kc-1",
            "alice",
            "@alice:example.com",
            &keycloak,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.is_ok());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn enable_account_failure_returns_error() {
        let audit = audit_svc().await;
        let keycloak = MockKeycloak {
            fail_logout: false,
            fail_disable: false,
            fail_enable: true,
        };

        let result = enable_identity_account(
            "reactivate",
            "kc-1",
            "alice",
            "@alice:example.com",
            &keycloak,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.is_err());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
        assert_eq!(logs[0].result, "failure");
    }

    // ── reactivate_auth_account ─────────────────────────────────────────────────

    #[tokio::test]
    async fn reactivate_auth_account_success() {
        let audit = audit_svc().await;
        let mas = MockMas {
            user: None,
            sessions: vec![],
            fail_finish: false,
            fail_delete: false,
            fail_reactivate: false,
        };

        let result = reactivate_auth_account(
            "reactivate",
            "kc-1",
            "mas-001",
            "alice",
            "@alice:example.com",
            &mas,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(!result.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "reactivate_auth_account_on_reactivate");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn reactivate_auth_account_failure_is_warning() {
        let audit = audit_svc().await;
        let mas = MockMas {
            user: None,
            sessions: vec![],
            fail_finish: false,
            fail_delete: false,
            fail_reactivate: true,
        };

        let result = reactivate_auth_account(
            "reactivate",
            "kc-1",
            "mas-001",
            "alice",
            "@alice:example.com",
            &mas,
            &audit,
            "sub",
            "admin",
        )
        .await;

        assert!(result.has_warnings());
        assert!(result.warnings[0].contains("reactivate"));
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "reactivate_auth_account_on_reactivate");
        assert_eq!(logs[0].result, "failure");
    }
}
