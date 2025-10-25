use std::collections::{BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::clock::{LamportClock, LamportTimestamp, ReplicaId};
use crate::dag::{
    BlobRef, EntityId, EntitySnapshot, EntityStore, Operation, OperationBlob, OperationId,
    OperationMetadata, OperationPack,
};
use crate::error::{Error, Result};
use crate::repo::{LockMode, RepositoryCacheHook};

const EVENT_VERSION: u8 = 2;

/// Identifier alias for issues backed by the entity store.
pub type IssueId = EntityId;

/// Identifier assigned to individual comments.
pub type CommentId = Uuid;

/// Identifier assigned to labels.
pub type LabelId = String;

/// High-level status for an issue.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    Draft,
    Open,
    Closed,
}

impl std::fmt::Display for IssueStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            IssueStatus::Draft => "draft",
            IssueStatus::Open => "open",
            IssueStatus::Closed => "closed",
        };
        f.write_str(label)
    }
}

impl std::str::FromStr for IssueStatus {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "draft" => Ok(Self::Draft),
            "open" => Ok(Self::Open),
            "closed" => Ok(Self::Closed),
            other => Err(Error::validation(format!("unknown issue status: {other}"))),
        }
    }
}

/// Event timeline entry for an issue.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
enum StoredEventPayload {
    #[serde(rename = "created")]
    Created(IssueCreated),
    #[serde(rename = "status_changed")]
    StatusChanged(IssueStatusChanged),
    #[serde(rename = "comment_appended")]
    CommentAppended(IssueCommentAppended),
    #[serde(rename = "label_attached")]
    LabelAttached(IssueLabelAttached),
    #[serde(rename = "label_detached")]
    LabelDetached(IssueLabelDetached),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredEvent {
    #[serde(default)]
    version: u8,
    #[serde(flatten)]
    payload: StoredEventPayload,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueCommentAppended {
    pub comment_id: CommentId,
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueLabelAttached {
    pub label: LabelId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueLabelDetached {
    pub label: LabelId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueCreated {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: IssueStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_comment: Option<IssueCommentAppended>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<LabelId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IssueStatusChanged {
    pub status: IssueStatus,
}

/// Represents a user-facing event in an issue's history.
#[derive(Clone, Debug, Serialize)]
pub struct IssueEvent {
    pub id: OperationId,
    pub timestamp: LamportTimestamp,
    pub metadata: OperationMetadata,
    pub payload: IssueEventKind,
}

/// High-level event kind presented to callers.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IssueEventKind {
    Created(IssueCreated),
    StatusChanged(IssueStatusChanged),
    CommentAppended(IssueCommentAppended),
    LabelAttached(IssueLabelAttached),
    LabelDetached(IssueLabelDetached),
    Unknown {
        version: Option<u8>,
        event_type: Option<String>,
        raw: Value,
    },
}

/// Snapshot representation of an issue comment.
#[derive(Clone, Debug, Serialize)]
pub struct IssueComment {
    pub id: CommentId,
    pub body: String,
    pub author: String,
    pub created_at: LamportTimestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<LamportTimestamp>,
}

/// Snapshot describing the current state of an issue.
#[derive(Clone, Debug, Serialize)]
pub struct IssueSnapshot {
    pub id: IssueId,
    pub title: String,
    pub description: Option<String>,
    pub status: IssueStatus,
    pub labels: BTreeSet<LabelId>,
    pub comments: Vec<IssueComment>,
    pub created_at: LamportTimestamp,
    pub updated_at: LamportTimestamp,
    pub clock_snapshot: LamportTimestamp,
    pub events: Vec<IssueEvent>,
}

/// Summary data used for listing issues.
#[derive(Clone, Debug, Serialize)]
pub struct IssueSummary {
    pub id: IssueId,
    pub title: String,
    pub status: IssueStatus,
    pub updated_at: LamportTimestamp,
}

/// Request payload for creating a new issue.
#[derive(Clone, Debug)]
pub struct CreateIssueInput {
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub initial_status: IssueStatus,
    pub initial_comment: Option<String>,
    pub labels: Vec<LabelId>,
}

/// Request payload for changing the status of an existing issue.
#[derive(Clone, Debug)]
pub struct IssueChangeStatusInput {
    pub issue_id: IssueId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub status: IssueStatus,
}

/// Outcome returned when applying a status change.
#[derive(Clone, Debug, Serialize)]
pub struct IssueChangeStatusOutcome {
    pub changed: bool,
    pub snapshot: IssueSnapshot,
}

/// Request payload for appending a comment to an issue.
#[derive(Clone, Debug)]
pub struct AppendIssueCommentInput {
    pub issue_id: IssueId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub comment_id: Option<CommentId>,
    pub body: String,
}

/// Outcome returned after attempting to append a comment.
#[derive(Clone, Debug, Serialize)]
pub struct AppendIssueCommentOutcome {
    pub created: bool,
    pub comment_id: CommentId,
    pub snapshot: IssueSnapshot,
}

/// Request payload for updating labels on an issue.
#[derive(Clone, Debug)]
pub struct UpdateIssueLabelsInput {
    pub issue_id: IssueId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub add: Vec<LabelId>,
    pub remove: Vec<LabelId>,
}

/// Outcome returned when updating labels on an issue.
#[derive(Clone, Debug, Serialize)]
pub struct UpdateIssueLabelsOutcome {
    pub changed: bool,
    pub added: Vec<LabelId>,
    pub removed: Vec<LabelId>,
    pub snapshot: IssueSnapshot,
}

/// High-level interface for issue operations.
pub struct IssueStore {
    entities: EntityStore,
}

impl IssueStore {
    pub fn open(repo_path: impl AsRef<Path>) -> Result<Self> {
        let entities = EntityStore::open(repo_path)?;
        Ok(Self { entities })
    }

    pub fn open_with_mode(repo_path: impl AsRef<Path>, mode: LockMode) -> Result<Self> {
        let entities = EntityStore::open_with_mode(repo_path, mode)?;
        Ok(Self { entities })
    }

    pub fn open_with_cache(
        repo_path: impl AsRef<Path>,
        mode: LockMode,
        cache: Arc<dyn RepositoryCacheHook>,
    ) -> Result<Self> {
        let entities = EntityStore::open_with_cache(repo_path, mode, cache)?;
        Ok(Self { entities })
    }

    pub fn create_issue(&self, input: CreateIssueInput) -> Result<IssueSnapshot> {
        let CreateIssueInput {
            replica_id,
            author,
            message,
            title,
            description,
            initial_status,
            initial_comment,
            labels,
        } = input;

        let initial_comment = initial_comment.map(|body| IssueCommentAppended {
            comment_id: Uuid::new_v4(),
            body,
        });
        let labels: Vec<LabelId> = labels
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        let entity_id = EntityId::new();
        let mut clock = LamportClock::new(replica_id.clone());
        let (operation, blob) = build_operation(
            &mut clock,
            vec![],
            IssueEventKind::Created(IssueCreated {
                title,
                description,
                status: initial_status,
                initial_comment,
                labels,
            }),
            author,
            message,
        )?;
        let pack = OperationPack::new(
            entity_id.clone(),
            clock.snapshot(),
            vec![operation],
            vec![blob],
        )?;

        self.entities.persist_pack(pack)?;
        let snapshot = self.entities.load_entity(&entity_id)?;
        build_issue_snapshot(snapshot)
    }

    pub fn load_issue(&self, issue_id: &IssueId) -> Result<IssueSnapshot> {
        let snapshot = self.entities.load_entity(issue_id)?;
        build_issue_snapshot(snapshot)
    }

    pub fn list_issues(&self) -> Result<Vec<IssueSummary>> {
        let summaries = self.entities.list_entities()?;
        let mut issues = Vec::with_capacity(summaries.len());

        for summary in summaries {
            let entity_id = summary.entity_id.clone();
            let snapshot = self.entities.load_entity(&entity_id)?;
            match build_issue_snapshot(snapshot) {
                Ok(issue) => {
                    issues.push(IssueSummary {
                        id: issue.id,
                        title: issue.title.clone(),
                        status: issue.status,
                        updated_at: issue.updated_at.clone(),
                    });
                }
                Err(Error::Validation(_)) => continue,
                Err(err) => return Err(err),
            }
        }

        issues.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
        issues.reverse();
        Ok(issues)
    }

    pub fn change_status(&self, input: IssueChangeStatusInput) -> Result<IssueChangeStatusOutcome> {
        let IssueChangeStatusInput {
            issue_id,
            replica_id,
            author,
            message,
            status,
        } = input;

        let snapshot = self.entities.load_entity(&issue_id)?;
        let heads = snapshot.heads.clone();
        if heads.len() != 1 {
            return Err(Error::conflict(format!(
                "issue {} has {} heads; resolve conflicts before changing status",
                issue_id,
                heads.len()
            )));
        }

        let counter = snapshot.clock_snapshot.counter();
        let mut issue_snapshot = build_issue_snapshot(snapshot)?;
        if issue_snapshot.status == status {
            return Ok(IssueChangeStatusOutcome {
                changed: false,
                snapshot: issue_snapshot,
            });
        }

        let mut clock = LamportClock::with_state(replica_id.clone(), counter);
        let event_payload = IssueEventKind::StatusChanged(IssueStatusChanged { status });
        let (operation, blob) = build_operation(&mut clock, heads, event_payload, author, message)?;

        let pack = OperationPack::new(
            issue_id.clone(),
            clock.snapshot(),
            vec![operation],
            vec![blob],
        )?;
        self.entities.persist_pack(pack)?;

        issue_snapshot = self
            .entities
            .load_entity(&issue_id)
            .and_then(build_issue_snapshot)?;
        Ok(IssueChangeStatusOutcome {
            changed: true,
            snapshot: issue_snapshot,
        })
    }

    pub fn append_comment(
        &self,
        input: AppendIssueCommentInput,
    ) -> Result<AppendIssueCommentOutcome> {
        let AppendIssueCommentInput {
            issue_id,
            replica_id,
            author,
            message,
            comment_id,
            body,
        } = input;

        let snapshot = self.entities.load_entity(&issue_id)?;
        let heads = snapshot.heads.clone();
        let counter = snapshot.clock_snapshot.counter();
        let mut issue_snapshot = build_issue_snapshot(snapshot)?;

        let target_id = match comment_id {
            Some(existing) => {
                if issue_snapshot
                    .comments
                    .iter()
                    .any(|comment| comment.id == existing)
                {
                    return Ok(AppendIssueCommentOutcome {
                        created: false,
                        comment_id: existing,
                        snapshot: issue_snapshot,
                    });
                }
                existing
            }
            None => Uuid::new_v4(),
        };

        let mut clock = LamportClock::with_state(replica_id.clone(), counter);
        let (operation, blob) = build_operation(
            &mut clock,
            heads,
            IssueEventKind::CommentAppended(IssueCommentAppended {
                comment_id: target_id,
                body,
            }),
            author,
            message,
        )?;

        let pack = OperationPack::new(
            issue_id.clone(),
            clock.snapshot(),
            vec![operation],
            vec![blob],
        )?;
        self.entities.persist_pack(pack)?;

        issue_snapshot = self
            .entities
            .load_entity(&issue_id)
            .and_then(build_issue_snapshot)?;
        Ok(AppendIssueCommentOutcome {
            created: true,
            comment_id: target_id,
            snapshot: issue_snapshot,
        })
    }

    pub fn update_labels(&self, input: UpdateIssueLabelsInput) -> Result<UpdateIssueLabelsOutcome> {
        let UpdateIssueLabelsInput {
            issue_id,
            replica_id,
            author,
            message,
            add,
            remove,
        } = input;

        let snapshot = self.entities.load_entity(&issue_id)?;
        let heads = snapshot.heads.clone();
        let counter = snapshot.clock_snapshot.counter();
        let mut issue_snapshot = build_issue_snapshot(snapshot)?;

        let mut additions: Vec<LabelId> = add
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut removals: Vec<LabelId> = remove
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();

        additions.retain(|label| !issue_snapshot.labels.contains(label));
        removals.retain(|label| issue_snapshot.labels.contains(label));

        if additions.is_empty() && removals.is_empty() {
            return Ok(UpdateIssueLabelsOutcome {
                changed: false,
                added: Vec::new(),
                removed: Vec::new(),
                snapshot: issue_snapshot,
            });
        }

        let mut clock = LamportClock::with_state(replica_id.clone(), counter);
        let mut parents = heads;
        let mut operations = Vec::new();
        let mut blobs = Vec::new();

        for label in &removals {
            let (operation, blob) = build_operation(
                &mut clock,
                parents.clone(),
                IssueEventKind::LabelDetached(IssueLabelDetached {
                    label: label.clone(),
                }),
                author.clone(),
                message.clone(),
            )?;
            parents = vec![operation.id.clone()];
            operations.push(operation);
            blobs.push(blob);
        }

        for label in &additions {
            let (operation, blob) = build_operation(
                &mut clock,
                parents.clone(),
                IssueEventKind::LabelAttached(IssueLabelAttached {
                    label: label.clone(),
                }),
                author.clone(),
                message.clone(),
            )?;
            parents = vec![operation.id.clone()];
            operations.push(operation);
            blobs.push(blob);
        }

        let pack = OperationPack::new(issue_id.clone(), clock.snapshot(), operations, blobs)?;
        self.entities.persist_pack(pack)?;

        issue_snapshot = self
            .entities
            .load_entity(&issue_id)
            .and_then(build_issue_snapshot)?;

        Ok(UpdateIssueLabelsOutcome {
            changed: true,
            added: additions,
            removed: removals,
            snapshot: issue_snapshot,
        })
    }
}

fn build_operation(
    clock: &mut LamportClock,
    parents: Vec<OperationId>,
    event: IssueEventKind,
    author: String,
    message: Option<String>,
) -> Result<(Operation, OperationBlob)> {
    let blob = match &event {
        IssueEventKind::Created(data) => encode_event(&StoredEventPayload::Created(data.clone()))?,
        IssueEventKind::StatusChanged(data) => {
            encode_event(&StoredEventPayload::StatusChanged(data.clone()))?
        }
        IssueEventKind::CommentAppended(data) => {
            encode_event(&StoredEventPayload::CommentAppended(data.clone()))?
        }
        IssueEventKind::LabelAttached(data) => {
            encode_event(&StoredEventPayload::LabelAttached(data.clone()))?
        }
        IssueEventKind::LabelDetached(data) => {
            encode_event(&StoredEventPayload::LabelDetached(data.clone()))?
        }
        IssueEventKind::Unknown { .. } => {
            return Err(Error::validation("cannot persist unknown event payload"));
        }
    };

    let timestamp = clock.tick()?;
    let op_id = OperationId::new(timestamp);
    let metadata = OperationMetadata::new(author, message);

    let operation = Operation::new(op_id.clone(), parents, blob.digest().clone(), metadata);

    Ok((operation, blob))
}

fn encode_event(payload: &StoredEventPayload) -> Result<OperationBlob> {
    let (event_type, data_value) = match payload {
        StoredEventPayload::Created(data) => ("created", serde_json::to_value(data)?),
        StoredEventPayload::StatusChanged(data) => ("status_changed", serde_json::to_value(data)?),
        StoredEventPayload::CommentAppended(data) => {
            ("comment_appended", serde_json::to_value(data)?)
        }
        StoredEventPayload::LabelAttached(data) => ("label_attached", serde_json::to_value(data)?),
        StoredEventPayload::LabelDetached(data) => ("label_detached", serde_json::to_value(data)?),
    };

    let value = json!({
        "version": EVENT_VERSION,
        "type": event_type,
        "data": data_value,
    });

    let bytes = serde_json::to_vec(&value)?;
    Ok(OperationBlob::from_bytes(bytes))
}

fn build_issue_snapshot(entity: EntitySnapshot) -> Result<IssueSnapshot> {
    let EntitySnapshot {
        entity_id,
        clock_snapshot,
        heads: _,
        operations,
        blobs,
    } = entity;

    let blob_lookup: HashMap<BlobRef, OperationBlob> = blobs
        .into_iter()
        .map(|blob| (blob.digest().clone(), blob))
        .collect();

    #[derive(Debug)]
    enum Decoded {
        Known {
            payload: StoredEventPayload,
        },
        Unknown {
            version: Option<u8>,
            event_type: Option<String>,
            raw: Value,
        },
    }

    fn decode_event(data: &[u8]) -> Result<Decoded> {
        let value: Value = serde_json::from_slice(data)?;
        let version = value
            .get("version")
            .and_then(|v| v.as_u64())
            .and_then(|v| u8::try_from(v).ok());
        let event_type = value
            .get("type")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string());

        match serde_json::from_value::<StoredEvent>(value.clone()) {
            Ok(record) => Ok(Decoded::Known {
                payload: record.payload,
            }),
            Err(_) => Ok(Decoded::Unknown {
                version,
                event_type,
                raw: value,
            }),
        }
    }

    let mut events = Vec::with_capacity(operations.len());
    for operation in operations {
        let blob = blob_lookup.get(&operation.payload).ok_or_else(|| {
            Error::validation(format!(
                "missing blob for operation payload {}",
                operation.payload
            ))
        })?;

        let decoded = decode_event(blob.data())?;
        let timestamp = LamportTimestamp::from(operation.id.clone());
        let metadata = operation.metadata.clone();

        let payload = match decoded {
            Decoded::Known { payload, .. } => match payload {
                StoredEventPayload::Created(data) => IssueEventKind::Created(data),
                StoredEventPayload::StatusChanged(data) => IssueEventKind::StatusChanged(data),
                StoredEventPayload::CommentAppended(data) => IssueEventKind::CommentAppended(data),
                StoredEventPayload::LabelAttached(data) => IssueEventKind::LabelAttached(data),
                StoredEventPayload::LabelDetached(data) => IssueEventKind::LabelDetached(data),
            },
            Decoded::Unknown {
                version,
                event_type,
                raw,
            } => IssueEventKind::Unknown {
                version,
                event_type,
                raw,
            },
        };

        events.push(IssueEvent {
            id: operation.id,
            timestamp,
            metadata,
            payload,
        });
    }

    if events.is_empty() {
        return Err(Error::validation(format!(
            "issue {} has no events",
            entity_id
        )));
    }

    let mut title: Option<String> = None;
    let mut description: Option<String> = None;
    let mut status: Option<IssueStatus> = None;
    let mut labels: BTreeSet<LabelId> = BTreeSet::new();
    let mut comments: Vec<IssueComment> = Vec::new();
    let mut comment_ids: BTreeSet<CommentId> = BTreeSet::new();

    for event in &events {
        match &event.payload {
            IssueEventKind::Created(data) => {
                title = Some(data.title.clone());
                description = data.description.clone();
                status = Some(data.status);
                labels.extend(data.labels.iter().cloned());
                if let Some(initial_comment) = &data.initial_comment {
                    if comment_ids.insert(initial_comment.comment_id) {
                        comments.push(IssueComment {
                            id: initial_comment.comment_id,
                            body: initial_comment.body.clone(),
                            author: event.metadata.author.clone(),
                            created_at: event.timestamp.clone(),
                            edited_at: None,
                        });
                    }
                }
            }
            IssueEventKind::StatusChanged(data) => {
                status = Some(data.status);
            }
            IssueEventKind::CommentAppended(data) => {
                if comment_ids.insert(data.comment_id) {
                    comments.push(IssueComment {
                        id: data.comment_id,
                        body: data.body.clone(),
                        author: event.metadata.author.clone(),
                        created_at: event.timestamp.clone(),
                        edited_at: None,
                    });
                }
            }
            IssueEventKind::LabelAttached(data) => {
                labels.insert(data.label.clone());
            }
            IssueEventKind::LabelDetached(data) => {
                labels.remove(&data.label);
            }
            IssueEventKind::Unknown { .. } => {}
        }
    }

    let title = title.ok_or_else(|| {
        Error::validation(format!(
            "issue {} missing creation event in history",
            entity_id
        ))
    })?;
    let status = status.ok_or_else(|| {
        Error::validation(format!(
            "issue {} missing resolved status in history",
            entity_id
        ))
    })?;

    let created_at = events
        .first()
        .map(|event| event.timestamp.clone())
        .ok_or_else(|| Error::validation("issue history missing creation timestamp"))?;
    let updated_at = events
        .last()
        .map(|event| event.timestamp.clone())
        .unwrap_or_else(|| created_at.clone());

    Ok(IssueSnapshot {
        id: entity_id,
        title,
        description,
        status,
        labels,
        comments,
        created_at,
        updated_at,
        clock_snapshot,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ReplicaId;
    use crate::identity::{CreateIdentityInput, IdentityStore};
    use tempfile::TempDir;
    use uuid::Uuid;

    fn init_store() -> (TempDir, IssueStore) {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = IssueStore::open(temp.path()).expect("open store");
        (temp, store)
    }

    #[test]
    fn create_issue_persists_initial_event() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_issue(CreateIssueInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: Some("create".into()),
                title: "Initial Issue".into(),
                description: Some("details".into()),
                initial_status: IssueStatus::Draft,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create issue");

        assert_eq!(snapshot.title, "Initial Issue");
        assert_eq!(snapshot.status, IssueStatus::Draft);
        assert_eq!(snapshot.events.len(), 1);
        match &snapshot.events[0].payload {
            IssueEventKind::Created(event) => {
                assert_eq!(event.title, "Initial Issue");
                assert!(event.initial_comment.is_none());
                assert!(event.labels.is_empty());
            }
            _ => panic!("expected created event"),
        }
    }

    #[test]
    fn create_issue_captures_initial_comment_and_labels() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_issue(CreateIssueInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Issue".into(),
                description: Some("details".into()),
                initial_status: IssueStatus::Draft,
                initial_comment: Some("hello world".into()),
                labels: vec!["alpha".into(), "beta".into(), "alpha".into()],
            })
            .expect("create issue");

        assert_eq!(snapshot.comments.len(), 1);
        let comment = &snapshot.comments[0];
        assert_eq!(comment.body, "hello world");
        assert_eq!(comment.author, "tester");

        assert_eq!(snapshot.labels.len(), 2);
        assert!(snapshot.labels.contains("alpha"));
        assert!(snapshot.labels.contains("beta"));

        match &snapshot.events[0].payload {
            IssueEventKind::Created(event) => {
                let created_comment = event
                    .initial_comment
                    .as_ref()
                    .expect("initial comment present");
                assert_eq!(created_comment.body, "hello world");
                assert_eq!(event.labels, vec!["alpha".to_string(), "beta".to_string()]);
            }
            _ => panic!("expected created event"),
        }
    }

    #[test]
    fn change_status_updates_snapshot() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_issue(CreateIssueInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Issue".into(),
                description: None,
                initial_status: IssueStatus::Draft,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create issue");

        let outcome = store
            .change_status(IssueChangeStatusInput {
                issue_id: snapshot.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: Some("open".into()),
                status: IssueStatus::Open,
            })
            .expect("change status");

        assert!(outcome.changed);
        assert_eq!(outcome.snapshot.status, IssueStatus::Open);
        assert_eq!(outcome.snapshot.events.len(), 2);
    }

    #[test]
    fn change_status_is_idempotent() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_issue(CreateIssueInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Issue".into(),
                description: None,
                initial_status: IssueStatus::Open,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create issue");

        let outcome = store
            .change_status(IssueChangeStatusInput {
                issue_id: snapshot.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                status: IssueStatus::Open,
            })
            .expect("change status");

        assert!(!outcome.changed);
        assert_eq!(outcome.snapshot.events.len(), 1);
    }

    #[test]
    fn append_comment_adds_timeline_entry() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_issue(CreateIssueInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Issue".into(),
                description: None,
                initial_status: IssueStatus::Draft,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create issue");

        let outcome = store
            .append_comment(AppendIssueCommentInput {
                issue_id: snapshot.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: Some("comment".into()),
                comment_id: None,
                body: "This is a comment".into(),
            })
            .expect("append comment");

        assert!(outcome.created);
        assert_eq!(outcome.snapshot.comments.len(), 1);
        let comment = &outcome.snapshot.comments[0];
        assert_eq!(comment.body, "This is a comment");
        assert_eq!(comment.author, "tester");

        match outcome.snapshot.events.last().map(|event| &event.payload) {
            Some(IssueEventKind::CommentAppended(data)) => {
                assert_eq!(data.body, "This is a comment");
                assert_eq!(data.comment_id, comment.id);
            }
            other => panic!("expected comment appended event, got {other:?}"),
        }
    }

    #[test]
    fn append_comment_is_idempotent_with_existing_id() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_issue(CreateIssueInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Issue".into(),
                description: None,
                initial_status: IssueStatus::Draft,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create issue");

        let comment_id = Uuid::new_v4();
        let first = store
            .append_comment(AppendIssueCommentInput {
                issue_id: snapshot.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                comment_id: Some(comment_id),
                body: "First".into(),
            })
            .expect("append comment");

        assert!(first.created);
        assert_eq!(first.comment_id, comment_id);

        let second = store
            .append_comment(AppendIssueCommentInput {
                issue_id: snapshot.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                comment_id: Some(comment_id),
                body: "Duplicate".into(),
            })
            .expect("append comment");

        assert!(!second.created);
        assert_eq!(second.comment_id, comment_id);
        assert_eq!(second.snapshot.comments.len(), 1);
        assert_eq!(second.snapshot.comments[0].body, "First");
    }

    #[test]
    fn update_labels_applies_additions_and_removals() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_issue(CreateIssueInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Issue".into(),
                description: None,
                initial_status: IssueStatus::Draft,
                initial_comment: None,
                labels: vec!["alpha".into()],
            })
            .expect("create issue");

        let outcome = store
            .update_labels(UpdateIssueLabelsInput {
                issue_id: snapshot.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: Some("update labels".into()),
                add: vec!["beta".into(), "gamma".into()],
                remove: vec!["alpha".into()],
            })
            .expect("update labels");

        assert!(outcome.changed);
        assert_eq!(
            outcome.added,
            vec![String::from("beta"), String::from("gamma")]
        );
        assert_eq!(outcome.removed, vec![String::from("alpha")]);
        assert!(outcome.snapshot.labels.contains("beta"));
        assert!(outcome.snapshot.labels.contains("gamma"));
        assert!(!outcome.snapshot.labels.contains("alpha"));

        assert!(matches!(
            outcome.snapshot.events.last().map(|event| &event.payload),
            Some(IssueEventKind::LabelAttached(_))
        ));
    }

    #[test]
    fn update_labels_is_noop_when_no_effective_change() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_issue(CreateIssueInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Issue".into(),
                description: None,
                initial_status: IssueStatus::Draft,
                initial_comment: None,
                labels: vec!["alpha".into()],
            })
            .expect("create issue");

        let outcome = store
            .update_labels(UpdateIssueLabelsInput {
                issue_id: snapshot.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                add: vec!["alpha".into()],
                remove: vec!["beta".into()],
            })
            .expect("update labels");

        assert!(!outcome.changed);
        assert!(outcome.added.is_empty());
        assert!(outcome.removed.is_empty());
        assert_eq!(outcome.snapshot.labels, snapshot.labels);
        assert_eq!(outcome.snapshot.events.len(), snapshot.events.len());
    }

    #[test]
    fn list_issues_ignores_identity_entities() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let replica = ReplicaId::new("replica-a");

        {
            let identity_store = IdentityStore::open(temp.path()).expect("open identity store");
            identity_store
                .create_identity(CreateIdentityInput {
                    replica_id: replica.clone(),
                    author: "tester <tester@example.com>".into(),
                    message: None,
                    display_name: "Alice".into(),
                    email: "alice@example.com".into(),
                    login: None,
                    initial_signature: None,
                    adopt_immediately: true,
                    protections: vec![],
                })
                .expect("create identity");
        }

        let store = IssueStore::open(temp.path()).expect("open issue store");
        let issue = store
            .create_issue(CreateIssueInput {
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: None,
                title: "Initial Issue".into(),
                description: None,
                initial_status: IssueStatus::Open,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create issue");

        let issues = store.list_issues().expect("list issues");
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, issue.id);
    }
}
