use std::sync::Arc;

use crate::{
    clients::{IdentityProviderApi, MasApi},
    error::AppError,
    models::unified::{
        CanonicalUser, LifecycleState, UnifiedSession, UnifiedUserDetail, UnifiedUserSummary,
    },
    services::identity_mapper::IdentityMapper,
};

/// Derive a user's lifecycle state from identity provider and MAS account data.
///
/// - `Disabled`: account disabled OR MAS account deactivated.
/// - `Invited`: account enabled + pending required actions (onboarding incomplete).
/// - `Active`: account enabled + no pending required actions.
pub fn derive_lifecycle_state(
    kc_enabled: bool,
    required_actions: &[String],
    mas_deactivated_at: Option<&str>,
) -> LifecycleState {
    if !kc_enabled || mas_deactivated_at.is_some() {
        LifecycleState::Disabled
    } else if !required_actions.is_empty() {
        LifecycleState::Invited
    } else {
        LifecycleState::Active
    }
}

pub struct UserService {
    identity_provider: Arc<dyn IdentityProviderApi>,
    mas: Arc<dyn MasApi>,
    mapper: IdentityMapper,
}

impl UserService {
    pub fn new(
        identity_provider: Arc<dyn IdentityProviderApi>,
        mas: Arc<dyn MasApi>,
        homeserver_domain: &str,
    ) -> Self {
        Self {
            identity_provider,
            mas,
            mapper: IdentityMapper::new(homeserver_domain),
        }
    }

    /// Search the identity provider for users matching `query` with server-side
    /// pagination. Returns lightweight summaries with derived Matrix IDs but does
    /// not fan out to MAS per result.
    pub async fn search(
        &self,
        query: &str,
        max: u32,
        first: u32,
    ) -> Result<Vec<UnifiedUserSummary>, AppError> {
        let users = self
            .identity_provider
            .search_users(query, max, first)
            .await?;

        let summaries = users
            .into_iter()
            .map(|u| {
                let mapped = self.mapper.map_summary_only(u);
                let lifecycle_state = derive_lifecycle_state(
                    mapped.canonical.enabled,
                    &mapped.canonical.required_actions,
                    None, // MAS not queried for search results
                );
                UnifiedUserSummary {
                    keycloak_id: mapped.canonical.id,
                    username: mapped.canonical.username,
                    email: mapped.canonical.email,
                    enabled: mapped.canonical.enabled,
                    lifecycle_state,
                    inferred_matrix_id: mapped.inferred_matrix_id,
                    correlation_status: mapped.correlation_status,
                }
            })
            .collect();

        Ok(summaries)
    }

