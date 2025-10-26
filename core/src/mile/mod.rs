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

const EVENT_VERSION: u8 = 3;

/// Identifier alias for miles backed by the entity store.
pub type MileId = EntityId;

/// Identifier assigned to individual comments.
pub type CommentId = Uuid;

/// Identifier assigned to labels.
pub type LabelId = String;

/// High-level status for a mile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MileStatus {
    Draft,
    Open,
    Closed,
}

impl std::fmt::Display for MileStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            MileStatus::Draft => "draft",
            MileStatus::Open => "open",
            MileStatus::Closed => "closed",
        };
        f.write_str(label)
    }
}

impl std::str::FromStr for MileStatus {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "draft" => Ok(Self::Draft),
            "open" => Ok(Self::Open),
            "closed" => Ok(Self::Closed),
            other => Err(Error::validation(format!("unknown mile status: {other}"))),
        }
    }
}

/// Event timeline entry for a mile.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
enum StoredEventPayload {
    #[serde(rename = "created")]
    Created(MileCreated),
    #[serde(rename = "status_changed")]
    StatusChanged(MileStatusChanged),
    #[serde(rename = "comment_appended")]
    CommentAppended(MileCommentAppended),
    #[serde(rename = "label_attached")]
    LabelAttached(MileLabelAttached),
    #[serde(rename = "label_detached")]
    LabelDetached(MileLabelDetached),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredEvent {
    #[serde(default)]
    version: u8,
    #[serde(flatten)]
    payload: StoredEventPayload,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MileCommentAppended {
    pub comment_id: CommentId,
    pub body: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MileLabelAttached {
    pub label: LabelId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MileLabelDetached {
    pub label: LabelId,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MileCreated {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: MileStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_comment: Option<MileCommentAppended>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<LabelId>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MileStatusChanged {
    pub status: MileStatus,
}

/// Represents a user-facing event in a mile's history.
#[derive(Clone, Debug, Serialize)]
pub struct MileEvent {
    pub id: OperationId,
    pub timestamp: LamportTimestamp,
    pub metadata: OperationMetadata,
    pub payload: MileEventKind,
}

/// High-level event kind presented to callers.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MileEventKind {
    Created(MileCreated),
    StatusChanged(MileStatusChanged),
    CommentAppended(MileCommentAppended),
    LabelAttached(MileLabelAttached),
    LabelDetached(MileLabelDetached),
    Unknown {
        version: Option<u8>,
        event_type: Option<String>,
        raw: Value,
    },
}

/// Snapshot representation of a mile comment.
#[derive(Clone, Debug, Serialize)]
pub struct MileComment {
    pub id: CommentId,
    pub body: String,
    pub author: String,
    pub created_at: LamportTimestamp,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<LamportTimestamp>,
}

/// Snapshot describing the current state of a mile.
#[derive(Clone, Debug, Serialize)]
pub struct MileSnapshot {
    pub id: MileId,
    pub title: String,
    pub description: Option<String>,
    pub status: MileStatus,
    pub labels: BTreeSet<LabelId>,
    pub comments: Vec<MileComment>,
    pub created_at: LamportTimestamp,
    pub updated_at: LamportTimestamp,
    pub clock_snapshot: LamportTimestamp,
    pub events: Vec<MileEvent>,
}

/// Summary data used for listing miles.
#[derive(Clone, Debug, Serialize)]
pub struct MileSummary {
    pub id: MileId,
    pub title: String,
    pub status: MileStatus,
    pub updated_at: LamportTimestamp,
}

/// Request payload for creating a new mile.
#[derive(Clone, Debug)]
pub struct CreateMileInput {
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub title: String,
    pub description: Option<String>,
    pub initial_status: MileStatus,
    pub initial_comment: Option<String>,
    pub labels: Vec<LabelId>,
}

/// Request payload for changing the status of an existing mile.
#[derive(Clone, Debug)]
pub struct ChangeStatusInput {
    pub mile_id: MileId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub status: MileStatus,
}

/// Outcome returned when applying a status change.
#[derive(Clone, Debug, Serialize)]
pub struct ChangeStatusOutcome {
    pub changed: bool,
    pub snapshot: MileSnapshot,
}

/// Request payload for appending a comment to a mile.
#[derive(Clone, Debug)]
pub struct AppendCommentInput {
    pub mile_id: MileId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub comment_id: Option<CommentId>,
    pub body: String,
}

/// Outcome returned after attempting to append a comment.
#[derive(Clone, Debug, Serialize)]
pub struct AppendCommentOutcome {
    pub created: bool,
    pub comment_id: CommentId,
    pub snapshot: MileSnapshot,
}

/// Request payload for updating labels on a mile.
#[derive(Clone, Debug)]
pub struct UpdateLabelsInput {
    pub mile_id: MileId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub add: Vec<LabelId>,
    pub remove: Vec<LabelId>,
}

/// Outcome returned when updating labels on a mile.
#[derive(Clone, Debug, Serialize)]
pub struct UpdateLabelsOutcome {
    pub changed: bool,
    pub added: Vec<LabelId>,
    pub removed: Vec<LabelId>,
    pub snapshot: MileSnapshot,
}

/// High-level interface for mile operations.
pub struct MileStore {
    entities: EntityStore,
}

impl MileStore {
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

    pub fn create_mile(&self, input: CreateMileInput) -> Result<MileSnapshot> {
        let CreateMileInput {
            replica_id,
            author,
            message,
            title,
            description,
            initial_status,
            initial_comment,
            labels,
        } = input;

        let initial_comment = initial_comment.map(|body| MileCommentAppended {
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
            MileEventKind::Created(MileCreated {
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
        build_mile_snapshot(snapshot)
    }

    pub fn load_mile(&self, mile_id: &MileId) -> Result<MileSnapshot> {
        let snapshot = self.entities.load_entity(mile_id)?;
        build_mile_snapshot(snapshot)
    }

    pub fn list_miles(&self) -> Result<Vec<MileSummary>> {
        let summaries = self.entities.list_entities()?;
        let mut miles = Vec::with_capacity(summaries.len());

        for summary in summaries {
            let entity_id = summary.entity_id.clone();
            let snapshot = self.entities.load_entity(&entity_id)?;
            match build_mile_snapshot(snapshot) {
                Ok(mile) => {
                    miles.push(MileSummary {
                        id: mile.id,
                        title: mile.title.clone(),
                        status: mile.status,
                        updated_at: mile.updated_at.clone(),
                    });
                }
                Err(Error::Validation(_)) => {}
                Err(err) => return Err(err),
            }
        }

        miles.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
        miles.reverse();
        Ok(miles)
    }

    pub fn change_status(&self, input: ChangeStatusInput) -> Result<ChangeStatusOutcome> {
        let ChangeStatusInput {
            mile_id,
            replica_id,
            author,
            message,
            status,
        } = input;

        let snapshot = self.entities.load_entity(&mile_id)?;
        let heads = snapshot.heads.clone();
        if heads.len() != 1 {
            return Err(Error::conflict(format!(
                "mile {} has {} heads; resolve conflicts before changing status",
                mile_id,
                heads.len()
            )));
        }

        let counter = snapshot.clock_snapshot.counter();
        let mut mile_snapshot = build_mile_snapshot(snapshot)?;
        if mile_snapshot.status == status {
            return Ok(ChangeStatusOutcome {
                changed: false,
                snapshot: mile_snapshot,
            });
        }

        let mut clock = LamportClock::with_state(replica_id.clone(), counter);
        let event_payload = MileEventKind::StatusChanged(MileStatusChanged { status });
        let (operation, blob) = build_operation(&mut clock, heads, event_payload, author, message)?;

        let pack = OperationPack::new(
            mile_id.clone(),
            clock.snapshot(),
            vec![operation],
            vec![blob],
        )?;
        self.entities.persist_pack(pack)?;

        mile_snapshot = self
            .entities
            .load_entity(&mile_id)
            .and_then(build_mile_snapshot)?;
        Ok(ChangeStatusOutcome {
            changed: true,
            snapshot: mile_snapshot,
        })
    }

    pub fn append_comment(&self, input: AppendCommentInput) -> Result<AppendCommentOutcome> {
        let AppendCommentInput {
            mile_id,
            replica_id,
            author,
            message,
            comment_id,
            body,
        } = input;

        let snapshot = self.entities.load_entity(&mile_id)?;
        let heads = snapshot.heads.clone();
        let counter = snapshot.clock_snapshot.counter();
        let mut mile_snapshot = build_mile_snapshot(snapshot)?;

        let target_id = match comment_id {
            Some(existing) => {
                if mile_snapshot
                    .comments
                    .iter()
                    .any(|comment| comment.id == existing)
                {
                    return Ok(AppendCommentOutcome {
                        created: false,
                        comment_id: existing,
                        snapshot: mile_snapshot,
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
            MileEventKind::CommentAppended(MileCommentAppended {
                comment_id: target_id,
                body,
            }),
            author,
            message,
        )?;

        let pack = OperationPack::new(
            mile_id.clone(),
            clock.snapshot(),
            vec![operation],
            vec![blob],
        )?;
        self.entities.persist_pack(pack)?;

        mile_snapshot = self
            .entities
            .load_entity(&mile_id)
            .and_then(build_mile_snapshot)?;
        Ok(AppendCommentOutcome {
            created: true,
            comment_id: target_id,
            snapshot: mile_snapshot,
        })
    }

    pub fn update_labels(&self, input: UpdateLabelsInput) -> Result<UpdateLabelsOutcome> {
        let UpdateLabelsInput {
            mile_id,
            replica_id,
            author,
            message,
            add,
            remove,
        } = input;

        let snapshot = self.entities.load_entity(&mile_id)?;
        let heads = snapshot.heads.clone();
        let counter = snapshot.clock_snapshot.counter();
        let mut mile_snapshot = build_mile_snapshot(snapshot)?;

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

        additions.retain(|label| !mile_snapshot.labels.contains(label));
        removals.retain(|label| mile_snapshot.labels.contains(label));

        if additions.is_empty() && removals.is_empty() {
            return Ok(UpdateLabelsOutcome {
                changed: false,
                added: Vec::new(),
                removed: Vec::new(),
                snapshot: mile_snapshot,
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
                MileEventKind::LabelDetached(MileLabelDetached {
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
                MileEventKind::LabelAttached(MileLabelAttached {
                    label: label.clone(),
                }),
                author.clone(),
                message.clone(),
            )?;
            parents = vec![operation.id.clone()];
            operations.push(operation);
            blobs.push(blob);
        }

        let pack = OperationPack::new(mile_id.clone(), clock.snapshot(), operations, blobs)?;
        self.entities.persist_pack(pack)?;

        mile_snapshot = self
            .entities
            .load_entity(&mile_id)
            .and_then(build_mile_snapshot)?;

        Ok(UpdateLabelsOutcome {
            changed: true,
            added: additions,
            removed: removals,
            snapshot: mile_snapshot,
        })
    }
}

fn build_operation(
    clock: &mut LamportClock,
    parents: Vec<OperationId>,
    event: MileEventKind,
    author: String,
    message: Option<String>,
) -> Result<(Operation, OperationBlob)> {
    let blob = match &event {
        MileEventKind::Created(data) => encode_event(&StoredEventPayload::Created(data.clone()))?,
        MileEventKind::StatusChanged(data) => {
            encode_event(&StoredEventPayload::StatusChanged(data.clone()))?
        }
        MileEventKind::CommentAppended(data) => {
            encode_event(&StoredEventPayload::CommentAppended(data.clone()))?
        }
        MileEventKind::LabelAttached(data) => {
            encode_event(&StoredEventPayload::LabelAttached(data.clone()))?
        }
        MileEventKind::LabelDetached(data) => {
            encode_event(&StoredEventPayload::LabelDetached(data.clone()))?
        }
        MileEventKind::Unknown { .. } => {
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

fn build_mile_snapshot(entity: EntitySnapshot) -> Result<MileSnapshot> {
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
                StoredEventPayload::Created(data) => MileEventKind::Created(data),
                StoredEventPayload::StatusChanged(data) => MileEventKind::StatusChanged(data),
                StoredEventPayload::CommentAppended(data) => MileEventKind::CommentAppended(data),
                StoredEventPayload::LabelAttached(data) => MileEventKind::LabelAttached(data),
                StoredEventPayload::LabelDetached(data) => MileEventKind::LabelDetached(data),
            },
            Decoded::Unknown {
                version,
                event_type,
                raw,
            } => MileEventKind::Unknown {
                version,
                event_type,
                raw,
            },
        };

        events.push(MileEvent {
            id: operation.id,
            timestamp,
            metadata,
            payload,
        });
    }

    if events.is_empty() {
        return Err(Error::validation(format!(
            "mile {} has no events",
            entity_id
        )));
    }

    let mut title: Option<String> = None;
    let mut description: Option<String> = None;
    let mut status: Option<MileStatus> = None;
    let mut labels: BTreeSet<LabelId> = BTreeSet::new();
    let mut comments: Vec<MileComment> = Vec::new();
    let mut comment_ids: BTreeSet<CommentId> = BTreeSet::new();

    for event in &events {
        match &event.payload {
            MileEventKind::Created(data) => {
                title = Some(data.title.clone());
                description = data.description.clone();
                status = Some(data.status);
                labels.extend(data.labels.iter().cloned());
                if let Some(initial_comment) = &data.initial_comment
                    && comment_ids.insert(initial_comment.comment_id)
                {
                    comments.push(MileComment {
                        id: initial_comment.comment_id,
                        body: initial_comment.body.clone(),
                        author: event.metadata.author.clone(),
                        created_at: event.timestamp.clone(),
                        edited_at: None,
                    });
                }
            }
            MileEventKind::StatusChanged(data) => {
                status = Some(data.status);
            }
            MileEventKind::CommentAppended(data) => {
                if comment_ids.insert(data.comment_id) {
                    comments.push(MileComment {
                        id: data.comment_id,
                        body: data.body.clone(),
                        author: event.metadata.author.clone(),
                        created_at: event.timestamp.clone(),
                        edited_at: None,
                    });
                }
            }
            MileEventKind::LabelAttached(data) => {
                labels.insert(data.label.clone());
            }
            MileEventKind::LabelDetached(data) => {
                labels.remove(&data.label);
            }
            MileEventKind::Unknown { .. } => {}
        }
    }

    let title = title.ok_or_else(|| {
        Error::validation(format!(
            "mile {} missing creation event in history",
            entity_id
        ))
    })?;
    let status = status.ok_or_else(|| {
        Error::validation(format!(
            "mile {} missing resolved status in history",
            entity_id
        ))
    })?;

    let created_at = events
        .first()
        .map(|event| event.timestamp.clone())
        .ok_or_else(|| Error::validation("mile history missing creation timestamp"))?;
    let updated_at = events
        .last()
        .map(|event| event.timestamp.clone())
        .unwrap_or_else(|| created_at.clone());

    Ok(MileSnapshot {
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

    fn init_store() -> (TempDir, MileStore) {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = MileStore::open(temp.path()).expect("open store");
        (temp, store)
    }

    #[test]
    fn create_mile_persists_initial_event() {
        let (_tmp, store) = init_store();

        let snapshot = store
            .create_mile(CreateMileInput {
                replica_id: ReplicaId::new("replica-a"),
                author: "tester".into(),
                message: Some("create".into()),
                title: "Initial Mile".into(),
                description: Some("details".into()),
                initial_status: MileStatus::Draft,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create mile");

        assert_eq!(snapshot.title, "Initial Mile");
        assert_eq!(snapshot.status, MileStatus::Draft);
        assert_eq!(snapshot.events.len(), 1);
        match &snapshot.events[0].payload {
            MileEventKind::Created(event) => {
                assert_eq!(event.title, "Initial Mile");
                assert!(event.initial_comment.is_none());
                assert!(event.labels.is_empty());
            }
            _ => panic!("expected created event"),
        }
    }

    #[test]
    fn create_mile_captures_initial_comment_and_labels() {
        let (_tmp, store) = init_store();

        let snapshot = store
            .create_mile(CreateMileInput {
                replica_id: ReplicaId::new("replica-a"),
                author: "tester".into(),
                message: None,
                title: "Initial Mile".into(),
                description: Some("details".into()),
                initial_status: MileStatus::Draft,
                initial_comment: Some("hello world".into()),
                labels: vec!["alpha".into(), "beta".into(), "alpha".into()],
            })
            .expect("create mile");

        assert_eq!(snapshot.comments.len(), 1);
        let comment = &snapshot.comments[0];
        assert_eq!(comment.body, "hello world");
        assert_eq!(comment.author, "tester");

        assert_eq!(snapshot.labels.len(), 2);
        assert!(snapshot.labels.contains("alpha"));
        assert!(snapshot.labels.contains("beta"));

        match &snapshot.events[0].payload {
            MileEventKind::Created(event) => {
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
            .create_mile(CreateMileInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Mile".into(),
                description: None,
                initial_status: MileStatus::Draft,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create mile");

        let outcome = store
            .change_status(ChangeStatusInput {
                mile_id: snapshot.id.clone(),
                replica_id: replica,
                author: "tester".into(),
                message: Some("open".into()),
                status: MileStatus::Open,
            })
            .expect("change status");

        assert!(outcome.changed);
        assert_eq!(outcome.snapshot.status, MileStatus::Open);
        assert_eq!(outcome.snapshot.events.len(), 2);
    }

    #[test]
    fn change_status_is_idempotent() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_mile(CreateMileInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Mile".into(),
                description: None,
                initial_status: MileStatus::Open,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create mile");

        let outcome = store
            .change_status(ChangeStatusInput {
                mile_id: snapshot.id.clone(),
                replica_id: replica,
                author: "tester".into(),
                message: None,
                status: MileStatus::Open,
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
            .create_mile(CreateMileInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Mile".into(),
                description: None,
                initial_status: MileStatus::Draft,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create mile");

        let outcome = store
            .append_comment(AppendCommentInput {
                mile_id: snapshot.id.clone(),
                replica_id: replica,
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
            Some(MileEventKind::CommentAppended(data)) => {
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
            .create_mile(CreateMileInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Mile".into(),
                description: None,
                initial_status: MileStatus::Draft,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create mile");

        let comment_id = Uuid::new_v4();
        let first = store
            .append_comment(AppendCommentInput {
                mile_id: snapshot.id.clone(),
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
            .append_comment(AppendCommentInput {
                mile_id: snapshot.id.clone(),
                replica_id: replica,
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
            .create_mile(CreateMileInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Mile".into(),
                description: None,
                initial_status: MileStatus::Draft,
                initial_comment: None,
                labels: vec!["alpha".into()],
            })
            .expect("create mile");

        let outcome = store
            .update_labels(UpdateLabelsInput {
                mile_id: snapshot.id.clone(),
                replica_id: replica,
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
            Some(MileEventKind::LabelAttached(_))
        ));
    }

    #[test]
    fn update_labels_is_noop_when_no_effective_change() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_mile(CreateMileInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                title: "Initial Mile".into(),
                description: None,
                initial_status: MileStatus::Draft,
                initial_comment: None,
                labels: vec!["alpha".into()],
            })
            .expect("create mile");

        let outcome = store
            .update_labels(UpdateLabelsInput {
                mile_id: snapshot.id.clone(),
                replica_id: replica,
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
    fn list_miles_ignores_identity_entities() {
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

        let store = MileStore::open(temp.path()).expect("open mile store");
        let mile = store
            .create_mile(CreateMileInput {
                replica_id: replica,
                author: "tester <tester@example.com>".into(),
                message: None,
                title: "Initial Mile".into(),
                description: None,
                initial_status: MileStatus::Open,
                initial_comment: None,
                labels: vec![],
            })
            .expect("create mile");

        let miles = store.list_miles().expect("list miles");
        assert_eq!(miles.len(), 1);
        assert_eq!(miles[0].id, mile.id);
    }
}
