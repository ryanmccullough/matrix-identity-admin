use crate::{
    clients::{AuthService, KeycloakIdentityProvider},
    error::AppError,
    models::workflow::WorkflowOutcome,
    services::{lifecycle_steps, AuditService},
};

/// Reactivate a previously disabled/offboarded user account.
///
/// Composes lifecycle primitives into a reactivate sequence:
///   1. Fetch the Keycloak user to resolve the username and Matrix ID.
///   2. Enable the identity account (fatal — error returned to caller).
///   3. Look up the MAS user by username.
///   4. If found and deactivated, reactivate the auth account (non-fatal).
#[allow(clippy::too_many_arguments)]
pub async fn reactivate_user(
    keycloak_id: &str,
    keycloak: &dyn KeycloakIdentityProvider,
    mas: &dyn AuthService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
    homeserver_domain: &str,
) -> Result<WorkflowOutcome, AppError> {
    let kc_user = keycloak.get_user(keycloak_id).await?;
    let username = &kc_user.username;
    let matrix_user_id = format!("@{}:{}", username, homeserver_domain);

    lifecycle_steps::enable_identity_account(
        "reactivate",
        keycloak_id,
        username,
        &matrix_user_id,
        keycloak,
        audit,
        admin_subject,
        admin_username,
    )
    .await?;

    let mut outcome = WorkflowOutcome::ok();

    let auth_user = match mas.get_user_by_username(username).await {
        Ok(Some(u)) => Some(u),
        Ok(None) => None,
        Err(e) => {
            tracing::warn!(error = %e, "Auth user lookup failed during reactivate; skipping auth reactivation");
            outcome.add_warning(format!("Auth user lookup failed: {e}"));
            None
        }
    };

    if let Some(ref u) = auth_user {
        if u.deactivated_at.is_some() {
            let reactivate_outcome = lifecycle_steps::reactivate_auth_account(
                "reactivate",
                keycloak_id,
                &u.id,
                username,
                &matrix_user_id,
                mas,
                audit,
                admin_subject,
                admin_username,
            )
            .await;
            outcome.warnings.extend(reactivate_outcome.warnings);
        }
    }

    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::{
        clients::{AuthService, KeycloakIdentityProvider},
        models::keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
        models::mas::{MasSession, MasUser},
        services::AuditService,
    };

    // ── Test helpers ──────────────────────────────────────────────────────────

    fn kc_user(id: &str, username: &str) -> KeycloakUser {
        KeycloakUser {
            id: id.to_string(),
            username: username.to_string(),
            email: Some(format!("{username}@example.com")),
            first_name: None,
            last_name: None,
            enabled: false,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
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

    fn mas_user_deactivated(username: &str) -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: username.to_string(),
            deactivated_at: Some("2026-01-01T00:00:00Z".to_string()),
        }
    }

    fn mas_user_active(username: &str) -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: username.to_string(),
            deactivated_at: None,
        }
    }

    // ── Mock Keycloak ─────────────────────────────────────────────────────────

    struct MockKc {
        user: Option<KeycloakUser>,
        fail_enable: bool,
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
            Ok(())
        }
        async fn disable_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
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

    // ── Mock MAS ──────────────────────────────────────────────────────────────

    struct MockMs {
        user: Option<MasUser>,
        fail_reactivate: bool,
    }

    #[async_trait]
    impl AuthService for MockMs {
        async fn get_user_by_username(&self, _: &str) -> Result<Option<MasUser>, AppError> {
            Ok(self.user.clone())
        }
        async fn list_sessions(&self, _: &str) -> Result<Vec<MasSession>, AppError> {
            Ok(vec![])
        }
        async fn finish_session(&self, _: &str, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
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

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn reactivate_succeeds_with_no_mas_user() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: None,
            fail_reactivate: false,
        });

        let outcome = reactivate_user(
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

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn reactivate_enables_keycloak_and_reactivates_mas() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(mas_user_deactivated("alice")),
            fail_reactivate: false,
        });

        let outcome = reactivate_user(
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

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 2);
        let enable = logs
            .iter()
            .find(|l| l.action == "enable_identity_account_on_reactivate")
            .unwrap();
        assert_eq!(enable.result, "success");
        let reactivate = logs
            .iter()
            .find(|l| l.action == "reactivate_auth_account_on_reactivate")
            .unwrap();
        assert_eq!(reactivate.result, "success");
    }

    #[tokio::test]
    async fn reactivate_skips_active_mas_user() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(mas_user_active("alice")),
            fail_reactivate: false,
        });

        let outcome = reactivate_user(
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

        assert!(!outcome.has_warnings());
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        // Only the enable log — no reactivate since MAS user is already active
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn reactivate_mas_failure_is_warning() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(mas_user_deactivated("alice")),
            fail_reactivate: true,
        });

        let outcome = reactivate_user(
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

        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("reactivate"));
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        let enable = logs
            .iter()
            .find(|l| l.action == "enable_identity_account_on_reactivate")
            .unwrap();
        assert_eq!(enable.result, "success");
        let reactivate = logs
            .iter()
            .find(|l| l.action == "reactivate_auth_account_on_reactivate")
            .unwrap();
        assert_eq!(reactivate.result, "failure");
    }

    #[tokio::test]
    async fn reactivate_enable_failure_returns_error() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_enable: true,
        });
        let mas = Arc::new(MockMs {
            user: None,
            fail_reactivate: false,
        });

        let result = reactivate_user(
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
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "enable_identity_account_on_reactivate");
        assert_eq!(logs[0].result, "failure");
    }

    #[tokio::test]
    async fn reactivate_user_not_found_returns_error() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: None,
            fail_enable: false,
        });
        let mas = Arc::new(MockMs {
            user: None,
            fail_reactivate: false,
        });

        let result = reactivate_user(
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
        // No audit log — we failed before doing anything
        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert!(logs.is_empty());
    }
}
