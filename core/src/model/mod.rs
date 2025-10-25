use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::clock::LamportTimestamp;
use crate::issue::{
    IssueEventKind, IssueId, IssueSnapshot, IssueStatus, LabelId as IssueLabelId,
};
use crate::mile::{
    LabelId as MileLabelId, MileEventKind, MileId, MileSnapshot, MileStatus,
};

/// Shared identifier type for comments written against issues or milestones.
pub type CommentId = Uuid;

/// Shared label identifier type used across issues and milestones.
pub type LabelId = String;

/// Wrapper representing a Markdown body persisted in the repository.
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct Markdown(String);

impl Markdown {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl Deref for Markdown {
    type Target = str;

    fn deref(&self) -> &Self::Target {
        self.as_str()
    }
}

impl From<String> for Markdown {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

impl From<&str> for Markdown {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl fmt::Display for Markdown {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Parent resource associated with a [`Comment`].
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "id", rename_all = "snake_case")]
pub enum CommentParent {
    Issue(IssueId),
    Milestone(MileId),
}

/// Domain representation of a stored comment body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: CommentId,
    pub parent: CommentParent,
    pub body_markdown: Markdown,
    pub author_id: String,
    pub created_at: LamportTimestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<LamportTimestamp>,
}

/// Operation applied to a label association timeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LabelOperation {
    Add,
    Remove,
}

/// Event describing a label change on a resource.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LabelEvent {
    pub operation: LabelOperation,
    pub label_id: LabelId,
    pub actor_id: String,
    pub timestamp: LamportTimestamp,
}

/// Snapshot-style representation of an issue, enriched with metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IssueDetails {
    pub id: IssueId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Markdown>,
    pub status: IssueStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_comment_id: Option<CommentId>,
    pub labels: std::collections::BTreeSet<IssueLabelId>,
    pub comments: Vec<Comment>,
    pub label_events: Vec<LabelEvent>,
    pub created_at: LamportTimestamp,
    pub updated_at: LamportTimestamp,
    pub clock_snapshot: LamportTimestamp,
}

/// Snapshot representation of a milestone with rich metadata.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MilestoneDetails {
    pub id: MileId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<Markdown>,
    pub status: MileStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_comment_id: Option<CommentId>,
    pub labels: std::collections::BTreeSet<MileLabelId>,
    pub comments: Vec<Comment>,
    pub label_events: Vec<LabelEvent>,
    pub created_at: LamportTimestamp,
    pub updated_at: LamportTimestamp,
    pub clock_snapshot: LamportTimestamp,
}

impl IssueDetails {
    pub(crate) fn from_snapshot(snapshot: IssueSnapshot) -> Self {
        let IssueSnapshot {
            id,
            title,
            description,
            status,
            labels,
            comments,
            created_at,
            updated_at,
            clock_snapshot,
            events,
        } = snapshot;

        let mut initial_comment_id = None;
        let mut label_events = Vec::new();
        for event in &events {
            match &event.payload {
                IssueEventKind::Created(created) => {
                    if let Some(initial) = &created.initial_comment {
                        initial_comment_id = Some(initial.comment_id);
                    }
                }
                IssueEventKind::LabelAttached(data) => {
                    label_events.push(LabelEvent {
                        operation: LabelOperation::Add,
                        label_id: data.label.clone(),
                        actor_id: event.metadata.author.clone(),
                        timestamp: event.timestamp.clone(),
                    });
                }
                IssueEventKind::LabelDetached(data) => {
                    label_events.push(LabelEvent {
                        operation: LabelOperation::Remove,
                        label_id: data.label.clone(),
                        actor_id: event.metadata.author.clone(),
                        timestamp: event.timestamp.clone(),
                    });
                }
                IssueEventKind::Unknown { .. }
                | IssueEventKind::StatusChanged(_)
                | IssueEventKind::CommentAppended(_) => {}
            }
        }

        let mut mapped_comments = Vec::with_capacity(comments.len());
        for comment in comments {
            mapped_comments.push(Comment {
                id: comment.id,
                parent: CommentParent::Issue(id.clone()),
                body_markdown: Markdown::new(comment.body),
                author_id: comment.author,
                created_at: comment.created_at,
                edited_at: comment.edited_at,
            });
        }

        Self {
            id,
            title,
            description: description.map(Markdown::new),
            status,
            initial_comment_id,
            labels,
            comments: mapped_comments,
            label_events,
            created_at,
            updated_at,
            clock_snapshot,
        }
    }
}

impl MilestoneDetails {
    pub(crate) fn from_snapshot(snapshot: MileSnapshot) -> Self {
        let MileSnapshot {
            id,
            title,
            description,
            status,
            labels,
            comments,
            created_at,
            updated_at,
            clock_snapshot,
            events,
        } = snapshot;

        let mut initial_comment_id = None;
        let mut label_events = Vec::new();
        for event in &events {
            match &event.payload {
                MileEventKind::Created(created) => {
                    if let Some(initial) = &created.initial_comment {
                        initial_comment_id = Some(initial.comment_id);
                    }
                }
                MileEventKind::LabelAttached(data) => {
                    label_events.push(LabelEvent {
                        operation: LabelOperation::Add,
                        label_id: data.label.clone(),
                        actor_id: event.metadata.author.clone(),
                        timestamp: event.timestamp.clone(),
                    });
                }
                MileEventKind::LabelDetached(data) => {
                    label_events.push(LabelEvent {
                        operation: LabelOperation::Remove,
                        label_id: data.label.clone(),
                        actor_id: event.metadata.author.clone(),
                        timestamp: event.timestamp.clone(),
                    });
                }
                MileEventKind::Unknown { .. }
                | MileEventKind::StatusChanged(_)
                | MileEventKind::CommentAppended(_) => {}
            }
        }

        let mut mapped_comments = Vec::with_capacity(comments.len());
        for comment in comments {
            mapped_comments.push(Comment {
                id: comment.id,
                parent: CommentParent::Milestone(id.clone()),
                body_markdown: Markdown::new(comment.body),
                author_id: comment.author,
                created_at: comment.created_at,
                edited_at: comment.edited_at,
            });
        }

        Self {
            id,
            title,
            description: description.map(Markdown::new),
            status,
            initial_comment_id,
            labels,
            comments: mapped_comments,
            label_events,
            created_at,
            updated_at,
            clock_snapshot,
        }
    }
}
