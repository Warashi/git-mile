use std::fmt::{self, Display};

use git_mile_core::id::TaskId;
use git_mile_core::{StateKind, TaskFilter, TaskFilterBuilder as CoreTaskFilterBuilder, UpdatedFilter};
use thiserror::Error;
use time::{OffsetDateTime, UtcOffset, format_description::well_known::Rfc3339};

/// Error type returned while constructing task filters from user-facing inputs.
#[derive(Debug, Error)]
pub enum FilterBuildError {
    #[error("invalid state kind: {token}")]
    InvalidStateKind { token: String },
    #[error("invalid {field} timestamp: {source}")]
    InvalidTimestamp {
        field: &'static str,
        #[source]
        source: time::error::Parse,
    },
}

/// Result alias for filter construction helpers.
pub type FilterBuildResult<T> = Result<T, FilterBuildError>;

/// Builder that accepts user-facing strings and normalizes them into [`TaskFilter`] values.
#[derive(Debug, Clone, Default)]
pub struct TaskFilterBuilder {
    states: Vec<String>,
    include_state_kinds: Vec<StateKind>,
    exclude_state_kinds: Vec<StateKind>,
    labels: Vec<String>,
    assignees: Vec<String>,
    parents: Vec<TaskId>,
    children: Vec<TaskId>,
    text: Option<String>,
    updated_since: Option<OffsetDateTime>,
    updated_until: Option<OffsetDateTime>,
}

impl TaskFilterBuilder {
    /// Create an empty builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Extend the workflow state filter.
    #[must_use]
    pub fn with_states(mut self, states: &[String]) -> Self {
        self.states.extend(states.iter().cloned());
        self
    }

    /// Extend the required label list (logical AND).
    #[must_use]
    pub fn with_labels(mut self, labels: &[String]) -> Self {
        self.labels.extend(labels.iter().cloned());
        self
    }

    /// Extend the assignee filters (logical OR).
    #[must_use]
    pub fn with_assignees(mut self, assignees: &[String]) -> Self {
        self.assignees.extend(assignees.iter().cloned());
        self
    }

    /// Add required parent identifiers.
    #[must_use]
    pub fn with_parents(mut self, parents: &[TaskId]) -> Self {
        self.parents.extend(parents.iter().copied());
        self
    }

    /// Add required child identifiers.
    #[must_use]
    pub fn with_children(mut self, children: &[TaskId]) -> Self {
        self.children.extend(children.iter().copied());
        self
    }

    /// Configure state kind include/exclude clauses.
    ///
    /// # Errors
    /// Returns an error if any of the provided tokens cannot be mapped to a known state kind.
    pub fn with_state_kinds(mut self, include: &[String], exclude: &[String]) -> FilterBuildResult<Self> {
        self.include_state_kinds.extend(parse_state_kind_tokens(include)?);
        self.exclude_state_kinds.extend(parse_state_kind_tokens(exclude)?);
        Ok(self)
    }

    /// Configure the optional search text (whitespace-only inputs become `None`).
    #[must_use]
    pub fn with_text(mut self, text: Option<String>) -> Self {
        self.text = text.and_then(|raw| {
            let trimmed = raw.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        });
        self
    }

    /// Configure the updated timestamp bounds using RFC3339 strings.
    ///
    /// # Errors
    /// Returns an error if either timestamp fails to parse.
    pub fn with_time_range(
        mut self,
        since: Option<String>,
        until: Option<String>,
    ) -> FilterBuildResult<Self> {
        self.updated_since = parse_optional_timestamp("updated_since", since)?;
        self.updated_until = parse_optional_timestamp("updated_until", until)?;
        Ok(self)
    }

    /// Configure the updated timestamp bounds using already parsed values.
    #[must_use]
    pub fn with_time_range_values(
        mut self,
        since: Option<OffsetDateTime>,
        until: Option<OffsetDateTime>,
    ) -> Self {
        self.updated_since = since.map(normalize_timestamp);
        self.updated_until = until.map(normalize_timestamp);
        self
    }

    /// Build the final [`TaskFilter`].
    #[must_use]
    pub fn build(self) -> TaskFilter {
        let mut builder = CoreTaskFilterBuilder::new()
            .states(self.states)
            .labels(self.labels)
            .assignees(self.assignees)
            .parents(self.parents)
            .children(self.children)
            .include_state_kinds(self.include_state_kinds)
            .exclude_state_kinds(self.exclude_state_kinds);

        if let Some(text) = self.text {
            builder = builder.text(text);
        }

        if self.updated_since.is_some() || self.updated_until.is_some() {
            builder = builder.updated(UpdatedFilter {
                since: self.updated_since,
                until: self.updated_until,
            });
        }

        builder.build()
    }
}

/// Convert arbitrary tokens into [`StateKind`] values.
///
/// # Errors
/// Returns an error if any token does not match a valid state kind.
pub fn parse_state_kind_tokens(tokens: &[String]) -> FilterBuildResult<Vec<StateKind>> {
    tokens
        .iter()
        .map(|token| {
            let normalized = token.trim().to_ascii_lowercase().replace(['-', ' '], "_");
            match normalized.as_str() {
                "todo" => Ok(StateKind::Todo),
                "in_progress" | "inprogress" => Ok(StateKind::InProgress),
                "blocked" => Ok(StateKind::Blocked),
                "done" => Ok(StateKind::Done),
                "backlog" => Ok(StateKind::Backlog),
                _ => Err(FilterBuildError::InvalidStateKind {
                    token: token.to_string(),
                }),
            }
        })
        .collect()
}

