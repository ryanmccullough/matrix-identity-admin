/// The outcome of a multi-step workflow.
///
/// A workflow may succeed overall while some non-fatal steps fail.
/// `warnings` collects those partial failures so callers can surface
/// them to the admin without treating the whole operation as a hard error.
///
/// Example: `disable_user` revokes MAS sessions before disabling the
/// Keycloak account. Individual session revocations are non-fatal — if
/// some fail the account is still disabled, but the admin should know
/// which sessions were not cleaned up.
#[derive(Debug, Default)]
pub struct WorkflowOutcome {
    pub warnings: Vec<String>,
}

impl WorkflowOutcome {
    /// Create an outcome with no warnings.
    pub fn ok() -> Self {
        Self::default()
    }

    /// Append a non-fatal warning message.
    pub fn add_warning(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }

    /// Returns true if any non-fatal warnings were collected.
    pub fn has_warnings(&self) -> bool {
        !self.warnings.is_empty()
    }

    /// Join all warnings into a single string suitable for a flash message.
    pub fn warning_summary(&self) -> String {
        self.warnings.join("; ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ok_has_no_warnings() {
        let outcome = WorkflowOutcome::ok();
        assert!(!outcome.has_warnings());
        assert!(outcome.warning_summary().is_empty());
    }

    #[test]
    fn add_warning_accumulates() {
        let mut outcome = WorkflowOutcome::ok();
        outcome.add_warning("first failure");
        outcome.add_warning("second failure");
        assert!(outcome.has_warnings());
        assert_eq!(outcome.warnings.len(), 2);
        assert_eq!(outcome.warning_summary(), "first failure; second failure");
    }
}
