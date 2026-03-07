use std::sync::Arc;

use crate::{
    clients::{KeycloakApi, MasApi},
    error::AppError,
    models::unified::{UnifiedSession, UnifiedUserDetail, UnifiedUserSummary},
    services::identity_mapper::IdentityMapper,
};


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

    /// Search Keycloak for users matching `query`. Returns lightweight summaries
    /// with derived Matrix IDs but does not fan out to MAS per result.
    pub async fn search(&self, query: &str) -> Result<Vec<UnifiedUserSummary>, AppError> {
        let users = self.keycloak.search_users(query).await?;

        let summaries = users
            .into_iter()
            .map(|u| {
                let mapped = self.mapper.map_summary_only(u);
                UnifiedUserSummary {
                    keycloak_id: mapped.keycloak_user.id,
                    username: mapped.keycloak_user.username,
                    email: mapped.keycloak_user.email,
                    enabled: mapped.keycloak_user.enabled,
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

        let mas_user = self.mas.get_user_by_username(&user.username).await
            .unwrap_or_else(|e| {
                tracing::warn!(error = %e, "MAS user lookup failed");
                None
            });

        let mas_user_id = mas_user.as_ref().map(|u| u.id.clone());

        let mapped = self.mapper.map(user.clone(), mas_user_id.clone());

        let sessions: Vec<UnifiedSession> = match &mas_user_id {
            Some(id) => self.mas.list_sessions(id).await
                .unwrap_or_else(|e| {
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

        let matrix_id = Some(inferred_matrix_id);

        Ok(UnifiedUserDetail {
            keycloak_id: user.id,
            username: user.username,
            email: user.email,
            first_name: user.first_name,
            last_name: user.last_name,
            enabled: user.enabled,
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
    use async_trait::async_trait;
    use crate::models::{
        keycloak::{KeycloakGroup, KeycloakRole, KeycloakUser},
        mas::{MasSession, MasUser},
        unified::CorrelationStatus,
    };

    // ── Manual mock implementations ───────────────────────────────────────────

    struct MockKeycloak {
        users: Vec<KeycloakUser>,
        groups: Vec<KeycloakGroup>,
        roles: Vec<KeycloakRole>,
    }

    #[async_trait]
    impl KeycloakApi for MockKeycloak {
        async fn search_users(&self, _query: &str) -> Result<Vec<KeycloakUser>, AppError> {
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
        async fn finish_session(&self, _session_id: &str, _session_type: &str) -> Result<(), AppError> {
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
        }
    }

    fn build_service(kc: MockKeycloak, mas: MockMas) -> UserService {
        UserService::new(Arc::new(kc), Arc::new(mas), "example.com")
    }

    // ── Search tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn search_returns_summary_with_inferred_matrix_id() {
        let svc = build_service(
            MockKeycloak { users: vec![kc_user("alice")], groups: vec![], roles: vec![] },
            MockMas { user: None, sessions: vec![] },
        );

        let results = svc.search("alice").await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].username, "alice");
        assert_eq!(results[0].inferred_matrix_id.as_deref(), Some("@alice:example.com"));
        assert_eq!(results[0].correlation_status, CorrelationStatus::Inferred);
    }

    #[tokio::test]
    async fn search_returns_empty_when_keycloak_finds_nothing() {
        let svc = build_service(
            MockKeycloak { users: vec![], groups: vec![], roles: vec![] },
            MockMas { user: None, sessions: vec![] },
        );

        let results = svc.search("nobody").await.unwrap();
        assert!(results.is_empty());
    }

    // ── Detail tests ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_detail_confirmed_when_mas_found() {
        let svc = build_service(
            MockKeycloak { users: vec![kc_user("alice")], groups: vec![], roles: vec![] },
            MockMas {
                user: Some(MasUser { id: "mas-001".to_string(), username: "alice".to_string() }),
                sessions: vec![],
            },
        );

        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.correlation_status, CorrelationStatus::Confirmed);
        assert_eq!(detail.matrix_id.as_deref(), Some("@alice:example.com"));
    }

    #[tokio::test]
    async fn get_detail_inferred_when_only_keycloak_found() {
        let svc = build_service(
            MockKeycloak { users: vec![kc_user("alice")], groups: vec![], roles: vec![] },
            MockMas { user: None, sessions: vec![] },
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
            MockMas { user: None, sessions: vec![] },
        );

        let detail = svc.get_detail("kc-001").await.unwrap();
        assert_eq!(detail.groups, vec!["matrix-admins"]);
        assert_eq!(detail.roles, vec!["matrix-admin"]);
    }

    #[tokio::test]
    async fn get_detail_includes_sessions() {
        let svc = build_service(
            MockKeycloak { users: vec![kc_user("alice")], groups: vec![], roles: vec![] },
            MockMas {
                user: Some(MasUser { id: "mas-001".to_string(), username: "alice".to_string() }),
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
}
