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
    template: Option<&crate::models::onboarding_template::OnboardingTemplate>,
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

    // ── Validate Matrix localpart ───────────────────────────────────────────
    if !is_valid_matrix_localpart(local) {
        return Err(AppError::Validation(format!(
            "Email local-part '{local}' cannot be used as a Matrix username \
             (only lowercase letters, digits, and ._=-/ are allowed)"
        )));
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

    // ── Apply onboarding template (groups + roles) — failures are non-fatal ─
    let mut assigned_groups: Vec<String> = vec![];
    let mut failed_groups: Vec<String> = vec![];
    let mut assigned_roles: Vec<String> = vec![];
    let mut failed_roles: Vec<String> = vec![];

    if let Some(tmpl) = template {
        if !tmpl.groups.is_empty() {
            let all_groups = keycloak.list_groups().await.unwrap_or_default();
            for group_name in &tmpl.groups {
                if let Some(group) = all_groups.iter().find(|g| g.name == *group_name) {
                    if let Err(e) = keycloak.add_user_to_group(&user_id, &group.id).await {
                        tracing::warn!(group = %group_name, error = %e, "Failed to assign group during onboarding");
                        failed_groups.push(group_name.clone());
                    } else {
                        assigned_groups.push(group_name.clone());
                    }
                } else {
                    tracing::warn!(group = %group_name, "Onboarding template references unknown group");
                    failed_groups.push(group_name.clone());
                }
            }
        }

        if !tmpl.roles.is_empty() {
            let all_roles = keycloak.list_realm_roles().await.unwrap_or_default();
            let matched_roles: Vec<_> = tmpl
                .roles
                .iter()
                .filter_map(|name| all_roles.iter().find(|r| r.name == *name).cloned())
                .collect();
            for name in &tmpl.roles {
                if !all_roles.iter().any(|r| r.name == *name) {
                    tracing::warn!(role = %name, "Onboarding template references unknown role");
                    failed_roles.push(name.clone());
                }
            }
            if !matched_roles.is_empty() {
                if let Err(e) = keycloak.assign_realm_roles(&user_id, &matched_roles).await {
                    tracing::warn!(error = %e, "Failed to assign roles during onboarding");
                    for r in &matched_roles {
                        failed_roles.push(r.name.clone());
                    }
                } else {
                    for r in &matched_roles {
                        assigned_roles.push(r.name.clone());
                    }
                }
            }
        }
    }

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
                "template": template.map(|t| &t.name),
                "assigned_groups": assigned_groups,
                "failed_groups": failed_groups,
                "assigned_roles": assigned_roles,
                "failed_roles": failed_roles,
            }),
        )
        .await?;

    invite_result?;

    Ok(format!(
        "Invite sent to {email} — they will receive an email to set their password and can then log into Matrix."
    ))
}

