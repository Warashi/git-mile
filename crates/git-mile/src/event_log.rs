//! Shared helpers for rendering task event logs.

use std::fmt::Write;

use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::EventId;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

/// Renderable representation of a task event.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Unique event identifier.
    pub id: EventId,
    /// Event timestamp.
    pub ts: OffsetDateTime,
    /// Actor who authored the event.
    pub actor: Actor,
    /// Short action label.
    pub action: String,
    /// Optional human-readable details.
    pub detail: Option<String>,
    /// Whether the detail should be rendered as multi-line description content.
    pub is_description: bool,
}

/// Convert raw events to display-friendly entries.
#[must_use]
pub fn entries_from_events(events: &[Event]) -> Vec<LogEntry> {
    events.iter().map(entry_from_event).collect()
}

/// Convert a single event to a display-friendly entry.
#[must_use]
pub fn entry_from_event(event: &Event) -> LogEntry {
    LogEntry {
        id: event.id,
        ts: event.ts,
        actor: event.actor.clone(),
        action: action_for_kind(&event.kind),
        detail: detail_for_kind(&event.kind),
        is_description: matches!(
            event.kind,
            EventKind::TaskDescriptionSet { .. }
                | EventKind::TaskCreated {
                    description: Some(_),
                    ..
                }
        ),
    }
}

/// Render a timestamp in RFC 3339 format.
#[must_use]
pub fn format_timestamp(ts: OffsetDateTime) -> String {
    ts.format(&Rfc3339).unwrap_or_else(|_| "-".to_owned())
}

/// Render an actor as `name <email>`.
#[must_use]
pub fn format_actor(actor: &Actor) -> String {
    format!("{} <{}>", actor.name, actor.email)
}

