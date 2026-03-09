use crate::models::group_mapping::GroupMapping;

/// Evaluates group membership policy to determine required Matrix room memberships.
///
/// Wraps the configured group → room mappings and provides query methods so
/// callers never iterate the raw mapping slice directly.
#[derive(Debug, Clone, Default)]
pub struct PolicyEngine {
    mappings: Vec<GroupMapping>,
}

impl PolicyEngine {
    /// Create a new `PolicyEngine` from the provided group → room mappings.
    pub fn new(mappings: Vec<GroupMapping>) -> Self {
        Self { mappings }
    }

    /// Returns all room IDs the user should be a member of, given their current groups.
    pub fn required_rooms_for(&self, user_groups: &[String]) -> Vec<String> {
        self.mappings
            .iter()
            .filter(|m| user_groups.iter().any(|g| g == &m.keycloak_group))
            .map(|m| m.matrix_room_id.clone())
            .collect()
    }

    /// Returns all mappings relevant to the given user's groups.
    pub fn mappings_for<'a>(&'a self, user_groups: &[String]) -> Vec<&'a GroupMapping> {
        self.mappings
            .iter()
            .filter(|m| user_groups.iter().any(|g| g == &m.keycloak_group))
            .collect()
    }

    /// Returns all configured mappings regardless of user groups.
    pub fn all_mappings(&self) -> &[GroupMapping] {
        &self.mappings
    }

    /// Returns `true` if no mappings are configured.
    pub fn is_empty(&self) -> bool {
        self.mappings.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mapping(group: &str, room: &str) -> GroupMapping {
        GroupMapping {
            keycloak_group: group.to_string(),
            matrix_room_id: room.to_string(),
        }
    }

    // ── PolicyEngine ──────────────────────────────────────────────────────────

    #[test]
    fn required_rooms_no_groups_returns_empty() {
        let engine = PolicyEngine::new(vec![mapping("staff", "!room1:example.com")]);
        let rooms = engine.required_rooms_for(&[]);
        assert!(rooms.is_empty());
    }

    #[test]
    fn required_rooms_matching_group_returns_room() {
        let engine = PolicyEngine::new(vec![mapping("staff", "!room1:example.com")]);
        let rooms = engine.required_rooms_for(&["staff".to_string()]);
        assert_eq!(rooms, vec!["!room1:example.com"]);
    }

    #[test]
    fn required_rooms_multiple_matches_returns_all() {
        let engine = PolicyEngine::new(vec![
            mapping("staff", "!room1:example.com"),
            mapping("admins", "!room2:example.com"),
        ]);
        let mut rooms = engine.required_rooms_for(&["staff".to_string(), "admins".to_string()]);
        rooms.sort();
        assert_eq!(rooms, vec!["!room1:example.com", "!room2:example.com"]);
    }

    #[test]
    fn required_rooms_no_match_returns_empty() {
        let engine = PolicyEngine::new(vec![mapping("staff", "!room1:example.com")]);
        let rooms = engine.required_rooms_for(&["other-group".to_string()]);
        assert!(rooms.is_empty());
    }

    #[test]
    fn mappings_for_returns_only_relevant_mappings() {
        let engine = PolicyEngine::new(vec![
            mapping("staff", "!room1:example.com"),
            mapping("admins", "!room2:example.com"),
        ]);
        let relevant = engine.mappings_for(&["staff".to_string()]);
        assert_eq!(relevant.len(), 1);
        assert_eq!(relevant[0].keycloak_group, "staff");
        assert_eq!(relevant[0].matrix_room_id, "!room1:example.com");
    }

    #[test]
    fn all_mappings_returns_every_entry() {
        let engine = PolicyEngine::new(vec![
            mapping("staff", "!room1:example.com"),
            mapping("admins", "!room2:example.com"),
        ]);
        assert_eq!(engine.all_mappings().len(), 2);
    }

    #[test]
    fn is_empty_true_when_no_mappings() {
        let engine = PolicyEngine::default();
        assert!(engine.is_empty());
    }

    #[test]
    fn is_empty_false_when_mappings_present() {
        let engine = PolicyEngine::new(vec![mapping("staff", "!room1:example.com")]);
        assert!(!engine.is_empty());
    }
}
