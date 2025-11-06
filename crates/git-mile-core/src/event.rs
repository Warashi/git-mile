use crate::id::{EventId, TaskId};
use crate::state::StateKind;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

/// Actor (author/committer).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Actor {
    /// Display name.
    pub name: String,
    /// Contact email.
    pub email: String,
}

/// Event envelope stored as JSON in the commit message body.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Schema identifier for forward compatibility.
    pub schema: String,
    /// Unique event identifier.
    pub id: EventId,
    #[serde(with = "time::serde::rfc3339")]
    /// Event timestamp in UTC.
    pub ts: OffsetDateTime,
    /// Actor who authored the event.
    pub actor: Actor,
    /// Target task identifier.
    pub task: TaskId,
    /// Domain-specific payload.
    pub kind: EventKind,
}

/// Event kinds (extend as needed).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum EventKind {
    /// A new task is created.
    TaskCreated {
        /// Human-readable title.
        title: String,
        /// Labels to attach.
        #[serde(default)]
        labels: Vec<String>,
        /// Initial assignees.
        #[serde(default)]
        assignees: Vec<String>,
        /// Optional description in Markdown.
        #[serde(default)]
        description: Option<String>,
        /// Optional workflow state label.
        #[serde(default)]
        state: Option<String>,
        /// Optional workflow classification.
        #[serde(default)]
        state_kind: Option<StateKind>,
    },
    /// The workflow state is overwritten.
    TaskStateSet {
        /// New state label.
        state: String,
        /// Optional workflow classification.
        #[serde(default)]
        state_kind: Option<StateKind>,
    },
    /// The workflow state is cleared.
    TaskStateCleared,
    /// The task title is overwritten.
    TaskTitleSet {
        /// New task title.
        title: String,
    },
    /// The task description is overwritten.
    TaskDescriptionSet {
        /// New description body in Markdown (or `None` to clear).
        #[serde(default)]
        description: Option<String>,
    },
    /// One or more labels are added.
    LabelsAdded {
        /// Labels to add.
        labels: Vec<String>,
    },
    /// One or more labels are removed.
    LabelsRemoved {
        /// Labels to remove.
        labels: Vec<String>,
    },
    /// One or more assignees are added.
    AssigneesAdded {
        /// Assignees to add.
        assignees: Vec<String>,
    },
    /// One or more assignees are removed.
    AssigneesRemoved {
        /// Assignees to remove.
        assignees: Vec<String>,
    },
    /// A Markdown comment is added.
    CommentAdded {
        /// Identifier for the comment event.
        comment_id: EventId,
        /// Comment body in Markdown.
        body_md: String,
    },
    /// A Markdown comment is updated.
    CommentUpdated {
        /// Identifier for the comment event to update.
        comment_id: EventId,
        /// New comment body in Markdown.
        body_md: String,
    },
    /// A parent-child relationship is established.
    ChildLinked {
        /// Parent task identifier.
        parent: TaskId,
        /// Child task identifier.
        child: TaskId,
    },
    /// An existing parent-child relation is removed.
    ChildUnlinked {
        /// Parent task identifier.
        parent: TaskId,
        /// Child task identifier.
        child: TaskId,
    },
    /// A generic relation is linked.
    RelationAdded {
        /// Relation kind key.
        kind: String,
        /// Target task identifier.
        target: TaskId,
    },
    /// A generic relation is unlinked.
    RelationRemoved {
        /// Relation kind key.
        kind: String,
        /// Target task identifier.
        target: TaskId,
    },
}

impl Event {
    /// Create a new event with the current timestamp.
    #[must_use]
    pub fn new(task: TaskId, actor: &Actor, kind: EventKind) -> Self {
        Self {
            schema: "git-mile-event@1".to_owned(),
            id: EventId::new(),
            ts: OffsetDateTime::now_utc(),
            actor: actor.clone(),
            task,
            kind,
        }
    }
}
