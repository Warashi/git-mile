//! Domain types & replay logic for git-mile events.

/// Event payload definitions.
pub mod event;
/// Identifier types.
pub mod id;

use crate::event::{Event, EventKind};
use crate::id::TaskId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use time::format_description::well_known::Rfc3339;

/// Materialized view of a task by replaying events.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TaskSnapshot {
    /// Identifier of the task.
    pub id: TaskId,
    /// Human-readable title.
    pub title: String,
    /// Current state label.
    pub state: Option<String>,
    /// Assigned labels.
    pub labels: BTreeSet<String>,
    /// Current assignees.
    pub assignees: BTreeSet<String>,
    /// Latest description in Markdown.
    pub description: String,
    /// Child tasks.
    pub children: BTreeSet<TaskId>,
    /// Parent tasks.
    pub parents: BTreeSet<TaskId>,
    /// Generic relation buckets.
    pub relates: BTreeMap<String, BTreeSet<TaskId>>,
    /// RFC 3339 timestamp of the most recent event.
    pub updated_rfc3339: Option<String>,
}

impl TaskSnapshot {
    /// Apply a single event (LWW per field).
    pub fn apply(&mut self, ev: &Event) {
        self.id = ev.task;

        match &ev.kind {
            EventKind::TaskCreated {
                title,
                labels,
                assignees,
                description,
                state,
            } => {
                self.title.clone_from(title);
                self.description = description.clone().unwrap_or_default();
                if let Some(st) = state {
                    self.state = Some(st.clone());
                }
                self.labels.extend(labels.iter().cloned());
                self.assignees.extend(assignees.iter().cloned());
            }
            EventKind::TaskStateSet { state } => {
                self.state = Some(state.clone());
            }
            EventKind::LabelsAdded { labels } => {
                self.labels.extend(labels.iter().cloned());
            }
            EventKind::LabelsRemoved { labels } => {
                for l in labels {
                    self.labels.remove(l);
                }
            }
            EventKind::AssigneesAdded { assignees } => {
                self.assignees.extend(assignees.iter().cloned());
            }
            EventKind::AssigneesRemoved { assignees } => {
                for a in assignees {
                    self.assignees.remove(a);
                }
            }
            EventKind::CommentAdded { .. } => {
                // Snapshot ignores comment bodies; only updated timestamp is affected.
            }
            EventKind::ChildLinked { parent, child } => {
                if &self.id == parent {
                    self.children.insert(*child);
                }
                if &self.id == child {
                    self.parents.insert(*parent);
                }
            }
            EventKind::ChildUnlinked { parent, child } => {
                if &self.id == parent {
                    self.children.remove(child);
                }
                if &self.id == child {
                    self.parents.remove(parent);
                }
            }
            EventKind::RelationAdded { kind, target } => {
                self.relates.entry(kind.clone()).or_default().insert(*target);
            }
            EventKind::RelationRemoved { kind, target } => {
                if let Some(s) = self.relates.get_mut(kind) {
                    s.remove(target);
                    if s.is_empty() {
                        self.relates.remove(kind);
                    }
                }
            }
        }

        self.updated_rfc3339 = ev.ts.format(&Rfc3339).ok();
    }

    /// Replay many events in time order.
    #[must_use]
    pub fn replay(mut events: Vec<Event>) -> Self {
        events.sort_by_key(|e| (e.ts, e.id));
        let mut snap = Self::default();
        for e in &events {
            snap.apply(e);
        }
        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Actor, Event, EventKind};
    use time::{Duration, OffsetDateTime};

    #[test]
    fn apply_and_replay_updates_snapshot() {
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let mut created = Event::new(
            task,
            actor.clone(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: vec!["type/bug".into()],
                assignees: vec!["alice".into()],
                description: Some("desc".into()),
                state: Some("state/todo".into()),
            },
        );
        created.ts = OffsetDateTime::now_utc() - Duration::seconds(10);

        let mut label_removed = Event::new(
            task,
            actor.clone(),
            EventKind::LabelsRemoved {
                labels: vec!["type/bug".into()],
            },
        );
        label_removed.ts = created.ts + Duration::seconds(5);

        let mut state_set = Event::new(
            task,
            actor,
            EventKind::TaskStateSet {
                state: "state/done".into(),
            },
        );
        state_set.ts = label_removed.ts + Duration::seconds(5);

        let snapshot = TaskSnapshot::replay(vec![state_set.clone(), created, label_removed]);

        assert_eq!(snapshot.id, task);
        assert_eq!(snapshot.title, "Initial");
        assert_eq!(snapshot.description, "desc");
        assert_eq!(snapshot.state.as_deref(), Some("state/done"));
        assert!(snapshot.labels.is_empty(), "labels should reflect removals");
        assert!(snapshot.assignees.contains("alice"));
        let expected_ts = state_set
            .ts
            .format(&Rfc3339)
            .unwrap_or_else(|err| panic!("timestamp must format: {err}"));
        assert_eq!(snapshot.updated_rfc3339.as_deref(), Some(expected_ts.as_str()));
    }

    #[test]
    fn replay_sorts_by_timestamp_then_id() {
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };
        let base = OffsetDateTime::now_utc();

        let mut early = Event::new(
            task,
            actor.clone(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: vec![],
                assignees: vec![],
                description: None,
                state: None,
            },
        );
        early.ts = base - Duration::seconds(5);

        let mut rel_add = Event::new(
            task,
            actor,
            EventKind::RelationAdded {
                kind: "relatesTo".into(),
                target: TaskId::new(),
            },
        );
        rel_add.ts = early.ts;

        let mut unordered = vec![rel_add, early];
        unordered.reverse();
        let snapshot = TaskSnapshot::replay(unordered);

        if let Some(relates) = snapshot.relates.get("relatesTo") {
            assert_eq!(relates.len(), 1);
        } else {
            panic!("relation not recorded");
        }
        assert_eq!(snapshot.title, "Initial");
    }
}
