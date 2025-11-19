use crate::TaskSnapshot;

/// Case-insensitive substring matcher for task fields.
pub struct TextMatcher {
    needle: String,
}

impl TextMatcher {
    /// Normalize a query string into a matcher. Returns `None` for blank inputs.
    pub fn new(query: &str) -> Option<Self> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return None;
        }
        Some(Self {
            needle: trimmed.to_ascii_lowercase(),
        })
    }

    /// Determine whether any textual field on the snapshot contains the query.
    pub fn matches(&self, snapshot: &TaskSnapshot) -> bool {
        self.matches_field(&snapshot.title)
            || self.matches_field(&snapshot.description)
            || snapshot
                .state
                .as_deref()
                .is_some_and(|state| self.matches_field(state))
            || snapshot.labels.iter().any(|label| self.matches_field(label))
            || snapshot
                .assignees
                .iter()
                .any(|assignee| self.matches_field(assignee))
    }

    fn matches_field(&self, value: &str) -> bool {
        value.to_ascii_lowercase().contains(&self.needle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TaskSnapshot;

    #[test]
    fn matcher_skips_blank_queries() {
        assert!(TextMatcher::new("").is_none());
        assert!(TextMatcher::new("   ").is_none());
        assert!(TextMatcher::new("\n").is_none());
    }

    #[test]
    fn matcher_finds_text_across_fields() {
        let mut snapshot = TaskSnapshot {
            title: "Lamport Clock Work".into(),
            description: "Refactor filters".into(),
            state: Some("STATE/TODO".into()),
            ..TaskSnapshot::default()
        };
        snapshot.labels.insert("type/feature".into());
        snapshot.assignees.insert("Alice".into());

        let matcher = TextMatcher::new("clock")
            .unwrap_or_else(|| panic!("matcher must exist for queries with content"));
        assert!(matcher.matches(&snapshot));

        let matcher = TextMatcher::new("refactor")
            .unwrap_or_else(|| panic!("matcher must exist for queries with content"));
        assert!(matcher.matches(&snapshot));

        let matcher = TextMatcher::new("state/ToDo")
            .unwrap_or_else(|| panic!("matcher must exist for queries with content"));
        assert!(matcher.matches(&snapshot));

        let matcher = TextMatcher::new("type/FEATURE")
            .unwrap_or_else(|| panic!("matcher must exist for queries with content"));
        assert!(matcher.matches(&snapshot));

        let matcher = TextMatcher::new("alice")
            .unwrap_or_else(|| panic!("matcher must exist for queries with content"));
        assert!(matcher.matches(&snapshot));
    }

    #[test]
    fn matcher_respects_case_insensitive_search() {
        let snapshot = TaskSnapshot {
            title: "Improve CLI".into(),
            ..TaskSnapshot::default()
        };

        let matcher =
            TextMatcher::new("cli").unwrap_or_else(|| panic!("matcher must exist for queries with content"));
        assert!(matcher.matches(&snapshot));

        let matcher =
            TextMatcher::new("CLI").unwrap_or_else(|| panic!("matcher must exist for queries with content"));
        assert!(matcher.matches(&snapshot));

        let missing =
            TextMatcher::new("api").unwrap_or_else(|| panic!("matcher must exist for queries with content"));
        assert!(!missing.matches(&snapshot));
    }
}
