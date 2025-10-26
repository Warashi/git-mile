use std::path::Path;
use std::sync::Arc;

use crate::clock::ReplicaId;
use crate::error::{Error, Result};
use crate::mile::{
    AppendCommentInput, ChangeStatusInput, CreateMileInput, LabelId, MileId, MileStatus, MileStore,
    UpdateLabelsInput,
};
use crate::model::{Comment, CommentId, Markdown, MilestoneDetails};
use crate::repo::{LockMode, RepositoryCacheHook};

/// High-level service facade for milestone operations.
pub struct MilestoneService {
    store: MileStore,
}

impl MilestoneService {
    /// Open the milestone service for the given repository.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying milestone store cannot be opened.
    pub fn open(repo_path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: MileStore::open(repo_path)?,
        })
    }

    /// Open the milestone service using the specified repository lock mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying store cannot be opened with the requested lock mode.
    pub fn open_with_mode(repo_path: impl AsRef<Path>, mode: LockMode) -> Result<Self> {
        Ok(Self {
            store: MileStore::open_with_mode(repo_path, mode)?,
        })
    }

    /// Open the milestone service with a cache hook used to accelerate entity lookups.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying store cannot be opened or the cache initialization
    /// fails.
    pub fn open_with_cache(
        repo_path: impl AsRef<Path>,
        mode: LockMode,
        cache: Arc<dyn RepositoryCacheHook>,
    ) -> Result<Self> {
        Ok(Self {
            store: MileStore::open_with_cache(repo_path, mode, cache)?,
        })
    }

    /// Create a new milestone with the provided payload.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying store fails to persist the milestone or when the output
    /// snapshot cannot be reconstructed.
    pub fn create(&self, payload: CreatePayload) -> Result<MilestoneDetails> {
        let snapshot = self.store.create_mile(CreateMileInput {
            replica_id: payload.replica_id,
            author: payload.author,
            message: payload.message,
            title: payload.title,
            description: payload.description.map(Markdown::into_string),
            initial_status: payload.initial_status,
            initial_comment: payload.initial_comment.map(Markdown::into_string),
            labels: payload.labels,
        })?;

        Ok(MilestoneDetails::from_snapshot(snapshot))
    }

    /// Append a comment to a milestone and return the updated details.
    ///
    /// # Errors
    ///
    /// Returns an error when the milestone cannot be loaded, the comment cannot be persisted, or
    /// the resulting snapshot does not contain the appended comment.
    pub fn append_comment(&self, payload: AppendCommentPayload) -> Result<AppendCommentResult> {
        let outcome = self.store.append_comment(AppendCommentInput {
            mile_id: payload.milestone_id.clone(),
            replica_id: payload.replica_id,
            author: payload.author,
            message: payload.message,
            comment_id: payload.comment_id,
            body: payload.body_markdown.into_string(),
        })?;

        let details = MilestoneDetails::from_snapshot(outcome.snapshot);
        let comment = details
            .comments
            .iter()
            .find(|comment| comment.id == outcome.comment_id)
            .cloned()
            .ok_or_else(|| {
                Error::validation(format!(
                    "comment {} missing from milestone {} snapshot",
                    outcome.comment_id, payload.milestone_id
                ))
            })?;

        Ok(AppendCommentResult {
            created: outcome.created,
            comment,
            details,
        })
    }

    /// Update the set of labels attached to a milestone.
    ///
    /// # Errors
    ///
    /// Returns an error when the milestone cannot be loaded or when the label updates fail to
    /// persist.
    pub fn update_labels(&self, payload: LabelUpdatePayload) -> Result<LabelUpdateResult> {
        let outcome = self.store.update_labels(UpdateLabelsInput {
            mile_id: payload.milestone_id.clone(),
            replica_id: payload.replica_id,
            author: payload.author,
            message: payload.message,
            add: payload.add.clone(),
            remove: payload.remove,
        })?;
        let details = MilestoneDetails::from_snapshot(outcome.snapshot);
        Ok(LabelUpdateResult {
            changed: outcome.changed,
            added: outcome.added,
            removed: outcome.removed,
            details,
        })
    }

    /// Change the status of a milestone.
    ///
    /// # Errors
    ///
    /// Returns an error when the milestone cannot be loaded or when the status change operation
    /// fails to persist.
    pub fn change_status(&self, payload: ChangeStatusPayload) -> Result<ChangeStatusResult> {
        let outcome = self.store.change_status(ChangeStatusInput {
            mile_id: payload.milestone_id.clone(),
            replica_id: payload.replica_id,
            author: payload.author,
            message: payload.message,
            status: payload.status,
        })?;
        let details = MilestoneDetails::from_snapshot(outcome.snapshot);
        Ok(ChangeStatusResult {
            changed: outcome.changed,
            details,
        })
    }

    /// Load a milestone with all associated comments.
    ///
    /// # Errors
    ///
    /// Returns an error when the milestone cannot be loaded or the snapshot reconstruction fails.
    pub fn get_with_comments(&self, milestone_id: &MileId) -> Result<MilestoneDetails> {
        let snapshot = self.store.load_mile(milestone_id)?;
        Ok(MilestoneDetails::from_snapshot(snapshot))
    }
}

