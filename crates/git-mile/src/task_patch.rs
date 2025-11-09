use std::collections::BTreeSet;

use git_mile_core::TaskSnapshot;

use crate::task_writer::{DescriptionPatch, SetDiff, StatePatch, TaskUpdate, diff_sets};

/// Normalized task edit fields used to compute diffs.
#[derive(Debug)]
pub struct TaskEditData {
    /// Desired task title.
    pub title: String,
    /// Desired workflow state (clears when `None`).
    pub state: Option<String>,
    /// Desired label set.
    pub labels: Vec<String>,
    /// Desired assignee set.
    pub assignees: Vec<String>,
    /// Desired description body (`None` leaves unchanged, `Some("")` clears).
    pub description: Option<String>,
}

#[allow(clippy::missing_const_for_fn)]
impl TaskEditData {
    /// Construct a new edit payload.
    #[must_use]
    pub fn new(
        title: String,
        state: Option<String>,
        labels: Vec<String>,
        assignees: Vec<String>,
        description: Option<String>,
    ) -> Self {
        Self {
            title,
            state,
            labels,
            assignees,
            description,
        }
    }
}

/// Diff between a snapshot and target fields.
#[derive(Debug, Default)]
pub struct TaskPatch {
    /// Title change (if any).
    pub title: Option<String>,
    /// State change (if any).
    pub state: Option<StatePatch>,
    /// Description change (if any).
    pub description: Option<DescriptionPatch>,
    /// Label additions/removals.
    pub labels: SetDiff<String>,
    /// Assignee additions/removals.
    pub assignees: SetDiff<String>,
}

#[allow(clippy::missing_const_for_fn)]
impl TaskPatch {
    /// Compute a patch by comparing snapshot state with the provided edits.
    #[must_use]
    pub fn from_snapshot(snapshot: &TaskSnapshot, data: TaskEditData) -> Self {
        let TaskEditData {
            title,
            state,
            labels,
            assignees,
            description,
        } = data;

        let mut patch = Self::default();

        if title != snapshot.title {
            patch.title = Some(title);
        }

        patch.state = match (snapshot.state.as_ref(), state) {
            (Some(old), Some(new)) if *old != new => Some(StatePatch::Set { state: new }),
            (None, Some(new)) => Some(StatePatch::Set { state: new }),
            (Some(_), None) => Some(StatePatch::Clear),
            _ => None,
        };

        let desired_labels: BTreeSet<String> = labels.into_iter().collect();
        patch.labels = diff_sets(&snapshot.labels, &desired_labels);

        let desired_assignees: BTreeSet<String> = assignees.into_iter().collect();
        patch.assignees = diff_sets(&snapshot.assignees, &desired_assignees);

        patch.description = description.map_or_else(
            || (!snapshot.description.is_empty()).then_some(DescriptionPatch::Clear),
            |text| {
                if text.is_empty() {
                    if snapshot.description.is_empty() {
                        None
                    } else {
                        Some(DescriptionPatch::Clear)
                    }
                } else if text == snapshot.description {
                    None
                } else {
                    Some(DescriptionPatch::Set { description: text })
                }
            },
        );

        patch
    }

    /// Returns true when the patch would not emit any `TaskUpdate`.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.state.is_none()
            && self.description.is_none()
            && self.labels.is_empty()
            && self.assignees.is_empty()
    }

    /// Convert the patch into a [`TaskUpdate`] consumable by [`TaskWriter`](crate::task_writer::TaskWriter).
    #[must_use]
    pub fn into_task_update(self) -> TaskUpdate {
        TaskUpdate {
            title: self.title,
            state: self.state,
            description: self.description,
            labels: self.labels,
            assignees: self.assignees,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::event::{Actor, Event, EventKind};
    use git_mile_core::id::TaskId;
    use std::str::FromStr;
    use time::OffsetDateTime;

    fn snapshot_with(title: &str, state: Option<&str>, description: &str) -> TaskSnapshot {
        let task = TaskId::from_str("00000000-0000-0000-0000-000000000001")
            .unwrap_or_else(|err| panic!("must parse task id: {err}"));
        let mut event = Event::new(
            task,
            &Actor {
                name: "tester".into(),
                email: "tester@example.invalid".into(),
            },
            EventKind::TaskCreated {
                title: title.into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: if description.is_empty() {
                    None
                } else {
                    Some(description.into())
                },
                state: state.map(str::to_owned),
                state_kind: None,
            },
        );
        event.ts = OffsetDateTime::UNIX_EPOCH;

        TaskSnapshot::replay(&[event])
    }

    fn default_data() -> TaskEditData {
        TaskEditData::new(
            "Title".into(),
            Some("state/todo".into()),
            Vec::new(),
            Vec::new(),
            None,
        )
    }

    #[test]
    fn patch_detects_title_and_state_changes() {
        let snapshot = snapshot_with("Old", Some("state/done"), "");
        let data = TaskEditData::new(
            "New".into(),
            Some("state/todo".into()),
            Vec::new(),
            Vec::new(),
            None,
        );
        let patch = TaskPatch::from_snapshot(&snapshot, data);
        assert!(patch.title.is_some());
        assert!(matches!(patch.state, Some(StatePatch::Set { state }) if state == "state/todo"));
    }

    #[test]
    fn patch_clears_description_when_empty() {
        let snapshot = snapshot_with("Title", None, "body");
        let data = TaskEditData::new("Title".into(), None, Vec::new(), Vec::new(), Some(String::new()));
        let patch = TaskPatch::from_snapshot(&snapshot, data);
        assert!(matches!(patch.description, Some(DescriptionPatch::Clear)));
    }

    #[test]
    fn patch_is_empty_when_fields_match() {
        let snapshot = snapshot_with("Title", Some("state/todo"), "");
        let data = default_data();
        let patch = TaskPatch::from_snapshot(&snapshot, data);
        assert!(patch.is_empty());
    }

    #[test]
    fn patch_emits_diff_for_labels_and_assignees() {
        let mut snapshot = snapshot_with("Title", None, "");
        snapshot.labels.insert("a".into());
        snapshot.assignees.insert("alice".into());

        let data = TaskEditData::new("Title".into(), None, vec!["b".into()], vec!["bob".into()], None);
        let patch = TaskPatch::from_snapshot(&snapshot, data);
        assert_eq!(patch.labels.removed, vec!["a"]);
        assert_eq!(patch.labels.added, vec!["b"]);
        assert_eq!(patch.assignees.removed, vec!["alice"]);
        assert_eq!(patch.assignees.added, vec!["bob"]);
    }
}
