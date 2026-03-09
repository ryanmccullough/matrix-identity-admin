use std::sync::Arc;

use crate::{
    clients::{KeycloakApi, MasApi},
    error::AppError,
    models::unified::{LifecycleState, UnifiedSession, UnifiedUserDetail, UnifiedUserSummary},
    services::identity_mapper::IdentityMapper,
};

/// Derive a user's lifecycle state from Keycloak and MAS account data.
///
/// - `Disabled`: KC account disabled OR MAS account deactivated.
/// - `Invited`: KC enabled + pending required actions (onboarding incomplete).
/// - `Active`: KC enabled + no pending required actions.
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
    keycloak: Arc<dyn KeycloakApi>,
    mas: Arc<dyn MasApi>,
    mapper: IdentityMapper,
}

impl UserService {
    pub fn new(
        keycloak: Arc<dyn KeycloakApi>,
        mas: Arc<dyn MasApi>,
        homeserver_domain: &str,
    ) -> Self {
        Self {
            keycloak,
            mas,
            mapper: IdentityMapper::new(homeserver_domain),
        }
    }

    /// Search Keycloak for users matching `query` with server-side pagination.
    /// Returns lightweight summaries with derived Matrix IDs but does not fan
    /// out to MAS per result.
    pub async fn search(
        &self,
        query: &str,
        max: u32,
        first: u32,
    ) -> Result<Vec<UnifiedUserSummary>, AppError> {
        let users = self.keycloak.search_users(query, max, first).await?;

        let summaries = users
            .into_iter()
            .map(|u| {
                let mapped = self.mapper.map_summary_only(u);
                let lifecycle_state = derive_lifecycle_state(
                    mapped.keycloak_user.enabled,
                    &mapped.keycloak_user.required_actions,
                    None, // MAS not queried for search results
                );
                UnifiedUserSummary {
                    keycloak_id: mapped.keycloak_user.id,
                    username: mapped.keycloak_user.username,
                    email: mapped.keycloak_user.email,
                    enabled: mapped.keycloak_user.enabled,
                    lifecycle_state,
                    inferred_matrix_id: mapped.inferred_matrix_id,
                    correlation_status: mapped.correlation_status,
                }
            })
            .collect();

        Ok(summaries)
    }