/// Collapse multiline text into a single line separated by spaces.
#[must_use]
pub fn single_line_detail(detail: &str) -> String {
    detail.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Truncate text to at most `max_chars`, adding `...` when shortened.
#[must_use]
pub fn truncate_detail(detail: &str, max_chars: usize) -> String {
    if detail.chars().count() <= max_chars {
        return detail.to_owned();
    }

    let mut truncated = String::new();
    for ch in detail.chars().take(max_chars.saturating_sub(3)) {
        truncated.push(ch);
    }
    truncated.push_str("...");
    truncated
}

fn action_for_kind(kind: &EventKind) -> String {
    match kind {
        EventKind::TaskCreated { .. } => "Task created",
        EventKind::TaskStateSet { .. } => "State set",
        EventKind::TaskStateCleared => "State cleared",
        EventKind::TaskTitleSet { .. } => "Title set",
        EventKind::TaskDescriptionSet { .. } => "Description set",
        EventKind::LabelsAdded { .. } => "Labels added",
        EventKind::LabelsRemoved { .. } => "Labels removed",
        EventKind::AssigneesAdded { .. } => "Assignees added",
        EventKind::AssigneesRemoved { .. } => "Assignees removed",
        EventKind::CommentAdded { .. } => "Comment added",
        EventKind::CommentUpdated { .. } => "Comment updated",
        EventKind::ChildLinked { .. } => "Child linked",
        EventKind::ChildUnlinked { .. } => "Child unlinked",
        EventKind::RelationAdded { .. } => "Relation added",
        EventKind::RelationRemoved { .. } => "Relation removed",
    }
    .to_owned()
}

fn detail_for_kind(kind: &EventKind) -> Option<String> {
    match kind {
        EventKind::TaskCreated {
            title,
            labels,
            assignees,
            description,
            state,
            state_kind,
        } => {
            let mut parts = vec![format!("title: {title}")];
            if let Some(state) = state {
                let mut state_line = format!("state: {state}");
                if let Some(kind) = state_kind {
                    let _ = write!(&mut state_line, " ({})", kind.as_str());
                }
                parts.push(state_line);
            }
            if !labels.is_empty() {
                parts.push(format!("labels: {}", labels.join(", ")));
            }
            if !assignees.is_empty() {
                parts.push(format!("assignees: {}", assignees.join(", ")));
            }
            let summary = join_parts(parts);
            description.as_ref().map_or(summary.clone(), |desc| {
                summary
                    .map_or_else(|| desc.clone(), |prefix| format!("{prefix}\n{desc}"))
                    .into()
            })
        }
        EventKind::TaskStateSet { state, state_kind } => {
            let mut line = format!("state: {state}");
            if let Some(kind) = state_kind {
                let _ = write!(&mut line, " ({})", kind.as_str());
            }
            Some(line)
        }
        EventKind::TaskStateCleared => None,
        EventKind::TaskTitleSet { title } => Some(format!("title: {title}")),
        EventKind::TaskDescriptionSet { description } => description
            .as_deref()
            .map(ToOwned::to_owned)
            .or_else(|| Some("description cleared".to_owned())),
        EventKind::LabelsAdded { labels } => Some(format!("added: {}", labels.join(", "))),
        EventKind::LabelsRemoved { labels } => Some(format!("removed: {}", labels.join(", "))),
        EventKind::AssigneesAdded { assignees } => Some(format!("added: {}", assignees.join(", "))),
        EventKind::AssigneesRemoved { assignees } => Some(format!("removed: {}", assignees.join(", "))),
        EventKind::CommentAdded { body_md, .. } | EventKind::CommentUpdated { body_md, .. } => {
            Some(body_md.clone())
        }
        EventKind::ChildLinked { parent, child } | EventKind::ChildUnlinked { parent, child } => {
            Some(format!("parent: {parent}, child: {child}"))
        }
        EventKind::RelationAdded { kind, target } | EventKind::RelationRemoved { kind, target } => {
            Some(format!("kind: {kind}, target: {target}"))
        }
    }
}

fn join_parts(parts: Vec<String>) -> Option<String> {
    let filtered: Vec<String> = parts.into_iter().filter(|part| !part.is_empty()).collect();
    if filtered.is_empty() {
        None
    } else {
        Some(filtered.join(" | "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::StateKind;
    use git_mile_core::event::EventKind;
    use git_mile_core::id::TaskId;

    fn sample_actor() -> Actor {
        Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        }
    }

    #[test]
    fn entry_from_event_sets_action_and_detail() {
        let task = TaskId::new();
        let actor = sample_actor();
        let event = Event::new(
            task,
            &actor,
            EventKind::TaskStateSet {
                state: "state/done".into(),
                state_kind: Some(StateKind::Done),
            },
        );

        let entry = entry_from_event(&event);

        assert_eq!(entry.id, event.id);
        assert_eq!(entry.action, "State set");
        assert_eq!(entry.detail.as_deref(), Some("state: state/done (done)"));
        assert_eq!(entry.actor.email, actor.email);
    }

    #[test]
    fn single_line_detail_collapses_whitespace() {
        let raw = "line one\nline   two\tthree";
        assert_eq!(single_line_detail(raw), "line one line two three");
    }

    #[test]
    fn truncate_detail_shortens_long_text() {
        let detail = "abcdefghij";
        assert_eq!(truncate_detail(detail, 5), "ab...");
        assert_eq!(truncate_detail(detail, 10), detail);
    }

    #[test]
    fn description_detail_is_marked_multiline() {
        let task = TaskId::new();
        let actor = sample_actor();
        let event = Event::new(
            task,
            &actor,
            EventKind::TaskDescriptionSet {
                description: Some("line1\nline2".into()),
            },
        );

        let entry = entry_from_event(&event);
        assert!(entry.is_description);
        assert_eq!(entry.detail.as_deref(), Some("line1\nline2"));
    }

    #[test]
    fn task_created_description_is_multiline() {
        let task = TaskId::new();
        let actor = sample_actor();
        let event = Event::new(
            task,
            &actor,
            EventKind::TaskCreated {
                title: "t".into(),
                labels: vec![],
                assignees: vec![],
                description: Some("first\nsecond".into()),
                state: None,
                state_kind: None,
            },
        );

        let entry = entry_from_event(&event);
        assert!(entry.is_description);
        assert_eq!(entry.detail.as_deref(), Some("title: t\nfirst\nsecond"));
    }
}
