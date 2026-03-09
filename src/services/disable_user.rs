use serde_json::json;

use crate::{
    clients::{AuthService, IdentityProvider},
    error::AppError,
    models::{audit::AuditResult, workflow::WorkflowOutcome},
    services::AuditService,
};

/// Disable a user account across Keycloak and MAS.
///
/// Steps:
///   1. Fetch the Keycloak user to resolve the username and Matrix ID.
///   2. Look up the MAS user by username (non-fatal if missing or unreachable).
///   3. Revoke all active MAS sessions (non-fatal per session — each is
///      audit-logged individually; a failed revoke does not abort the disable).
///   4. Disable the Keycloak account (fatal — audit-logged; error is returned
///      to the caller).
pub async fn disable_user(
    keycloak_id: &str,
    keycloak: &dyn IdentityProvider,
    mas: &dyn AuthService,
    audit: &AuditService,
    admin_subject: &str,
    admin_username: &str,
    homeserver_domain: &str,
) -> Result<WorkflowOutcome, AppError> {
    let mut outcome = WorkflowOutcome::ok();

    let kc_user = keycloak.get_user(keycloak_id).await?;
    let username = &kc_user.username;
    let matrix_user_id = format!("@{}:{}", username, homeserver_domain);

    // ── Revoke active MAS sessions ────────────────────────────────────────────
    let mas_user = mas
        .get_user_by_username(username)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!(error = %e, "MAS user lookup failed during disable; skipping session revocation");
            None
        });

    if let Some(ref mas_user) = mas_user {
        let sessions = mas
            .list_sessions(&mas_user.id)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "MAS session list failed during disable; skipping session revocation");
                vec![]
            });

        for session in sessions.iter().filter(|s| s.finished_at.is_none()) {
            let result = mas.finish_session(&session.id, &session.session_type).await;
            let audit_result = if result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            };

            // NOTE: session revocation failures are logged but do not abort —
            // we still proceed to disable the Keycloak account. The failure is
            // surfaced to the caller via WorkflowOutcome so it can be shown to
            // the admin rather than silently swallowed.
            if let Err(ref e) = result {
                tracing::warn!(
                    session_id = %session.id,
                    error = %e,
                    "Failed to revoke MAS session during disable"
                );
                outcome.add_warning(format!(
                    "Session {} ({}) could not be revoked: {}",
                    session.id, session.session_type, e
                ));
            }

            audit
                .log(
                    admin_subject,
                    admin_username,
                    Some(keycloak_id),
                    Some(&matrix_user_id),
                    "revoke_mas_session_on_disable",
                    audit_result,
                    json!({
                        "session_id": session.id,
                        "session_type": session.session_type,
                    }),
                )
                .await?;
        }
    }

    // ── Disable Keycloak account ──────────────────────────────────────────────
    let kc_result = keycloak.disable_user(keycloak_id).await;
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
            "disable_keycloak_account",
            audit_result,
            json!({
                "keycloak_user_id": keycloak_id,
                "username": username,
                "mas_sessions_found": mas_user.is_some(),
            }),
        )
        .await?;

    kc_result?;
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use sqlx::sqlite::SqlitePoolOptions;

    use super::*;
    use crate::{
        clients::{AuthService, IdentityProvider},
        models::{
            keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
            mas::{MasSession, MasUser},
        },
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
            enabled: true,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
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

    async fn audit_svc() -> AuditService {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::migrate!("./migrations").run(&pool).await.unwrap();
        AuditService::new(pool)
    }

    // ── Mock Keycloak ─────────────────────────────────────────────────────────

    struct MockKc {
        user: Option<KeycloakUser>,
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

    // ── Mock MAS ──────────────────────────────────────────────────────────────

    struct MockMs {
        user: Option<MasUser>,
        sessions: Vec<MasSession>,
        fail_finish: bool,
    }

    #[async_trait]
    impl AuthService for MockMs {
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
            Ok(())
        }
        async fn reactivate_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    fn mas_user(username: &str) -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: username.to_string(),
            deactivated_at: None,
        }
    }

    // ── Tests ─────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn disable_succeeds_with_no_mas_user() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_disable: false,
        });
        let mas = Arc::new(MockMs {
            user: None,
            sessions: vec![],
            fail_finish: false,
        });

        disable_user(
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

        let logs = audit.for_user("kc-1", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "disable_keycloak_account");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn disable_revokes_active_sessions_and_skips_finished() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_disable: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(mas_user("alice")),
            sessions: vec![
                active_session("s1"),
                finished_session("s2"),
                active_session("s3"),
            ],
            fail_finish: false,
        });

        disable_user(
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

        let logs = audit.for_user("kc-1", 10).await.unwrap();
        // 2 active session revokes + 1 disable_keycloak_account
        let revokes: Vec<_> = logs
            .iter()
            .filter(|l| l.action == "revoke_mas_session_on_disable")
            .collect();
        assert_eq!(revokes.len(), 2);
        assert!(revokes.iter().all(|l| l.result == "success"));
        let disables: Vec<_> = logs
            .iter()
            .filter(|l| l.action == "disable_keycloak_account")
            .collect();
        assert_eq!(disables.len(), 1);
        assert_eq!(disables[0].result, "success");
    }

    #[tokio::test]
    async fn disable_session_failure_is_logged_but_does_not_abort() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_disable: false,
        });
        let mas = Arc::new(MockMs {
            user: Some(mas_user("alice")),
            sessions: vec![active_session("s1")],
            fail_finish: true,
        });

        // Workflow should still succeed — session failure is non-fatal
        let outcome = disable_user(
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

        // Caller must be notified of the partial failure via WorkflowOutcome.
        assert!(outcome.has_warnings());
        assert!(outcome.warnings[0].contains("s1"));

        let logs = audit.for_user("kc-1", 10).await.unwrap();
        let revoke = logs
            .iter()
            .find(|l| l.action == "revoke_mas_session_on_disable")
            .unwrap();
        assert_eq!(revoke.result, "failure");
        let disable = logs
            .iter()
            .find(|l| l.action == "disable_keycloak_account")
            .unwrap();
        assert_eq!(disable.result, "success");
    }

    #[tokio::test]
    async fn disable_keycloak_failure_returns_error_and_is_logged() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: Some(kc_user("kc-1", "alice")),
            fail_disable: true,
        });
        let mas = Arc::new(MockMs {
            user: None,
            sessions: vec![],
            fail_finish: false,
        });

        let result = disable_user(
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
        assert_eq!(logs[0].action, "disable_keycloak_account");
        assert_eq!(logs[0].result, "failure");
    }

    #[tokio::test]
    async fn disable_keycloak_user_not_found_returns_error() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            user: None,
            fail_disable: false,
        });
        let mas = Arc::new(MockMs {
            user: None,
            sessions: vec![],
            fail_finish: false,
        });

        let result = disable_user(
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
