use serde::{Deserialize, Serialize};

/// A reusable onboarding configuration that assigns Keycloak groups and
/// realm roles when inviting a new user.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OnboardingTemplate {
    pub name: String,
    pub description: String,
    pub groups: Vec<String>,
    pub roles: Vec<String>,
}

/// Loads onboarding templates from a JSON file.
/// Returns an empty vec if the file does not exist.
pub fn load_templates(path: &std::path::Path) -> Result<Vec<OnboardingTemplate>, String> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let contents =
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read templates file: {e}"))?;
    if contents.trim().is_empty() {
        return Ok(vec![]);
    }
    serde_json::from_str(&contents).map_err(|e| format!("Failed to parse templates file: {e}"))
}

// TODO: Use atomic write (write to temp file + rename) to prevent data
// corruption on concurrent requests. Low priority for single-admin use.
/// Saves onboarding templates to a JSON file.
pub fn save_templates(
    path: &std::path::Path,
    templates: &[OnboardingTemplate],
) -> Result<(), String> {
    let json = serde_json::to_string_pretty(templates)
        .map_err(|e| format!("Failed to serialize templates: {e}"))?;
    std::fs::write(path, json).map_err(|e| format!("Failed to write templates file: {e}"))
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use tempfile::NamedTempFile;

    use super::*;

    // ── load_templates ────────────────────────────────────────────────

    #[test]
    fn load_missing_file_returns_empty() {
        let path = std::path::Path::new("/tmp/nonexistent-onboarding-templates-test.json");
        let result = load_templates(path).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn load_empty_file_returns_empty() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "").unwrap();
        let result = load_templates(f.path()).unwrap();
        assert!(result.is_empty());
    }

    #[test]
    fn load_valid_file_parses_templates() {
        let mut f = NamedTempFile::new().unwrap();
        write!(
            f,
            r#"[{{"name":"Staff","description":"Full access","groups":["staff"],"roles":["admin"]}}]"#
        )
        .unwrap();
        let result = load_templates(f.path()).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Staff");
        assert_eq!(result[0].groups, vec!["staff"]);
        assert_eq!(result[0].roles, vec!["admin"]);
    }

    #[test]
    fn load_invalid_json_returns_error() {
        let mut f = NamedTempFile::new().unwrap();
        write!(f, "not json").unwrap();
        assert!(load_templates(f.path()).is_err());
    }

    // ── save_templates ────────────────────────────────────────────────

    #[test]
    fn save_and_reload_round_trips() {
        let f = NamedTempFile::new().unwrap();
        let templates = vec![OnboardingTemplate {
            name: "Test".to_string(),
            description: "Desc".to_string(),
            groups: vec!["g1".to_string()],
            roles: vec!["r1".to_string()],
        }];
        save_templates(f.path(), &templates).unwrap();
        let loaded = load_templates(f.path()).unwrap();
        assert_eq!(templates, loaded);
    }
}
