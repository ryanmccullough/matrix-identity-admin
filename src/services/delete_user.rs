use serde_json::json;

use crate::{
    clients::{AuthService, KeycloakIdentityProvider},
    error::AppError,
    models::{audit::AuditResult, workflow::WorkflowOutcome},
    services::AuditService,
};

/// Result of the delete-user workflow.
///
/// A delete can complete in two ways:
/// - `Deleted`: Keycloak was removed successfully.
/// - `PartialFailure`: MAS was already deactivated, but the Keycloak delete
///   failed and the admin must retry or reactivate manually.
pub enum DeleteUserResult {
    Deleted(WorkflowOutcome),
    PartialFailure(WorkflowOutcome),
}

impl DeleteUserResult {
    /// Return the workflow warnings collected while deleting the user.
    pub fn outcome(&self) -> &WorkflowOutcome {
        match self {
            Self::Deleted(outcome) | Self::PartialFailure(outcome) => outcome,
        }
    }
}

/// Delete a user account from Keycloak and deactivate it in MAS.
///
/// Steps:
///   1. Fetch the Keycloak user to resolve the username and Matrix ID.
///   2. Look up the MAS user by username (non-fatal if missing or unreachable).
///   3. Deactivate the MAS account (fatal — audit-logged; if this fails the
///      Keycloak record is preserved so the admin can retry).
///   4. Delete the Keycloak user (fatal unless MAS was already deactivated; in
///      that case the workflow returns a partial-failure outcome with recovery
///      guidance so the admin can retry or reactivate manually).
pub async fn delete_user(
    keycloak_id: &str,
    keycloak: &dyn KeycloakIdentityProvider,
    mas: &dyn AuthService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
    homeserver_domain: &str,
) -> Result<DeleteUserResult, AppError> {
    let kc_user = keycloak.get_user(keycloak_id).await?;
    let username = &kc_user.username;
    let matrix_user_id = format!("@{}:{}", username, homeserver_domain);
    let mut outcome = WorkflowOutcome::ok();

    // ── Deactivate MAS user first (if present) ────────────────────────────────
    // MAS is attempted before Keycloak so that if it fails the Keycloak record
    // is preserved and the admin can retry cleanly.
    let mas_user = match mas.get_user_by_username(username).await {
        Ok(user) => user,
        Err(e) => {
            tracing::warn!(error = %e, "MAS user lookup failed during delete; skipping MAS deactivation");
            outcome.add_warning(
                "Deleted the Keycloak user, but MAS lookup failed before cleanup. Check MAS for a leftover account."
                    .to_string(),
            );
            None
        }
    };

    if let Some(ref mas_user) = mas_user {
        let mas_result = mas.deactivate_user(&mas_user.id).await;
        let audit_result = if mas_result.is_ok() {
            AuditResult::Success
        } else {
            AuditResult::Failure
        };

        audit
            .log(
                admin_subject,
                admin_username,
                Some(keycloak_id),
                Some(&matrix_user_id),
                "deactivate_mas_user",
                audit_result,
                json!({
                    "keycloak_user_id": keycloak_id,
                    "mas_user_id": mas_user.id,
                    "username": username,
                }),
            )
            .await?;

        mas_result?;
    }

    // ── Delete Keycloak user ──────────────────────────────────────────────────
    let kc_result = keycloak.delete_user(keycloak_id).await;
    let audit_result = if kc_result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    audit
        .log(
            admin_subject,
            admin_username,
            Some(keycloak_id),
            Some(&matrix_user_id),
            "delete_keycloak_user",
            audit_result,
            json!({
                "keycloak_user_id": keycloak_id,
                "username": username,
                "mas_deactivated": mas_user.is_some(),
            }),
        )
        .await?;

    if let Err(err) = kc_result {
        if mas_user.is_some() {
            outcome.add_warning(
                "MAS was deactivated, but deleting the Keycloak user failed. Retry delete or reactivate the user before reinviting them."
                    .to_string(),
            );
            return Ok(DeleteUserResult::PartialFailure(outcome));
        }

        return Err(err);
    }

    Ok(DeleteUserResult::Deleted(outcome))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::{
        clients::{AuthService, KeycloakIdentityProvider},
        models::{
            keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
            mas::{MasUser, SessionListResult},
        },
        services::AuditService,
    };

    // ── Test helpers ──────────────────────────────────────────────────────────

    async fn audit_svc() -> AuditService {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        AuditService::new(pool)
    }

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

    fn mas_user(username: &str) -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: username.to_string(),
            deactivated_at: None,
        }
    }

    // ── Mock Keycloak ─────────────────────────────────────────────────────────

    struct MockKc {
        user: Option<KeycloakUser>,
        fail_delete: bool,
    }

    #[async_trait]
    impl KeycloakIdentityProvider for MockKc {
        async fn search_users(
            &self,
            _: &str,
            _: u32,
            _: u32,
        ) -> Result<Vec<KeycloakUser>, AppError> {
            Ok(vec![])
        }
        async fn count_users(&self, _: &str) -> Result<u32, AppError> {
            Ok(0)
        }
        async fn get_user(&self, _: &str) -> Result<KeycloakUser, AppError> {
            self.user
                .clone()
                .ok_or_else(|| AppError::NotFound("not found".into()))
        }
        async fn get_user_by_email(&self, _: &str) -> Result<Option<KeycloakUser>, AppError> {
            Ok(None)
        }
        async fn get_user_groups(&self, _: &str) -> Result<Vec<KeycloakGroup>, AppError> {
            Ok(vec![])
        }
        async fn get_user_roles(&self, _: &str) -> Result<Vec<KeycloakRole>, AppError> {
            Ok(vec![])
        }
        async fn logout_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn create_user(&self, _: &str, _: &str) -> Result<String, AppError> {
            Ok("id".into())
        }
        async fn send_invite_email(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_delete {
                Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock delete failure".into(),
                })
            } else {
                Ok(())
            }
        }
        async fn disable_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn enable_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn list_groups(&self) -> Result<Vec<KeycloakGroup>, AppError> {
            Ok(vec![])
        }
        async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>, AppError> {
            Ok(vec![])
        }
        async fn add_user_to_group(&self, _: &str, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn assign_realm_roles(&self, _: &str, _: &[KeycloakRole]) -> Result<(), AppError> {
            Ok(())
        }
    }

    // ── Mock MAS ──────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockMs {
        user: Option<MasUser>,
        fail_lookup: bool,
        fail_deactivate: bool,
    }

    #[async_trait]
    impl AuthService for MockMs {
        async fn get_user_by_username(&self, _: &str) -> Result<Option<MasUser>, AppError> {
            if self.fail_lookup {
                Err(AppError::Upstream {
                    service: "mas".into(),
                    message: "mock lookup failure".into(),
                })
            } else {
                Ok(self.user.clone())
            }
        }
        async fn list_sessions(&self, _: &str) -> Result<SessionListResult, AppError> {
            Ok(SessionListResult {
                sessions: vec![],
                warnings: vec![],
            })
        }
        async fn finish_session(&self, _: &str, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn deactivate_user(&self, _: &str) -> Result<(), AppError> {
            if self.fail_deactivate {
                Err(AppError::Upstream {
                    service: "mas".into(),
                    message: "mock deactivate failure".into(),
                })
            } else {
                Ok(())
            }
        }
        async fn reactivate_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn delete_succeeds_with_no_mas_user() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_delete: false,
        });
        let mas = Arc::new(MockMs::default());

        let result = delete_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();
        assert!(!result.outcome().has_warnings());

        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "delete_keycloak_user");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn delete_deactivates_mas_then_deletes_keycloak() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_delete: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(mas_user("alice")),
            ..Default::default()
        });

        let result = delete_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();
        assert!(!result.outcome().has_warnings());

        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 2);
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"deactivate_mas_user"));
        assert!(actions.contains(&"delete_keycloak_user"));
        assert!(logs.iter().all(|l| l.result == "success"));
    }

    #[tokio::test]
    async fn delete_mas_failure_aborts_before_keycloak_and_is_logged() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_delete: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(mas_user("alice")),
            fail_deactivate: true,
            ..Default::default()
        });

        let result = delete_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await;

        assert!(result.is_err());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        // Only the MAS deactivation is logged — Keycloak delete was not attempted.
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "deactivate_mas_user");
        assert_eq!(logs[0].result, "failure");
    }

    #[tokio::test]
    async fn delete_keycloak_failure_without_mas_deactivation_is_logged_and_returned() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_delete: true,
        });
        let mas = Arc::new(MockMs::default());

        let result = delete_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await;

        assert!(result.is_err());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs[0].action, "delete_keycloak_user");
        assert_eq!(logs[0].result, "failure");
    }

    #[tokio::test]
    async fn delete_keycloak_failure_after_mas_deactivation_returns_partial_failure() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_delete: true,
        });
        let mas = Arc::new(MockMs {
            user: Some(mas_user("alice")),
            ..Default::default()
        });

        let result = delete_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();

        let outcome = match result {
            DeleteUserResult::PartialFailure(outcome) => outcome,
            DeleteUserResult::Deleted(_) => panic!("expected partial failure"),
        };
        assert!(outcome.has_warnings());
        assert!(outcome
            .warning_summary()
            .contains("MAS was deactivated, but deleting the Keycloak user failed"));

        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 2);
        assert_eq!(logs[0].action, "deactivate_mas_user");
        assert_eq!(logs[0].result, "success");
        assert_eq!(logs[1].action, "delete_keycloak_user");
        assert_eq!(logs[1].result, "failure");
    }

    #[tokio::test]
    async fn delete_mas_lookup_failure_is_non_fatal_and_surfaces_warning() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_delete: false,
        });
        let mas = Arc::new(MockMs {
            fail_lookup: true,
            ..Default::default()
        });

        let result = delete_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await
        .unwrap();
        assert!(result.outcome().has_warnings());
        assert!(result
            .outcome()
            .warning_summary()
            .contains("MAS lookup failed before cleanup"));

        // MAS lookup failed non-fatally — only the Keycloak delete is logged.
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "delete_keycloak_user");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn delete_keycloak_user_not_found_returns_error() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: None,
            fail_delete: false,
        });
        let mas = Arc::new(MockMs::default());

        let result = delete_user(
            "kc-1",
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
        )
        .await;

        assert!(result.is_err());
        // Failed before any audit log was written.
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert!(logs.is_empty());
    }
}
