use std::collections::HashMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clock::{LamportClock, LamportTimestamp, ReplicaId};
use crate::dag::{
    BlobRef, EntityId, EntitySnapshot, EntityStore, Operation, OperationBlob, OperationId,
    OperationMetadata, OperationPack,
};
use crate::error::{Error, Result};

const EVENT_VERSION: u8 = 1;

/// Identifier alias for miles backed by the entity store.
pub type MileId = EntityId;

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
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredEvent {
    #[serde(default)]
    version: u8,
    #[serde(flatten)]
    payload: StoredEventPayload,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MileCreated {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub status: MileStatus,
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
    Unknown {
        version: Option<u8>,
        event_type: Option<String>,
        raw: Value,
    },
}

/// Snapshot describing the current state of a mile.
#[derive(Clone, Debug, Serialize)]
pub struct MileSnapshot {
    pub id: MileId,
    pub title: String,
    pub description: Option<String>,
    pub status: MileStatus,
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

/// High-level interface for mile operations.
pub struct MileStore {
    entities: EntityStore,
}

impl MileStore {
    pub fn open(repo_path: impl AsRef<Path>) -> Result<Self> {
        let entities = EntityStore::open(repo_path)?;
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
        } = input;

        let entity_id = EntityId::new();
        let mut clock = LamportClock::new(replica_id.clone());
        let (operation, blob) = build_operation(
            &mut clock,
            vec![],
            MileEventKind::Created(MileCreated {
                title,
                description,
                status: initial_status,
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
            let snapshot = self.entities.load_entity(&summary.entity_id)?;
            let mile = build_mile_snapshot(snapshot)?;
            miles.push(MileSummary {
                id: mile.id,
                title: mile.title.clone(),
                status: mile.status,
                updated_at: mile.updated_at.clone(),
            });
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

    for event in &events {
        match &event.payload {
            MileEventKind::Created(data) => {
                title = Some(data.title.clone());
                description = data.description.clone();
                status = Some(data.status);
            }
            MileEventKind::StatusChanged(data) => {
                status = Some(data.status);
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
    use tempfile::TempDir;

    fn init_store() -> (TempDir, MileStore) {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = MileStore::open(temp.path()).expect("open store");
        (temp, store)
    }

    #[test]
    fn create_mile_persists_initial_event() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_mile(CreateMileInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: Some("create".into()),
                title: "Initial Mile".into(),
                description: Some("details".into()),
                initial_status: MileStatus::Draft,
            })
            .expect("create mile");

        assert_eq!(snapshot.title, "Initial Mile");
        assert_eq!(snapshot.status, MileStatus::Draft);
        assert_eq!(snapshot.events.len(), 1);
        match &snapshot.events[0].payload {
            MileEventKind::Created(event) => {
                assert_eq!(event.title, "Initial Mile");
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
            })
            .expect("create mile");

        let outcome = store
            .change_status(ChangeStatusInput {
                mile_id: snapshot.id.clone(),
                replica_id: replica.clone(),
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
            })
            .expect("create mile");

        let outcome = store
            .change_status(ChangeStatusInput {
                mile_id: snapshot.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                status: MileStatus::Open,
            })
            .expect("change status");

        assert!(!outcome.changed);
        assert_eq!(outcome.snapshot.events.len(), 1);
    }
}
