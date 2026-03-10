use serde_json::json;

use crate::{
    clients::{AuthService, KeycloakIdentityProvider},
    error::AppError,
    models::audit::AuditResult,
    services::AuditService,
};

/// Invite a new user by email across Keycloak and optionally MAS.
///
/// Steps:
///   1. Validate and normalise the email address.
///   2. Check domain allowlist (if configured).
///   3. Reject if a Keycloak user already exists for this email.
///   4. Look up MAS for a deactivated account with the same username
///      (non-fatal if MAS is unreachable).
///   5. Create the user in Keycloak.
///   6. Reactivate the deactivated MAS account if one was found (fatal).
///   7. Send the Keycloak invite email (fatal — audit-logged regardless).
///
/// Returns the success message to surface to the caller.
#[allow(clippy::too_many_arguments)]
pub async fn invite_user(
    raw_email: &str,
    allowed_domains: Option<&[String]>,
    keycloak: &dyn KeycloakIdentityProvider,
    mas: &dyn AuthService,
    audit: &AuditService,
    actor_subject: &str,
    actor_username: &str,
    homeserver_domain: &str,
    requested_by: Option<&str>,
) -> Result<String, AppError> {
    // ── Validate and normalise email ──────────────────────────────────────────
    let email = raw_email.trim().to_lowercase();
    let at = email
        .find('@')
        .ok_or_else(|| AppError::Validation("Invalid email address".to_string()))?;
    let local = &email[..at];
    let domain = &email[at + 1..];

    if local.is_empty() || domain.is_empty() || !domain.contains('.') {
        return Err(AppError::Validation("Invalid email address".to_string()));
    }

    // ── Domain allowlist ──────────────────────────────────────────────────────
    if let Some(allowed) = allowed_domains {
        if !allowed.iter().any(|d| d == domain) {
            return Err(AppError::Validation(format!(
                "Email domain '{domain}' is not permitted"
            )));
        }
    }

    // ── Check for existing Keycloak user ──────────────────────────────────────
    if let Some(existing) = keycloak.get_user_by_email(&email).await? {
        return Err(AppError::Validation(format!(
            "A user with email {email} already exists (id: {})",
            existing.id
        )));
    }

    // ── Check MAS for a deactivated account ──────────────────────────────────
    // Non-fatal: if MAS is unreachable, log a warning and proceed. The
    // Keycloak user will still be created; the stale MAS account stays
    // deactivated until the user next logs in.
    let existing_mas = mas.get_user_by_username(local).await.unwrap_or_else(|e| {
        tracing::warn!(error = %e, "MAS user lookup failed during invite");
        None
    });

    // ── Create user in Keycloak ───────────────────────────────────────────────
    // Use the email local-part as the Matrix username.
    let user_id = keycloak.create_user(local, &email).await?;
    let matrix_user_id = format!("@{}:{}", local, homeserver_domain);

    // ── Reactivate MAS user if previously deactivated ────────────────────────
    // Reactivating preserves the Matrix ID and room history rather than
    // leaving a permanently deactivated ghost account.
    if let Some(ref mas_user) = existing_mas {
        if mas_user.deactivated_at.is_some() {
            let reactivate_result = mas.reactivate_user(&mas_user.id).await;
            let audit_result = if reactivate_result.is_ok() {
                AuditResult::Success
            } else {
                AuditResult::Failure
            };

            audit
                .log(
                    actor_subject,
                    actor_username,
                    Some(&user_id),
                    Some(&matrix_user_id),
                    "reactivate_mas_user",
                    audit_result,
                    json!({
                        "email": email,
                        "mas_user_id": mas_user.id,
                        "keycloak_user_id": user_id,
                    }),
                )
                .await?;

            reactivate_result?;
        }
    }

    // ── Send invite email via Keycloak ────────────────────────────────────────
    let invite_result = keycloak.send_invite_email(&user_id).await;
    let audit_result = if invite_result.is_ok() {
        AuditResult::Success
    } else {
        AuditResult::Failure
    };

    audit
        .log(
            actor_subject,
            actor_username,
            Some(&user_id),
            Some(&matrix_user_id),
            "invite_user",
            audit_result,
            json!({
                "email": email,
                "requested_by": requested_by,
                "keycloak_user_id": user_id,
            }),
        )
        .await?;

    invite_result?;

    Ok(format!(
        "Invite sent to {email} — they will receive an email to set their password and can then log into Matrix."
    ))
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
            mas::{MasSession, MasUser},
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

    fn existing_kc_user() -> KeycloakUser {
        KeycloakUser {
            id: "existing-id".to_string(),
            username: "existing".to_string(),
            email: Some("user@test.com".to_string()),
            first_name: None,
            last_name: None,
            enabled: true,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    fn deactivated_mas_user() -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: "user".to_string(),
            deactivated_at: Some("2026-01-01T00:00:00Z".to_string()),
        }
    }

    fn active_mas_user() -> MasUser {
        MasUser {
            id: "mas-001".to_string(),
            username: "user".to_string(),
            deactivated_at: None,
        }
    }

    // ── Mock Keycloak ─────────────────────────────────────────────────────────

    struct MockKc {
        existing_email: Option<KeycloakUser>,
        fail_create: bool,
        fail_send_invite: bool,
        created_id: String,
    }

    impl Default for MockKc {
        fn default() -> Self {
            Self {
                existing_email: None,
                fail_create: false,
                fail_send_invite: false,
                // Non-trivial default — matches what MockKeycloak in test_helpers returns.
                created_id: "new-kc-id".to_string(),
            }
        }
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
            Err(AppError::NotFound("not found".into()))
        }
        async fn get_user_by_email(&self, _: &str) -> Result<Option<KeycloakUser>, AppError> {
            Ok(self.existing_email.clone())
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
            if self.fail_create {
                Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock create failure".into(),
                })
            } else {
                Ok(self.created_id.clone())
            }
        }
        async fn send_invite_email(&self, _: &str) -> Result<(), AppError> {
            if self.fail_send_invite {
                Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock invite failure".into(),
                })
            } else {
                Ok(())
            }
        }
        async fn delete_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
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
    }

    // ── Mock MAS ──────────────────────────────────────────────────────────────

    #[derive(Default)]
    struct MockMs {
        user: Option<MasUser>,
        fail_lookup: bool,
        fail_reactivate: bool,
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
    async fn invite_succeeds_with_no_mas_account() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc::default());
        let mas = Arc::new(MockMs::default());

        let result = invite_user(
            "user@test.com",
            None,
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
        )
        .await;

        assert!(result.is_ok());
        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "invite_user");
        assert_eq!(logs[0].result, "success");
    }

    #[tokio::test]
    async fn invite_blocked_by_domain_allowlist() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc::default());
        let mas = Arc::new(MockMs::default());
        let allowed = vec!["allowed.com".to_string()];

        let result = invite_user(
            "user@other.com",
            Some(&allowed),
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
        )
        .await;

        assert!(matches!(result, Err(AppError::Validation(_))));
    }

    #[tokio::test]
    async fn invite_blocked_for_existing_keycloak_user() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            existing_email: Some(existing_kc_user()),
            ..Default::default()
        });
        let mas = Arc::new(MockMs::default());

        let result = invite_user(
            "user@test.com",
            None,
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
        )
        .await;

        assert!(matches!(result, Err(AppError::Validation(_))));
    }

    #[tokio::test]
    async fn invite_reactivates_deactivated_mas_account() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc::default());
        let mas = Arc::new(MockMs {
            user: Some(deactivated_mas_user()),
            ..Default::default()
        });

        let result = invite_user(
            "user@test.com",
            None,
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
        )
        .await;

        assert!(result.is_ok());
        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        let actions: Vec<&str> = logs.iter().map(|l| l.action.as_str()).collect();
        assert!(actions.contains(&"reactivate_mas_user"));
        assert!(actions.contains(&"invite_user"));
        assert!(logs.iter().all(|l| l.result == "success"));
    }

    #[tokio::test]
    async fn invite_active_mas_user_does_not_reactivate() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc::default());
        let mas = Arc::new(MockMs {
            user: Some(active_mas_user()),
            ..Default::default()
        });

        let result = invite_user(
            "user@test.com",
            None,
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
        )
        .await;

        assert!(result.is_ok());
        // Only invite_user logged — no reactivate_mas_user entry
        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].action, "invite_user");
    }

    #[tokio::test]
    async fn invite_mas_lookup_failure_is_non_fatal() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc::default());
        let mas = Arc::new(MockMs {
            fail_lookup: true,
            ..Default::default()
        });

        let result = invite_user(
            "user@test.com",
            None,
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn invite_reactivate_failure_aborts_and_is_logged() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc::default());
        let mas = Arc::new(MockMs {
            user: Some(deactivated_mas_user()),
            fail_reactivate: true,
            ..Default::default()
        });

        let result = invite_user(
            "user@test.com",
            None,
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
        )
        .await;

        assert!(result.is_err());
        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        let reactivate = logs
            .iter()
            .find(|l| l.action == "reactivate_mas_user")
            .unwrap();
        assert_eq!(reactivate.result, "failure");
    }

    #[tokio::test]
    async fn invite_keycloak_create_failure_returns_error() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            fail_create: true,
            ..Default::default()
        });
        let mas = Arc::new(MockMs::default());

        let result = invite_user(
            "user@test.com",
            None,
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
        )
        .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn invite_send_email_failure_is_logged_and_returns_error() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            fail_send_invite: true,
            ..Default::default()
        });
        let mas = Arc::new(MockMs::default());

        let result = invite_user(
            "user@test.com",
            None,
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
        )
        .await;

        assert!(result.is_err());
        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        assert_eq!(logs[0].action, "invite_user");
        assert_eq!(logs[0].result, "failure");
    }
}