/// Input payload for creating a milestone.
#[derive(Clone, Debug)]
pub struct CreatePayload {
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub title: String,
    pub description: Option<Markdown>,
    pub initial_status: MileStatus,
    pub initial_comment: Option<Markdown>,
    pub labels: Vec<LabelId>,
}

/// Input payload for appending a comment to a milestone.
#[derive(Clone, Debug)]
pub struct AppendCommentPayload {
    pub milestone_id: MileId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub comment_id: Option<CommentId>,
    pub body_markdown: Markdown,
}

/// Result returned when appending a comment to a milestone.
#[derive(Clone, Debug)]
pub struct AppendCommentResult {
    pub created: bool,
    pub comment: Comment,
    pub details: MilestoneDetails,
}

/// Input payload for updating labels on a milestone.
#[derive(Clone, Debug)]
pub struct LabelUpdatePayload {
    pub milestone_id: MileId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub add: Vec<LabelId>,
    pub remove: Vec<LabelId>,
}

/// Result returned when updating labels on a milestone.
#[derive(Clone, Debug)]
pub struct LabelUpdateResult {
    pub changed: bool,
    pub added: Vec<LabelId>,
    pub removed: Vec<LabelId>,
    pub details: MilestoneDetails,
}

/// Input payload for changing a milestone's status.
#[derive(Clone, Debug)]
pub struct ChangeStatusPayload {
    pub milestone_id: MileId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub status: MileStatus,
}

/// Result returned when changing a milestone's status.
#[derive(Clone, Debug)]
pub struct ChangeStatusResult {
    pub changed: bool,
    pub details: MilestoneDetails,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ReplicaId;
    use crate::model::{CommentParent, LabelOperation};
    use tempfile::tempdir;

    #[test]
    fn milestone_service_tracks_comment_and_labels() {
        let temp = tempdir().expect("create temp dir");
        let service = MilestoneService::open(temp.path()).expect("open milestone service");
        let replica = ReplicaId::new("replica-milestone");

        let created = service
            .create(CreatePayload {
                replica_id: replica.clone(),
                author: "dora@example.com".into(),
                message: Some("create milestone".into()),
                title: "Milestone Alpha".into(),
                description: Some(Markdown::new("Milestone description.")),
                initial_status: MileStatus::Open,
                initial_comment: Some(Markdown::new("Kickoff comment")),
                labels: vec!["roadmap".into()],
            })
            .expect("create milestone");

        assert_eq!(created.status, MileStatus::Open);
        assert!(created.labels.contains("roadmap"));
        let initial = created.comments.first().expect("initial comment present");
        assert!(matches!(
            initial.parent,
            CommentParent::Milestone(ref parent_id) if parent_id == &created.id
        ));
        assert_eq!(created.initial_comment_id, Some(initial.id));

        let appended = service
            .append_comment(AppendCommentPayload {
                milestone_id: created.id.clone(),
                replica_id: replica.clone(),
                author: "eli@example.com".into(),
                message: Some("follow up".into()),
                comment_id: None,
                body_markdown: Markdown::new("Progress update"),
            })
            .expect("append comment");
        assert!(appended.created);
        assert_eq!(appended.details.comments.len(), 2);

        let updated_labels = service
            .update_labels(LabelUpdatePayload {
                milestone_id: created.id.clone(),
                replica_id: replica.clone(),
                author: "faye@example.com".into(),
                message: Some("tweak labels".into()),
                add: vec!["priority".into()],
                remove: vec!["roadmap".into()],
            })
            .expect("update labels");
        assert!(updated_labels.changed);
        assert!(updated_labels.details.labels.contains("priority"));
        assert!(!updated_labels.details.labels.contains("roadmap"));

        let label_events = &updated_labels.details.label_events;
        assert_eq!(label_events.len(), 2);
        assert_eq!(label_events[0].operation, LabelOperation::Remove);
        assert_eq!(label_events[0].label_id, "roadmap");
        assert_eq!(label_events[1].operation, LabelOperation::Add);
        assert_eq!(label_events[1].label_id, "priority");

        let status_outcome = service
            .change_status(ChangeStatusPayload {
                milestone_id: created.id.clone(),
                replica_id: replica,
                author: "dora@example.com".into(),
                message: Some("close milestone".into()),
                status: MileStatus::Closed,
            })
            .expect("change status");
        assert!(status_outcome.changed);
        assert_eq!(status_outcome.details.status, MileStatus::Closed);

        let fetched = service
            .get_with_comments(&created.id)
            .expect("load milestone");
        assert_eq!(fetched.comments.len(), 2);
        assert_eq!(fetched.status, MileStatus::Closed);
    }
}
