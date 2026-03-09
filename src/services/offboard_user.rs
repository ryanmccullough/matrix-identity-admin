//! Offboard-user workflow: fully removes a user from all systems.
//!
//! Composes lifecycle primitives into a complete offboarding sequence:
//! revoke sessions -> force logout -> disable account -> kick rooms ->
//! deactivate auth account. Non-fatal steps collect warnings;
//! fatal steps abort on failure.

use crate::{
    clients::{AuthService, IdentityProvider, MatrixService},
    error::AppError,
    models::{group_mapping::GroupMapping, workflow::WorkflowOutcome},
    services::{lifecycle_steps, AuditService},
};

/// Offboard a user: revoke sessions, force logout, disable identity,
/// kick from mapped rooms, and deactivate auth account.
///
/// Steps 1-2 (session revocation, identity logout) are non-fatal.
/// Step 3 (identity disable) is fatal — failure aborts remaining steps.
/// Step 4 (room kicks) is non-fatal; skipped with warning if no Synapse connector.
/// Step 5 (auth deactivation) is fatal if the auth user exists; if the auth user
/// lookup fails, it is non-fatal (logged as warning and skipped).
#[allow(clippy::too_many_arguments)]
pub async fn offboard_user(
    keycloak_id: &str,
    keycloak: &dyn IdentityProvider,
    mas: &dyn AuthService,
    synapse: Option<&dyn MatrixService>,
    group_mappings: &[GroupMapping],
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
    homeserver_domain: &str,
) -> Result<WorkflowOutcome, AppError> {
    let kc_user = keycloak.get_user(keycloak_id).await?;
    let username = &kc_user.username;
    let matrix_user_id = format!("@{}:{}", username, homeserver_domain);

    // 1. Revoke auth sessions (non-fatal)
    let mut outcome = lifecycle_steps::revoke_auth_sessions(
        "offboard",
        keycloak_id,
        username,
        &matrix_user_id,
        mas,
        audit,
        admin_subject,
        admin_username,
    )
    .await;

    // 2. Force identity logout (non-fatal)
    let logout_outcome = lifecycle_steps::force_identity_logout(
        "offboard",
        keycloak_id,
        &matrix_user_id,
        keycloak,
        audit,
        admin_subject,
        admin_username,
    )
    .await;
    outcome.warnings.extend(logout_outcome.warnings);

    // 3. Disable identity account (fatal)
    lifecycle_steps::disable_identity_account(
        "offboard",
        keycloak_id,
        username,
        &matrix_user_id,
        keycloak,
        audit,
        admin_subject,
        admin_username,
    )
    .await?;

    // 4. Kick from all mapped rooms (non-fatal; skip if no Synapse connector)
    if let Some(synapse) = synapse {
        let kick_outcome = lifecycle_steps::kick_from_all_mapped_rooms(
            "offboard",
            keycloak_id,
            &matrix_user_id,
            group_mappings,
            synapse,
            audit,
            admin_subject,
            admin_username,
        )
        .await;
        outcome.warnings.extend(kick_outcome.warnings);
    } else if !group_mappings.is_empty() {
        outcome.add_warning(
            "Matrix connector not configured; room membership was not cleaned up".to_string(),
        );
    }

    // 5. Deactivate auth account (fatal if auth user exists)
    let auth_user = match mas.get_user_by_username(username).await {
        Ok(u) => u,
        Err(e) => {
            tracing::warn!(error = %e, "Auth user lookup failed during offboard deactivation");
            outcome.add_warning(format!(
                "Auth user lookup failed; skipping deactivation: {e}"
            ));
            None
        }
    };
    if let Some(ref auth_user) = auth_user {
        lifecycle_steps::deactivate_auth_account(
            "offboard",
            keycloak_id,
            &auth_user.id,
            username,
            &matrix_user_id,
            mas,
            audit,
            admin_subject,
            admin_username,
        )
        .await?;
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use async_trait::async_trait;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::models::{
        keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
        mas::{MasSession, MasUser},
        synapse::{SynapseDevice, SynapseUser},
    };

    // ── Test helpers ───────────────────────────────────────────────────────────

    fn kc_user(id: &str, username: &str) -> KeycloakUser {
        KeycloakUser {
            id: id.to_string(),
            username: username.to_string(),
            email: Some(format!("{username}@example.com")),
            first_name: None,
            last_name: None,
            enabled: true,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    fn mas_user() -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: "alice".to_string(),
            deactivated_at: None,
        }
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

    fn mapping(group: &str, room: &str) -> GroupMapping {
        GroupMapping {
            keycloak_group: group.to_string(),
            matrix_room_id: room.to_string(),
        }
    }

    async fn audit_svc() -> AuditService {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        AuditService::new(pool)
    }

    // ── Mock IdentityProvider ───────────────────────────────────────────────────────

    struct MockKc {
        user: Option<KeycloakUser>,
        fail_logout: bool,
        fail_disable: bool,
    }

    #[async_trait]
    impl IdentityProvider for MockKc {
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
            self.user
                .clone()
                .ok_or_else(|| AppError::NotFound("not found".into()))
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
    }

    // ── Mock AuthService ────────────────────────────────────────────────────────────

    struct MockMs {
        user: Option<MasUser>,
        sessions: Vec<MasSession>,
        fail_finish: bool,
        fail_delete: bool,
        fail_get_user: bool,
    }

    impl MockMs {
        fn with_user_and_sessions(user: MasUser, sessions: Vec<MasSession>) -> Self {
            Self {
                user: Some(user),
                sessions,
                fail_finish: false,
                fail_delete: false,
                fail_get_user: false,
            }
        }

        fn empty() -> Self {
            Self {
                user: None,
                sessions: vec![],
                fail_finish: false,
                fail_delete: false,
                fail_get_user: false,
            }
        }
    }

    #[async_trait]
    impl AuthService for MockMs {
        async fn get_user_by_username(&self, _: &str) -> Result<Option<MasUser>, AppError> {
            if self.fail_get_user {
                Err(AppError::Upstream {
                    service: "mas".into(),
                    message: "mock get user failure".into(),
                })
            } else {
                Ok(self.user.clone())
            }
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
            unimplemented!()
        }
    }

    // ── Mock MatrixService ────────────────────────────────────────────────────────

    struct MockSyn {
        members: Vec<String>,
        fail_kick: bool,
    }

    impl MockSyn {
        fn with_members(members: Vec<String>) -> Self {
            Self {
                members,
                fail_kick: false,
            }
        }

        fn failing_kick(members: Vec<String>) -> Self {
            Self {
                members,
                fail_kick: true,
            }
        }
    }

    #[async_trait]
    impl MatrixService for MockSyn {
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
            Ok(self.members.clone())
        }
        async fn force_join_user(&self, _: &str, _: &str) -> Result<(), AppError> {
            unimplemented!()
        }
        async fn kick_user_from_room(&self, _: &str, _: &str, _: &str) -> Result<(), AppError> {
            if self.fail_kick {
                Err(AppError::Upstream {
                    service: "synapse".into(),
                    message: "mock kick failure".into(),
                })
            } else {
                Ok(())
            }
        }
    }

    // ── offboard_user tests ────────────────────────────────────────────────────

    #[tokio::test]
    async fn offboard_happy_path() {
        let audit = audit_svc().await;
        let kc = MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_logout: false,
            fail_disable: false,
        };
        let mas = MockMs::with_user_and_sessions(mas_user(), vec![active_session("s1")]);
        let syn = MockSyn::with_members(vec!["@alice:example.com".to_string()]);
        let mappings = vec![mapping("staff", "!room1:example.com")];

        let outcome = offboard_user(
            "kc-1",
            &kc,
            &mas,
            Some(&syn),
            &mappings,
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());

        let logs = audit.for_user("kc-1", 20).await.unwrap();
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"revoke_auth_session_on_offboard"));
        assert!(actions.contains(&"force_identity_logout_on_offboard"));
        assert!(actions.contains(&"disable_identity_account_on_offboard"));
        assert!(actions.contains(&"kick_room_on_offboard"));
        assert!(actions.contains(&"deactivate_auth_account_on_offboard"));
        assert!(logs.iter().all(|l| l.result == "success"));
    }

    #[tokio::test]
    async fn offboard_no_auth_user_still_disables() {
        let audit = audit_svc().await;
        let kc = MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_logout: false,
            fail_disable: false,
        };
        let mas = MockMs::empty();

        let outcome = offboard_user(
            "kc-1",
            &kc,
            &mas,
            None,
            &[],
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        assert!(!outcome.has_warnings());

        let logs = audit.for_user("kc-1", 20).await.unwrap();
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"disable_identity_account_on_offboard"));
        assert!(!actions.contains(&"revoke_auth_session_on_offboard"));
        assert!(!actions.contains(&"deactivate_auth_account_on_offboard"));
    }

    #[tokio::test]
    async fn offboard_no_matrix_connector_warns() {
        let audit = audit_svc().await;
        let kc = MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_logout: false,
            fail_disable: false,
        };
        let mas = MockMs::empty();
        let mappings = vec![mapping("staff", "!room1:example.com")];

        let outcome = offboard_user(
            "kc-1",
            &kc,
            &mas,
            None,
            &mappings,
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Matrix connector not configured"));
    }

    #[tokio::test]
    async fn offboard_partial_room_kick_failure_is_warning() {
        let audit = audit_svc().await;
        let kc = MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_logout: false,
            fail_disable: false,
        };
        let mas = MockMs::with_user_and_sessions(mas_user(), vec![]);
        let syn = MockSyn::failing_kick(vec!["@alice:example.com".to_string()]);
        let mappings = vec![mapping("staff", "!room1:example.com")];

        let outcome = offboard_user(
            "kc-1",
            &kc,
            &mas,
            Some(&syn),
            &mappings,
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("Could not kick"));

        let logs = audit.for_user("kc-1", 20).await.unwrap();
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"deactivate_auth_account_on_offboard"));
    }

    #[tokio::test]
    async fn offboard_identity_disable_failure_aborts() {
        let audit = audit_svc().await;
        let kc = MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_logout: false,
            fail_disable: true,
        };
        let mas = MockMs::with_user_and_sessions(mas_user(), vec![]);
        let syn = MockSyn::with_members(vec!["@alice:example.com".to_string()]);
        let mappings = vec![mapping("staff", "!room1:example.com")];

        let result = offboard_user(
            "kc-1",
            &kc,
            &mas,
            Some(&syn),
            &mappings,
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await;

        assert!(result.is_err());

        let logs = audit.for_user("kc-1", 20).await.unwrap();
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(!actions.contains(&"kick_room_on_offboard"));
        assert!(!actions.contains(&"deactivate_auth_account_on_offboard"));
        assert!(actions.contains(&"disable_identity_account_on_offboard"));
    }

    #[tokio::test]
    async fn offboard_auth_deactivation_failure_returns_error() {
        let audit = audit_svc().await;
        let kc = MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_logout: false,
            fail_disable: false,
        };
        let mas = MockMs {
            user: Some(mas_user()),
            sessions: vec![],
            fail_finish: false,
            fail_delete: true,
            fail_get_user: false,
        };

        let result = offboard_user(
            "kc-1",
            &kc,
            &mas,
            None,
            &[],
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await;

        assert!(result.is_err());

        let logs = audit.for_user("kc-1", 20).await.unwrap();
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"disable_identity_account_on_offboard"));
        let disable_log = logs
            .iter()
            .find(|l| l.action == "disable_identity_account_on_offboard")
            .unwrap();
        assert_eq!(disable_log.result, "success");
    }

    #[tokio::test]
    async fn offboard_auth_lookup_failure_is_non_fatal() {
        let audit = audit_svc().await;
        let kc = MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_logout: false,
            fail_disable: false,
        };
        let mas = MockMs {
            user: None,
            sessions: vec![],
            fail_finish: false,
            fail_delete: false,
            fail_get_user: true,
        };

        let outcome = offboard_user(
            "kc-1",
            &kc,
            &mas,
            None,
            &[],
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        assert!(outcome.has_warnings());
        assert!(outcome
            .warnings
            .iter()
            .any(|w| w.contains("Auth user lookup failed")));
    }
}