    /// Load the full detail view for a single user, aggregating data from the
    /// identity provider and MAS. Missing upstream data is noted but never
    /// causes the whole request to fail.
    pub async fn get_detail(&self, user_id: &str) -> Result<UnifiedUserDetail, AppError> {
        let mut canonical: CanonicalUser = self.identity_provider.get_user(user_id).await?;

        let (groups_result, roles_result) = tokio::join!(
            self.identity_provider.get_user_groups(user_id),
            self.identity_provider.get_user_roles(user_id),
        );

        canonical.groups = groups_result.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to fetch user groups");
            vec![]
        });

        canonical.roles = roles_result.unwrap_or_else(|e| {
            tracing::warn!(error = %e, "Failed to fetch user roles");
            vec![]
        });

        let inferred_matrix_id = self.mapper.derive_matrix_id(&canonical.username);

        let mas_user = self
            .mas
            .get_user_by_username(&canonical.username)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "MAS user lookup failed");
                None
            });

        let mas_user_id = mas_user.as_ref().map(|u| u.id.clone());

        let mapped = self.mapper.map(canonical.clone(), mas_user_id.clone());

        let sessions: Vec<UnifiedSession> = match &mas_user_id {
            Some(id) => self.mas.list_sessions(id).await.unwrap_or_else(|e| {
                tracing::warn!(error = %e, "MAS session list failed");
                vec![]
            }),
            None => vec![],
        }
        .into_iter()
        .map(|s| {
            let state = if s.finished_at.is_some() {
                "finished".to_string()
            } else {
                "active".to_string()
            };
            UnifiedSession {
                id: s.id,
                session_type: s.session_type,
                created_at: s.created_at,
                last_active_at: s.last_active_at,
                user_agent: s.user_agent,
                ip_address: s.ip_address,
                state,
            }
        })
        .collect();

        let lifecycle_state = derive_lifecycle_state(
            canonical.enabled,
            &canonical.required_actions,
            mas_user.as_ref().and_then(|u| u.deactivated_at.as_deref()),
        );

        let matrix_id = Some(inferred_matrix_id);

        Ok(UnifiedUserDetail {
            keycloak_id: canonical.id,
            username: canonical.username,
            email: canonical.email,
            first_name: canonical.first_name,
            last_name: canonical.last_name,
            enabled: canonical.enabled,
            lifecycle_state,
            groups: canonical.groups,
            roles: canonical.roles,
            matrix_id,
            correlation_status: mapped.correlation_status,
            sessions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{
        mas::{MasSession, MasUser},
        unified::{CanonicalUser, CorrelationStatus, LifecycleState},
    };
    use async_trait::async_trait;

    // ── Mock IdentityProvider ─────────────────────────────────────────────────

    struct MockIdP {
        users: Vec<CanonicalUser>,
        fail_groups: bool,
        fail_roles: bool,
    }

    #[async_trait]
    impl IdentityProviderApi for MockIdP {
        async fn search_users(
            &self,
            _query: &str,
            _max: u32,
            _first: u32,
        ) -> Result<Vec<CanonicalUser>, AppError> {
            Ok(self.users.clone())
        }
        async fn get_user(&self, _id: &str) -> Result<CanonicalUser, AppError> {
            self.users
                .first()
                .cloned()
                .ok_or_else(|| AppError::NotFound("user not found".into()))
        }
        async fn get_user_groups(&self, _id: &str) -> Result<Vec<String>, AppError> {
            if self.fail_groups {
                Err(AppError::Upstream {
                    service: "idp".into(),
                    message: "groups error".into(),
                })
            } else {
                Ok(self
                    .users
                    .first()
                    .map(|u| u.groups.clone())
                    .unwrap_or_default())
            }
        }
        async fn get_user_roles(&self, _id: &str) -> Result<Vec<String>, AppError> {
            if self.fail_roles {
                Err(AppError::Upstream {
                    service: "idp".into(),
                    message: "roles error".into(),
                })
            } else {
                Ok(self
                    .users
                    .first()
                    .map(|u| u.roles.clone())
                    .unwrap_or_default())
            }
        }
        async fn logout_user(&self, _id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn count_users(&self, _query: &str) -> Result<u32, AppError> {
            Ok(self.users.len() as u32)
        }
    }

    // ── Mock MAS ──────────────────────────────────────────────────────────────

    struct MockMas {
        user: Option<MasUser>,
        sessions: Vec<MasSession>,
    }

    #[async_trait]
    impl MasApi for MockMas {
        async fn get_user_by_username(&self, _username: &str) -> Result<Option<MasUser>, AppError> {
            Ok(self.user.clone())
        }
        async fn list_sessions(&self, _mas_user_id: &str) -> Result<Vec<MasSession>, AppError> {
            Ok(self.sessions.clone())
        }
        async fn finish_session(
            &self,
            _session_id: &str,
            _session_type: &str,
        ) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_user(&self, _mas_user_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn reactivate_user(&self, _mas_user_id: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    fn canonical_user(username: &str) -> CanonicalUser {
        CanonicalUser {
            id: "kc-001".to_string(),
            username: username.to_string(),
            email: Some(format!("{username}@example.com")),
            first_name: Some("Alice".to_string()),
            last_name: Some("Smith".to_string()),
            enabled: true,
            groups: vec![],
            roles: vec![],
            required_actions: vec![],
        }
    }

    fn build_service(idp: MockIdP, mas: MockMas) -> UserService {
        UserService::new(Arc::new(idp), Arc::new(mas), "example.com")
    }

    // ── derive_lifecycle_state unit tests ─────────────────────────────────────

    #[test]
    fn lifecycle_active_when_enabled_no_actions_no_deactivation() {
        assert_eq!(
            derive_lifecycle_state(true, &[], None),
            LifecycleState::Active
        );
    }

    #[test]
    fn lifecycle_invited_when_enabled_with_required_actions() {
        assert_eq!(
            derive_lifecycle_state(true, &["UPDATE_PASSWORD".to_string()], None),
            LifecycleState::Invited
        );
    }

    #[test]
    fn lifecycle_disabled_when_kc_disabled() {
        assert_eq!(
            derive_lifecycle_state(false, &[], None),
            LifecycleState::Disabled
        );
    }

    #[test]
    fn lifecycle_disabled_when_mas_deactivated() {
        assert_eq!(
            derive_lifecycle_state(true, &[], Some("2026-01-01T00:00:00Z")),
            LifecycleState::Disabled
        );
    }

    #[test]
    fn lifecycle_disabled_takes_precedence_over_invited() {
        // disabled + pending actions → Disabled, not Invited
        assert_eq!(
            derive_lifecycle_state(false, &["VERIFY_EMAIL".to_string()], None),
            LifecycleState::Disabled
        );
    }

    // ── Search tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn search_returns_summary_with_inferred_matrix_id() {
        let svc = build_service(
            MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );

        let results = svc.search("alice", 25, 0).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].username, "alice");
        assert_eq!(
            results[0].inferred_matrix_id.as_deref(),
            Some("@alice:example.com")
        );
        assert_eq!(results[0].correlation_status, CorrelationStatus::Inferred);
    }

    #[tokio::test]
    async fn search_returns_empty_when_idp_finds_nothing() {
        let svc = build_service(
            MockIdP {
                users: vec![],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );

        let results = svc.search("nobody", 25, 0).await.unwrap();
        assert!(results.is_empty());
    }

    #[tokio::test]
    async fn search_active_user_has_active_lifecycle_state() {
        let svc = build_service(
            MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );
        let results = svc.search("alice", 25, 0).await.unwrap();
        assert_eq!(results[0].lifecycle_state, LifecycleState::Active);
    }

    #[tokio::test]
    async fn search_invited_user_has_invited_lifecycle_state() {
        let mut user = canonical_user("alice");
        user.required_actions = vec!["UPDATE_PASSWORD".to_string()];
        let svc = build_service(
            MockIdP {
                users: vec![user],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );
        let results = svc.search("alice", 25, 0).await.unwrap();
        assert_eq!(results[0].lifecycle_state, LifecycleState::Invited);
    }

    #[tokio::test]
    async fn search_disabled_user_has_disabled_lifecycle_state() {
        let mut user = canonical_user("alice");
        user.enabled = false;
        let svc = build_service(
            MockIdP {
                users: vec![user],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );
        let results = svc.search("alice", 25, 0).await.unwrap();
        assert_eq!(results[0].lifecycle_state, LifecycleState::Disabled);
    }

    // ── Detail tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_detail_confirmed_when_mas_found() {
        let svc = build_service(
            MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: Some(MasUser {
                    id: "mas-001".to_string(),
                    username: "alice".to_string(),
                    deactivated_at: None,
                }),
                sessions: vec![],
            },
        );

        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.correlation_status, CorrelationStatus::Confirmed);
        assert_eq!(detail.matrix_id.as_deref(), Some("@alice:example.com"));
    }

    #[tokio::test]
    async fn get_detail_mas_deactivated_yields_disabled_lifecycle_state() {
        let svc = build_service(
            MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: Some(MasUser {
                    id: "mas-001".to_string(),
                    username: "alice".to_string(),
                    deactivated_at: Some("2026-01-01T00:00:00Z".to_string()),
                }),
                sessions: vec![],
            },
        );
        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.lifecycle_state, LifecycleState::Disabled);
    }

    #[tokio::test]
    async fn get_detail_inferred_when_only_idp_found() {
        let svc = build_service(
            MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );

        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.correlation_status, CorrelationStatus::Inferred);
    }

    #[tokio::test]
    async fn get_detail_includes_groups_and_roles() {
        let mut user = canonical_user("alice");
        user.groups = vec!["matrix-admins".to_string()];
        user.roles = vec!["matrix-admin".to_string()];
        let svc = build_service(
            MockIdP {
                users: vec![user],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );

        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.groups, vec!["matrix-admins"]);
        assert_eq!(detail.roles, vec!["matrix-admin"]);
    }

    // ── Group/role graceful failure ───────────────────────────────────────────

    #[tokio::test]
    async fn get_detail_when_groups_fail_returns_empty_groups() {
        let svc = build_service(
            MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: true,
                fail_roles: false,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );
        let detail = svc.get_detail("kc-001").await.unwrap();
        assert!(detail.groups.is_empty(), "expected empty groups on failure");
    }

    #[tokio::test]
    async fn get_detail_when_roles_fail_returns_empty_roles() {
        let svc = build_service(
            MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: true,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );
        let detail = svc.get_detail("kc-001").await.unwrap();
        assert!(detail.roles.is_empty(), "expected empty roles on failure");
    }

    #[tokio::test]
    async fn get_detail_finished_session_has_finished_state() {
        let svc = build_service(
            MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: Some(MasUser {
                    id: "mas-001".to_string(),
                    username: "alice".to_string(),
                    deactivated_at: None,
                }),
                sessions: vec![MasSession {
                    id: "finished-sess".to_string(),
                    session_type: "compat".to_string(),
                    created_at: None,
                    last_active_at: None,
                    user_agent: None,
                    ip_address: None,
                    finished_at: Some("2026-01-01T00:00:00Z".to_string()),
                }],
            },
        );

        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.sessions.len(), 1);
        assert_eq!(detail.sessions[0].state, "finished");
    }

    // ── Graceful MAS failure ──────────────────────────────────────────────────

    struct MasLookupFails;

    #[async_trait]
    impl MasApi for MasLookupFails {
        async fn get_user_by_username(&self, _: &str) -> Result<Option<MasUser>, AppError> {
            Err(AppError::Upstream {
                service: "mas".into(),
                message: "network error".into(),
            })
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
            Ok(())
        }
    }

    struct MasSessionsFail {
        user: MasUser,
    }

    #[async_trait]
    impl MasApi for MasSessionsFail {
        async fn get_user_by_username(&self, _: &str) -> Result<Option<MasUser>, AppError> {
            Ok(Some(self.user.clone()))
        }
        async fn list_sessions(&self, _: &str) -> Result<Vec<MasSession>, AppError> {
            Err(AppError::Upstream {
                service: "mas".into(),
                message: "timeout".into(),
            })
        }
        async fn finish_session(&self, _: &str, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn reactivate_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn get_detail_when_mas_lookup_fails_returns_inferred_status() {
        let svc = UserService::new(
            Arc::new(MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: false,
            }),
            Arc::new(MasLookupFails),
            "example.com",
        );
        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.correlation_status, CorrelationStatus::Inferred);
        assert!(detail.sessions.is_empty());
    }

    #[tokio::test]
    async fn get_detail_when_session_list_fails_returns_empty_sessions() {
        let svc = UserService::new(
            Arc::new(MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: false,
            }),
            Arc::new(MasSessionsFail {
                user: MasUser {
                    id: "mas-001".to_string(),
                    username: "alice".to_string(),
                    deactivated_at: None,
                },
            }),
            "example.com",
        );
        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.correlation_status, CorrelationStatus::Confirmed);
        assert!(detail.sessions.is_empty());
    }

    #[tokio::test]
    async fn get_detail_includes_sessions() {
        let svc = build_service(
            MockIdP {
                users: vec![canonical_user("alice")],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: Some(MasUser {
                    id: "mas-001".to_string(),
                    username: "alice".to_string(),
                    deactivated_at: None,
                }),
                sessions: vec![MasSession {
                    id: "sess-1".to_string(),
                    session_type: "compat".to_string(),
                    created_at: None,
                    last_active_at: None,
                    user_agent: None,
                    ip_address: Some("10.0.0.1".to_string()),
                    finished_at: None,
                }],
            },
        );

        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.sessions.len(), 1);
        assert_eq!(detail.sessions[0].id, "sess-1");
        assert_eq!(detail.sessions[0].ip_address.as_deref(), Some("10.0.0.1"));
    }

    #[tokio::test]
    async fn get_detail_returns_error_when_user_not_found() {
        let svc = build_service(
            MockIdP {
                users: vec![],
                fail_groups: false,
                fail_roles: false,
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );
        let result = svc.get_detail("nonexistent").await;
        assert!(result.is_err());
    }

    /// Exercises unused trait methods on local test mocks to ensure coverage.
    #[tokio::test]
    async fn mock_unused_trait_methods_coverage() {
        let idp = MockIdP {
            users: vec![],
            fail_groups: false,
            fail_roles: false,
        };
        let _ = idp.logout_user("id").await;
        let _ = idp.count_users("").await;

        let mas = MockMas {
            user: None,
            sessions: vec![],
        };
        let _ = mas.finish_session("id", "compat").await;
        let _ = mas.delete_user("id").await;
        let _ = mas.reactivate_user("id").await;

        let ml = MasLookupFails;
        let _ = ml.list_sessions("id").await;
        let _ = ml.finish_session("id", "compat").await;
        let _ = ml.delete_user("id").await;
        let _ = ml.reactivate_user("id").await;

        let ms = MasSessionsFail {
            user: MasUser {
                id: "id".into(),
                username: "u".into(),
                deactivated_at: None,
            },
        };
        let _ = ms.finish_session("id", "compat").await;
        let _ = ms.delete_user("id").await;
        let _ = ms.reactivate_user("id").await;
    }
}