/// Parse an RFC3339 timestamp string.
///
/// # Errors
/// Returns an error if the string does not conform to RFC3339.
pub fn parse_timestamp(s: &str) -> Result<OffsetDateTime, time::error::Parse> {
    OffsetDateTime::parse(s.trim(), &Rfc3339)
}

/// Normalize timestamps to UTC to avoid offset mismatches across interfaces.
#[must_use]
pub const fn normalize_timestamp(dt: OffsetDateTime) -> OffsetDateTime {
    dt.to_offset(UtcOffset::UTC)
}

fn parse_optional_timestamp(
    field: &'static str,
    value: Option<String>,
) -> FilterBuildResult<Option<OffsetDateTime>> {
    let Some(raw) = value else {
        return Ok(None);
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let parsed =
        parse_timestamp(trimmed).map_err(|source| FilterBuildError::InvalidTimestamp { field, source })?;
    Ok(Some(normalize_timestamp(parsed)))
}

impl FilterBuildError {
    /// Convert the error into a message that is friendly for end-users.
    #[must_use]
    pub fn describe_user_facing(&self) -> String {
        match self {
            Self::InvalidStateKind { token } => format!("state_kind の指定が不正です: {token}"),
            Self::InvalidTimestamp { field, .. } => {
                format!("{field} の時刻フォーマットが不正です (RFC3339 必須)")
            }
        }
    }
}

impl Display for TaskFilterBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskFilterBuilder")
            .field("states", &self.states)
            .field("include_state_kinds", &self.include_state_kinds)
            .field("exclude_state_kinds", &self.exclude_state_kinds)
            .field("labels", &self.labels)
            .field("assignees", &self.assignees)
            .field("parents", &self.parents)
            .field("children", &self.children)
            .field("text", &self.text)
            .field("updated_since", &self.updated_since)
            .field("updated_until", &self.updated_until)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::{fmt::Display, str::FromStr};

    use super::*;

    fn ok<T, E: Display>(result: Result<T, E>, context: &str) -> T {
        result.unwrap_or_else(|err| panic!("{context}: {err}"))
    }

    fn datetime(input: &str) -> OffsetDateTime {
        ok(parse_timestamp(input), "valid timestamp")
    }

    #[test]
    fn test_parse_state_kind_tokens() {
        let tokens = vec!["todo".into(), "In-Progress ".into(), "DONE".into()];
        let parsed = ok(parse_state_kind_tokens(&tokens), "parse state kinds");
        assert_eq!(
            parsed,
            vec![StateKind::Todo, StateKind::InProgress, StateKind::Done]
        );
    }

    #[test]
    fn test_parse_timestamp() {
        let parsed = ok(parse_timestamp("2025-01-01T09:00:00+09:00"), "parse timestamp");
        assert_eq!(parsed.year(), 2025);
        let offset = ok(time::UtcOffset::from_hms(9, 0, 0), "offset parse");
        assert_eq!(parsed.offset(), offset);
    }

    #[test]
    fn test_normalize_timestamp() {
        let original = datetime("2025-01-01T09:00:00+09:00");
        let normalized = normalize_timestamp(original);
        assert_eq!(normalized.offset(), UtcOffset::UTC);
        assert_eq!(normalized.unix_timestamp(), original.unix_timestamp());
    }

    #[test]
    fn test_state_kind_include_exclude() {
        let builder = TaskFilterBuilder::new()
            .with_state_kinds(
                &[String::from("todo"), String::from("in_progress")],
                &[String::from("done")],
            )
            .unwrap_or_else(|err| panic!("state kinds parse: {err}"));
        assert_eq!(builder.include_state_kinds.len(), 2);
        assert_eq!(builder.exclude_state_kinds.len(), 1);
    }

    #[test]
    fn test_filter_builder_full_workflow() {
        use git_mile_core::id::TaskId;

        let parent = ok(
            TaskId::from_str("019a6ff3-119f-7661-869e-2a6c4fca5c4f"),
            "parse parent id",
        );
        let child = ok(
            TaskId::from_str("019a6ff5-7c1f-7643-80c8-28f4c7d1754e"),
            "parse child id",
        );
        let states = vec!["state/todo".into()];
        let labels = vec!["type/doc".into()];
        let assignees = vec!["alice".into()];
        let parents = vec![parent];
        let children = vec![child];
        let include_kinds = vec!["todo".into()];
        let exclude_kinds = vec!["done".into()];
        let filter = TaskFilterBuilder::new()
            .with_states(&states)
            .with_labels(&labels)
            .with_assignees(&assignees)
            .with_parents(&parents)
            .with_children(&children)
            .with_state_kinds(&include_kinds, &exclude_kinds)
            .unwrap_or_else(|err| panic!("state kinds parse: {err}"))
            .with_text(Some(" Panic ".into()))
            .with_time_range(
                Some("2025-01-01T00:00:00Z".into()),
                Some("2025-01-02T00:00:00Z".into()),
            )
            .unwrap_or_else(|err| panic!("time range parse: {err}"))
            .build();

        assert!(filter.states.contains("state/todo"));
        assert!(filter.labels.contains("type/doc"));
        assert!(filter.assignees.contains("alice"));
        assert!(filter.parents.contains(&parent));
        assert!(filter.children.contains(&child));
        assert_eq!(filter.text.as_deref(), Some("panic"));
        assert!(filter.state_kinds.include.contains(&StateKind::Todo));
        assert!(filter.state_kinds.exclude.contains(&StateKind::Done));
        let updated = filter
            .updated
            .unwrap_or_else(|| panic!("expected updated filter"));
        assert_eq!(updated.since, Some(datetime("2025-01-01T00:00:00Z")));
        assert_eq!(updated.until, Some(datetime("2025-01-02T00:00:00Z")));
    }
}
