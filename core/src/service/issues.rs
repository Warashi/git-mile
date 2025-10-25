use std::path::Path;
use std::sync::Arc;

use crate::clock::ReplicaId;
use crate::error::{Error, Result};
use crate::issue::{
    AppendIssueCommentInput, CreateIssueInput, IssueId, IssueStatus, IssueStore,
    UpdateIssueLabelsInput,
};
use crate::model::{Comment, CommentId, IssueDetails, Markdown};
use crate::repo::{LockMode, RepositoryCacheHook};

/// High-level service facade for issue operations.
pub struct IssueService {
    store: IssueStore,
}

impl IssueService {
    pub fn open(repo_path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self {
            store: IssueStore::open(repo_path)?,
        })
    }

    pub fn open_with_mode(repo_path: impl AsRef<Path>, mode: LockMode) -> Result<Self> {
        Ok(Self {
            store: IssueStore::open_with_mode(repo_path, mode)?,
        })
    }

    pub fn open_with_cache(
        repo_path: impl AsRef<Path>,
        mode: LockMode,
        cache: Arc<dyn RepositoryCacheHook>,
    ) -> Result<Self> {
        Ok(Self {
            store: IssueStore::open_with_cache(repo_path, mode, cache)?,
        })
    }

    pub fn create(&self, payload: CreatePayload) -> Result<IssueDetails> {
        let snapshot = self.store.create_issue(CreateIssueInput {
            replica_id: payload.replica_id,
            author: payload.author,
            message: payload.message,
            title: payload.title,
            description: payload.description.map(Markdown::into_string),
            initial_status: payload.initial_status,
            initial_comment: payload.initial_comment.map(Markdown::into_string),
            labels: payload.labels,
        })?;

        Ok(IssueDetails::from_snapshot(snapshot))
    }

    pub fn append_comment(&self, payload: AppendCommentPayload) -> Result<AppendCommentResult> {
        let outcome = self.store.append_comment(AppendIssueCommentInput {
            issue_id: payload.issue_id.clone(),
            replica_id: payload.replica_id,
            author: payload.author,
            message: payload.message,
            comment_id: payload.comment_id,
            body: payload.body_markdown.into_string(),
        })?;

        let details = IssueDetails::from_snapshot(outcome.snapshot);
        let comment = details
            .comments
            .iter()
            .find(|comment| comment.id == outcome.comment_id)
            .cloned()
            .ok_or_else(|| {
                Error::validation(format!(
                    "comment {} missing from issue {} snapshot",
                    outcome.comment_id, payload.issue_id
                ))
            })?;

        Ok(AppendCommentResult {
            created: outcome.created,
            comment,
            details,
        })
    }

    pub fn update_labels(&self, payload: LabelUpdatePayload) -> Result<LabelUpdateResult> {
        let outcome = self.store.update_labels(UpdateIssueLabelsInput {
            issue_id: payload.issue_id,
            replica_id: payload.replica_id,
            author: payload.author,
            message: payload.message,
            add: payload.add,
            remove: payload.remove,
        })?;

        let details = IssueDetails::from_snapshot(outcome.snapshot);
        Ok(LabelUpdateResult {
            changed: outcome.changed,
            added: outcome.added,
            removed: outcome.removed,
            details,
        })
    }

    pub fn get_with_comments(&self, issue_id: &IssueId) -> Result<IssueDetails> {
        let snapshot = self.store.load_issue(issue_id)?;
        Ok(IssueDetails::from_snapshot(snapshot))
    }
}

/// Input payload for creating an issue.
#[derive(Clone, Debug)]
pub struct CreatePayload {
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub title: String,
    pub description: Option<Markdown>,
    pub initial_status: IssueStatus,
    pub initial_comment: Option<Markdown>,
    pub labels: Vec<String>,
}

/// Input payload for appending a comment to an issue.
#[derive(Clone, Debug)]
pub struct AppendCommentPayload {
    pub issue_id: IssueId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub comment_id: Option<CommentId>,
    pub body_markdown: Markdown,
}

/// Result returned when appending a comment.
#[derive(Clone, Debug)]
pub struct AppendCommentResult {
    pub created: bool,
    pub comment: Comment,
    pub details: IssueDetails,
}

/// Input payload for updating labels on an issue.
#[derive(Clone, Debug)]
pub struct LabelUpdatePayload {
    pub issue_id: IssueId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

/// Result returned after updating labels on an issue.
#[derive(Clone, Debug)]
pub struct LabelUpdateResult {
    pub changed: bool,
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub details: IssueDetails,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ReplicaId;
    use crate::model::{CommentParent, LabelOperation};
    use tempfile::tempdir;

    #[test]
    fn issue_service_round_trips_details() {
        let temp = tempdir().expect("create temp dir");
        let service = IssueService::open(temp.path()).expect("open issue service");
        let replica = ReplicaId::new("replica-issue");

        let created = service
            .create(CreatePayload {
                replica_id: replica.clone(),
                author: "alice@example.com".into(),
                message: Some("create issue".into()),
                title: "Service Issue".into(),
                description: Some(Markdown::new("## Details\n\nBody text.")),
                initial_status: IssueStatus::Draft,
                initial_comment: Some(Markdown::new("First comment")),
                labels: vec!["alpha".into()],
            })
            .expect("create issue");

        assert_eq!(created.title, "Service Issue");
        assert_eq!(created.status, IssueStatus::Draft);
        assert_eq!(created.labels.len(), 1);
        let first_comment = created.comments.first().expect("initial comment present");
        assert!(matches!(
            first_comment.parent,
            CommentParent::Issue(ref parent_id) if parent_id == &created.id
        ));
        assert_eq!(created.initial_comment_id, Some(first_comment.id));

        let appended = service
            .append_comment(AppendCommentPayload {
                issue_id: created.id.clone(),
                replica_id: replica.clone(),
                author: "bob@example.com".into(),
                message: Some("append comment".into()),
                comment_id: None,
                body_markdown: Markdown::new("Follow up from Bob"),
            })
            .expect("append comment");

        assert!(appended.created);
        assert_eq!(appended.comment.author_id, "bob@example.com");
        assert_eq!(appended.details.comments.len(), 2);

        let updated_labels = service
            .update_labels(LabelUpdatePayload {
                issue_id: created.id.clone(),
                replica_id: replica.clone(),
                author: "carol@example.com".into(),
                message: Some("update labels".into()),
                add: vec!["beta".into()],
                remove: vec!["alpha".into()],
            })
            .expect("update labels");

        assert!(updated_labels.changed);
        assert_eq!(updated_labels.added, vec!["beta".to_string()]);
        assert_eq!(updated_labels.removed, vec!["alpha".to_string()]);
        assert!(updated_labels.details.labels.contains("beta"));
        assert!(!updated_labels.details.labels.contains("alpha"));

        let label_events = &updated_labels.details.label_events;
        assert_eq!(label_events.len(), 2);
        assert_eq!(label_events[0].operation, LabelOperation::Remove);
        assert_eq!(label_events[0].label_id, "alpha");
        assert_eq!(label_events[0].actor_id, "carol@example.com");
        assert_eq!(label_events[1].operation, LabelOperation::Add);
        assert_eq!(label_events[1].label_id, "beta");

        let fetched = service
            .get_with_comments(&created.id)
            .expect("load issue details");
        assert_eq!(fetched.comments.len(), 2);
        assert!(fetched.labels.contains("beta"));
        assert_eq!(fetched.initial_comment_id, created.initial_comment_id);
    }
}