/// Validate that a string is a valid Matrix localpart.
///
/// Per the Matrix spec, localparts may contain: lowercase ASCII letters,
/// digits, and the characters `._=-/`.
fn is_valid_matrix_localpart(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "._=-/".contains(c))
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
            onboarding_template::OnboardingTemplate,
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
        fail_add_to_group: bool,
        fail_assign_roles: bool,
        created_id: String,
        all_groups: Vec<KeycloakGroup>,
        all_roles: Vec<KeycloakRole>,
        assigned_groups: std::sync::Mutex<Vec<(String, String)>>,
        assigned_roles: std::sync::Mutex<Vec<(String, Vec<KeycloakRole>)>>,
    }

    impl Default for MockKc {
        fn default() -> Self {
            Self {
                existing_email: None,
                fail_create: false,
                fail_send_invite: false,
                fail_add_to_group: false,
                fail_assign_roles: false,
                // Non-trivial default — matches what MockKeycloak in test_helpers returns.
                created_id: "new-kc-id".to_string(),
                all_groups: vec![],
                all_roles: vec![],
                assigned_groups: std::sync::Mutex::new(vec![]),
                assigned_roles: std::sync::Mutex::new(vec![]),
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
            Ok(self.all_groups.clone())
        }
        async fn list_realm_roles(&self) -> Result<Vec<KeycloakRole>, AppError> {
            Ok(self.all_roles.clone())
        }
        async fn add_user_to_group(&self, user_id: &str, group_id: &str) -> Result<(), AppError> {
            if self.fail_add_to_group {
                return Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock add_user_to_group failure".into(),
                });
            }
            self.assigned_groups
                .lock()
                .unwrap()
                .push((user_id.to_string(), group_id.to_string()));
            Ok(())
        }
        async fn assign_realm_roles(
            &self,
            user_id: &str,
            roles: &[KeycloakRole],
        ) -> Result<(), AppError> {
            if self.fail_assign_roles {
                return Err(AppError::Upstream {
                    service: "keycloak".into(),
                    message: "mock assign_realm_roles failure".into(),
                });
            }
            self.assigned_roles
                .lock()
                .unwrap()
                .push((user_id.to_string(), roles.to_vec()));
            Ok(())
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
            None,
        )
        .await;

        assert!(result.is_err());
        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        assert_eq!(logs[0].action, "invite_user");
        assert_eq!(logs[0].result, "failure");
    }

    // ── Onboarding template ──────────────────────────────────────────────────

    #[tokio::test]
    async fn invite_with_template_assigns_groups_and_roles() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            all_groups: vec![KeycloakGroup {
                id: "g-staff-id".to_string(),
                name: "staff".to_string(),
                path: "/staff".to_string(),
            }],
            all_roles: vec![KeycloakRole {
                id: "r-admin-id".to_string(),
                name: "admin".to_string(),
                composite: false,
                client_role: false,
                container_id: None,
            }],
            ..Default::default()
        });
        let mas = Arc::new(MockMs::default());
        let tmpl = OnboardingTemplate {
            name: "Staff".to_string(),
            description: "Full access".to_string(),
            groups: vec!["staff".to_string()],
            roles: vec!["admin".to_string()],
        };

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
            Some(&tmpl),
        )
        .await;

        assert!(result.is_ok());
        let groups = kc.assigned_groups.lock().unwrap();
        assert_eq!(groups.len(), 1);
        assert_eq!(
            groups[0],
            ("new-kc-id".to_string(), "g-staff-id".to_string())
        );
        let roles = kc.assigned_roles.lock().unwrap();
        assert_eq!(roles.len(), 1);
        assert_eq!(roles[0].0, "new-kc-id");
        assert_eq!(roles[0].1.len(), 1);
        assert_eq!(roles[0].1[0].name, "admin");
    }

    #[tokio::test]
    async fn invite_with_template_unknown_group_still_succeeds() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc::default());
        let mas = Arc::new(MockMs::default());
        let tmpl = OnboardingTemplate {
            name: "Bad".to_string(),
            description: "References nonexistent group".to_string(),
            groups: vec!["nonexistent".to_string()],
            roles: vec![],
        };

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
            Some(&tmpl),
        )
        .await;

        assert!(result.is_ok());
        let groups = kc.assigned_groups.lock().unwrap();
        assert!(groups.is_empty());
    }

    #[tokio::test]
    async fn invite_with_template_group_failure_still_succeeds() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            fail_add_to_group: true,
            all_groups: vec![KeycloakGroup {
                id: "g-staff-id".to_string(),
                name: "staff".to_string(),
                path: "/staff".to_string(),
            }],
            ..Default::default()
        });
        let mas = Arc::new(MockMs::default());
        let tmpl = OnboardingTemplate {
            name: "Staff".to_string(),
            description: "".to_string(),
            groups: vec!["staff".to_string()],
            roles: vec![],
        };

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
            Some(&tmpl),
        )
        .await;

        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn invite_with_template_role_failure_still_succeeds() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            fail_assign_roles: true,
            all_roles: vec![KeycloakRole {
                id: "r-admin-id".to_string(),
                name: "admin".to_string(),
                composite: false,
                client_role: false,
                container_id: None,
            }],
            ..Default::default()
        });
        let mas = Arc::new(MockMs::default());
        let tmpl = OnboardingTemplate {
            name: "Staff".to_string(),
            description: "".to_string(),
            groups: vec![],
            roles: vec!["admin".to_string()],
        };

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
            Some(&tmpl),
        )
        .await;

        assert!(result.is_ok());
        // Verify failure was tracked in audit metadata
        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        let invite_log = logs.iter().find(|l| l.action == "invite_user").unwrap();
        assert!(invite_log
            .metadata_json
            .contains("\"failed_roles\":[\"admin\"]"));
    }

    #[tokio::test]
    async fn invite_with_template_unknown_role_tracked_in_audit() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc::default()); // no roles available
        let mas = Arc::new(MockMs::default());
        let tmpl = OnboardingTemplate {
            name: "Staff".to_string(),
            description: "".to_string(),
            groups: vec![],
            roles: vec!["nonexistent-role".to_string()],
        };

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
            Some(&tmpl),
        )
        .await;

        assert!(result.is_ok());
        let logs = audit.for_user("new-kc-id", 10).await.unwrap();
        let invite_log = logs.iter().find(|l| l.action == "invite_user").unwrap();
        assert!(invite_log.metadata_json.contains("nonexistent-role"));
    }

    #[tokio::test]
    async fn invite_without_template_skips_assignment() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc {
            all_groups: vec![KeycloakGroup {
                id: "g-staff-id".to_string(),
                name: "staff".to_string(),
                path: "/staff".to_string(),
            }],
            all_roles: vec![KeycloakRole {
                id: "r-admin-id".to_string(),
                name: "admin".to_string(),
                composite: false,
                client_role: false,
                container_id: None,
            }],
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
            None,
        )
        .await;

        assert!(result.is_ok());
        let groups = kc.assigned_groups.lock().unwrap();
        assert!(groups.is_empty());
        let roles = kc.assigned_roles.lock().unwrap();
        assert!(roles.is_empty());
    }

    // ── Matrix localpart validation ───────────────────────────────────────────

    #[tokio::test]
    async fn invite_rejects_plus_in_localpart() {
        let audit = audit_svc().await;
        let kc = Arc::new(MockKc::default());
        let mas = Arc::new(MockMs::default());

        let result = invite_user(
            "user+tag@test.com",
            None,
            kc.as_ref(),
            mas.as_ref(),
            &audit,
            "sub",
            "admin",
            "example.com",
            None,
            None,
        )
        .await;
        assert!(
            matches!(result, Err(AppError::Validation(msg)) if msg.contains("Matrix username"))
        );
    }

    #[test]
    fn valid_matrix_localparts() {
        assert!(is_valid_matrix_localpart("alice"));
        assert!(is_valid_matrix_localpart("alice.bob"));
        assert!(is_valid_matrix_localpart("alice_bob"));
        assert!(is_valid_matrix_localpart("alice-bob"));
        assert!(is_valid_matrix_localpart("alice=bob"));
        assert!(is_valid_matrix_localpart("alice/bob"));
        assert!(is_valid_matrix_localpart("123"));
    }

    #[test]
    fn invalid_matrix_localparts() {
        assert!(!is_valid_matrix_localpart(""));
        assert!(!is_valid_matrix_localpart("Alice"));
        assert!(!is_valid_matrix_localpart("alice+bob"));
        assert!(!is_valid_matrix_localpart("alice bob"));
        assert!(!is_valid_matrix_localpart("alice@bob"));
        assert!(!is_valid_matrix_localpart("alice:bob"));
    }
}