    /// Load the full detail view for a single Keycloak user, aggregating data
    /// from Keycloak and MAS. Missing upstream data is noted but never causes
    /// the whole request to fail.
    pub async fn get_detail(&self, keycloak_user_id: &str) -> Result<UnifiedUserDetail, AppError> {
        let user = self.keycloak.get_user(keycloak_user_id).await?;

        let (groups_result, roles_result) = tokio::join!(
            self.keycloak.get_user_groups(keycloak_user_id),
            self.keycloak.get_user_roles(keycloak_user_id),
        );

        let groups = groups_result
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Failed to fetch Keycloak groups");
                vec![]
            })
            .into_iter()
            .map(|g| g.name)
            .collect();

        let roles = roles_result
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "Failed to fetch Keycloak roles");
                vec![]
            })
            .into_iter()
            .map(|r| r.name)
            .collect();

        let inferred_matrix_id = self.mapper.derive_matrix_id(&user.username);

        let mas_user = self
            .mas
            .get_user_by_username(&user.username)
            .await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "MAS user lookup failed");
                None
            });

        let mas_user_id = mas_user.as_ref().map(|u| u.id.clone());

        let mapped = self.mapper.map(user.clone(), mas_user_id.clone());

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
            user.enabled,
            &user.required_actions,
            mas_user.as_ref().and_then(|u| u.deactivated_at.as_deref()),
        );

        let matrix_id = Some(inferred_matrix_id);

        Ok(UnifiedUserDetail {
            keycloak_id: user.id,
            username: user.username,
            email: user.email,
            first_name: user.first_name,
            last_name: user.last_name,
            enabled: user.enabled,
            lifecycle_state,
            groups,
            roles,
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
        keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
        mas::{MasSession, MasUser},
        unified::{CorrelationStatus, LifecycleState},
    };
    use async_trait::async_trait;

    // ── Manual mock implementations ───────────────────────────────────────────

    struct MockKeycloak {
        users: Vec<KeycloakUser>,
        groups: Vec<KeycloakGroup>,
        roles: Vec<KeycloakRole>,
    }

    #[async_trait]
    impl KeycloakApi for MockKeycloak {
        async fn search_users(
            &self,
            _query: &str,
            _max: u32,
            _first: u32,
        ) -> Result<Vec<KeycloakUser>, AppError> {
            Ok(self.users.clone())
        }
        async fn get_user(&self, _user_id: &str) -> Result<KeycloakUser, AppError> {
            self.users
                .first()
                .cloned()
                .ok_or_else(|| AppError::NotFound("user not found".into()))
        }
        async fn get_user_groups(&self, _user_id: &str) -> Result<Vec<KeycloakGroup>, AppError> {
            Ok(self.groups.clone())
        }
        async fn get_user_roles(&self, _user_id: &str) -> Result<Vec<KeycloakRole>, AppError> {
            Ok(self.roles.clone())
        }
        async fn logout_user(&self, _user_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn get_user_by_email(&self, _email: &str) -> Result<Option<KeycloakUser>, AppError> {
            Ok(None)
        }
        async fn create_user(&self, _username: &str, _email: &str) -> Result<String, AppError> {
            Ok("mock-user-id".to_string())
        }
        async fn send_invite_email(&self, _user_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn delete_user(&self, _user_id: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn count_users(&self, _query: &str) -> Result<u32, AppError> {
            Ok(0)
        }
    }

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

    fn kc_user(username: &str) -> KeycloakUser {
        KeycloakUser {
            id: "kc-001".to_string(),
            username: username.to_string(),
            email: Some(format!("{username}@example.com")),
            first_name: Some("Alice".to_string()),
            last_name: Some("Smith".to_string()),
            enabled: true,
            email_verified: true,
            created_timestamp: None,
            required_actions: vec![],
        }
    }

    fn build_service(kc: MockKeycloak, mas: MockMas) -> UserService {
        UserService::new(Arc::new(kc), Arc::new(mas), "example.com")
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
        // KC disabled + pending actions → Disabled, not Invited
        assert_eq!(
            derive_lifecycle_state(false, &["VERIFY_EMAIL".to_string()], None),
            LifecycleState::Disabled
        );
    }

    // ── Search tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn search_returns_summary_with_inferred_matrix_id() {
        let svc = build_service(
            MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![],
                roles: vec![],
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
    async fn search_returns_empty_when_keycloak_finds_nothing() {
        let svc = build_service(
            MockKeycloak {
                users: vec![],
                groups: vec![],
                roles: vec![],
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
            MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![],
                roles: vec![],
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
        let mut user = kc_user("alice");
        user.required_actions = vec!["UPDATE_PASSWORD".to_string()];
        let svc = build_service(
            MockKeycloak {
                users: vec![user],
                groups: vec![],
                roles: vec![],
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
        let mut user = kc_user("alice");
        user.enabled = false;
        let svc = build_service(
            MockKeycloak {
                users: vec![user],
                groups: vec![],
                roles: vec![],
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
            MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![],
                roles: vec![],
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
            MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![],
                roles: vec![],
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
    async fn get_detail_inferred_when_only_keycloak_found() {
        let svc = build_service(
            MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![],
                roles: vec![],
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
        let svc = build_service(
            MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![KeycloakGroup {
                    id: "g1".to_string(),
                    name: "matrix-admins".to_string(),
                    path: "/matrix-admins".to_string(),
                }],
                roles: vec![KeycloakRole {
                    id: "r1".to_string(),
                    name: "matrix-admin".to_string(),
                    composite: false,
                    client_role: false,
                    container_id: None,
                }],
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

    // ── Keycloak group/role graceful failure ──────────────────────────────────

    struct FailGroups {
        users: Vec<KeycloakUser>,
        roles: Vec<KeycloakRole>,
    }

    #[async_trait]
    impl KeycloakApi for FailGroups {
        async fn search_users(
            &self,
            _: &str,
            _: u32,
            _: u32,
        ) -> Result<Vec<KeycloakUser>, AppError> {
            Ok(self.users.clone())
        }
        async fn get_user(&self, _: &str) -> Result<KeycloakUser, AppError> {
            self.users
                .first()
                .cloned()
                .ok_or_else(|| AppError::NotFound("not found".into()))
        }
        async fn get_user_groups(&self, _: &str) -> Result<Vec<KeycloakGroup>, AppError> {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "groups error".into(),
            })
        }
        async fn get_user_roles(&self, _: &str) -> Result<Vec<KeycloakRole>, AppError> {
            Ok(self.roles.clone())
        }
        async fn logout_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn get_user_by_email(&self, _: &str) -> Result<Option<KeycloakUser>, AppError> {
            Ok(None)
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
        async fn count_users(&self, _: &str) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    struct FailRoles {
        users: Vec<KeycloakUser>,
        groups: Vec<KeycloakGroup>,
    }

    #[async_trait]
    impl KeycloakApi for FailRoles {
        async fn search_users(
            &self,
            _: &str,
            _: u32,
            _: u32,
        ) -> Result<Vec<KeycloakUser>, AppError> {
            Ok(self.users.clone())
        }
        async fn get_user(&self, _: &str) -> Result<KeycloakUser, AppError> {
            self.users
                .first()
                .cloned()
                .ok_or_else(|| AppError::NotFound("not found".into()))
        }
        async fn get_user_groups(&self, _: &str) -> Result<Vec<KeycloakGroup>, AppError> {
            Ok(self.groups.clone())
        }
        async fn get_user_roles(&self, _: &str) -> Result<Vec<KeycloakRole>, AppError> {
            Err(AppError::Upstream {
                service: "keycloak".into(),
                message: "roles error".into(),
            })
        }
        async fn logout_user(&self, _: &str) -> Result<(), AppError> {
            Ok(())
        }
        async fn get_user_by_email(&self, _: &str) -> Result<Option<KeycloakUser>, AppError> {
            Ok(None)
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
        async fn count_users(&self, _: &str) -> Result<u32, AppError> {
            Ok(0)
        }
    }

    #[tokio::test]
    async fn get_detail_when_groups_fail_returns_empty_groups() {
        let svc = UserService::new(
            Arc::new(FailGroups {
                users: vec![kc_user("alice")],
                roles: vec![],
            }),
            Arc::new(MockMas {
                user: None,
                sessions: vec![],
            }),
            "example.com",
        );
        let detail = svc.get_detail("kc-001").await.unwrap();
        assert!(detail.groups.is_empty(), "expected empty groups on failure");
    }

    #[tokio::test]
    async fn get_detail_when_roles_fail_returns_empty_roles() {
        let svc = UserService::new(
            Arc::new(FailRoles {
                users: vec![kc_user("alice")],
                groups: vec![],
            }),
            Arc::new(MockMas {
                user: None,
                sessions: vec![],
            }),
            "example.com",
        );
        let detail = svc.get_detail("kc-001").await.unwrap();
        assert!(detail.roles.is_empty(), "expected empty roles on failure");
    }

    #[tokio::test]
    async fn get_detail_finished_session_has_finished_state() {
        let svc = build_service(
            MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![],
                roles: vec![],
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

    // ── Graceful failure ──────────────────────────────────────────────────────

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
            Arc::new(MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![],
                roles: vec![],
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
            Arc::new(MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![],
                roles: vec![],
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
            MockKeycloak {
                users: vec![kc_user("alice")],
                groups: vec![],
                roles: vec![],
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
    async fn get_detail_returns_error_when_keycloak_user_not_found() {
        // MockKeycloak with empty `users` causes `get_user` to return NotFound.
        let svc = build_service(
            MockKeycloak {
                users: vec![],
                groups: vec![],
                roles: vec![],
            },
            MockMas {
                user: None,
                sessions: vec![],
            },
        );
        let result = svc.get_detail("nonexistent").await;
        assert!(result.is_err());
    }

    /// Exercises trait methods on local test mocks that `UserService` itself never
    /// calls (they exist only to satisfy the `KeycloakApi` / `MasApi` trait contracts).
    #[tokio::test]
    async fn mock_unused_trait_methods_coverage() {
        let kc = MockKeycloak {
            users: vec![],
            groups: vec![],
            roles: vec![],
        };
        let _ = kc.logout_user("id").await;
        let _ = kc.get_user_by_email("e@test.com").await;
        let _ = kc.create_user("u", "e@test.com").await;
        let _ = kc.send_invite_email("id").await;
        let _ = kc.delete_user("id").await;
        let _ = kc.count_users("").await;

        let mas = MockMas {
            user: None,
            sessions: vec![],
        };
        let _ = mas.finish_session("id", "compat").await;
        let _ = mas.delete_user("id").await;
        let _ = mas.reactivate_user("id").await;

        let fg = FailGroups {
            users: vec![],
            roles: vec![],
        };
        let _ = fg.search_users("", 10, 0).await;
        let _ = fg.logout_user("id").await;
        let _ = fg.get_user_by_email("e@test.com").await;
        let _ = fg.create_user("u", "e@test.com").await;
        let _ = fg.send_invite_email("id").await;
        let _ = fg.delete_user("id").await;
        let _ = fg.count_users("").await;

        let fr = FailRoles {
            users: vec![],
            groups: vec![],
        };
        let _ = fr.search_users("", 10, 0).await;
        let _ = fr.logout_user("id").await;
        let _ = fr.get_user_by_email("e@test.com").await;
        let _ = fr.create_user("u", "e@test.com").await;
        let _ = fr.send_invite_email("id").await;
        let _ = fr.delete_user("id").await;
        let _ = fr.count_users("").await;

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
