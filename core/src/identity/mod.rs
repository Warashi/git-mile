use std::collections::HashMap;
use std::mem;
use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::clock::{LamportClock, LamportTimestamp, ReplicaId};
use crate::dag::{
    BlobRef, EntityId, EntitySnapshot, EntityStore, Operation, OperationBlob, OperationId,
    OperationMetadata, OperationPack,
};
use crate::error::{Error, Result};
use crate::repo::{LockMode, RepositoryCacheHook};

const EVENT_VERSION: u8 = 1;

/// Identifier alias for identities backed by the entity store.
pub type IdentityId = EntityId;

/// High-level lifecycle state for an identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityStatus {
    PendingAdoption,
    Adopted,
    Protected,
}

impl std::fmt::Display for IdentityStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let label = match self {
            Self::PendingAdoption => "pending_adoption",
            Self::Adopted => "adopted",
            Self::Protected => "protected",
        };
        f.write_str(label)
    }
}

/// Supported protection mechanisms for an identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtectionKind {
    Pgp,
}

/// Protection metadata associated with an identity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct IdentityProtection {
    pub kind: ProtectionKind,
    pub fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub armored_public_key: Option<String>,
}

/// Stored event payloads for identities.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
enum StoredEventPayload {
    #[serde(rename = "created")]
    Created(IdentityCreated),
    #[serde(rename = "adopted")]
    Adopted(IdentityAdopted),
    #[serde(rename = "protection_added")]
    ProtectionAdded(IdentityProtectionAdded),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct StoredEvent {
    #[serde(default)]
    version: u8,
    #[serde(flatten)]
    payload: StoredEventPayload,
}

/// Event emitted when an identity is created.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityCreated {
    pub display_name: String,
    pub email: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_signature: Option<String>,
}

/// Event emitted when an identity is adopted by a replica.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityAdopted {
    pub replica_id: ReplicaId,
    pub signature: String,
}

/// Event emitted when a protection is attached to an identity.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IdentityProtectionAdded {
    pub protection: IdentityProtection,
}

/// Represents an event in an identity's timeline.
#[derive(Clone, Debug, Serialize)]
pub struct IdentityEvent {
    pub id: OperationId,
    pub timestamp: LamportTimestamp,
    pub metadata: OperationMetadata,
    pub payload: IdentityEventKind,
}

/// High-level event kinds surfaced to callers.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum IdentityEventKind {
    Created(IdentityCreated),
    Adopted(IdentityAdopted),
    ProtectionAdded(IdentityProtectionAdded),
    Unknown {
        version: Option<u8>,
        event_type: Option<String>,
        raw: Value,
    },
}

/// Snapshot describing current state of an identity.
#[derive(Clone, Debug, Serialize)]
pub struct IdentitySnapshot {
    pub id: IdentityId,
    pub display_name: String,
    pub email: String,
    pub login: Option<String>,
    pub status: IdentityStatus,
    pub adopted_by: Option<ReplicaId>,
    pub signature: Option<String>,
    pub protections: Vec<IdentityProtection>,
    pub created_at: LamportTimestamp,
    pub updated_at: LamportTimestamp,
    pub clock_snapshot: LamportTimestamp,
    pub events: Vec<IdentityEvent>,
}

/// Summary data used for listing identities.
#[derive(Clone, Debug, Serialize)]
pub struct IdentitySummary {
    pub id: IdentityId,
    pub display_name: String,
    pub status: IdentityStatus,
    pub adopted_by: Option<ReplicaId>,
    pub updated_at: LamportTimestamp,
}

/// Request payload for creating a new identity.
#[derive(Clone, Debug)]
pub struct CreateIdentityInput {
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub display_name: String,
    pub email: String,
    pub login: Option<String>,
    pub initial_signature: Option<String>,
    pub adopt_immediately: bool,
    pub protections: Vec<IdentityProtection>,
}

/// Request payload for adopting an identity.
#[derive(Clone, Debug)]
pub struct AdoptIdentityInput {
    pub identity_id: IdentityId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub signature: String,
}

/// Outcome returned when attempting to adopt an identity.
#[derive(Clone, Debug, Serialize)]
pub struct AdoptIdentityOutcome {
    pub changed: bool,
    pub snapshot: IdentitySnapshot,
}

/// Request payload for adding a protection to an identity.
#[derive(Clone, Debug)]
pub struct AddProtectionInput {
    pub identity_id: IdentityId,
    pub replica_id: ReplicaId,
    pub author: String,
    pub message: Option<String>,
    pub protection: IdentityProtection,
}

/// Outcome returned when attempting to add a protection.
#[derive(Clone, Debug, Serialize)]
pub struct AddProtectionOutcome {
    pub changed: bool,
    pub snapshot: IdentitySnapshot,
}

/// High-level interface for identity operations.
pub struct IdentityStore {
    entities: EntityStore,
}

impl IdentityStore {
    /// Open the identity store for the given repository.
    ///
    /// # Errors
    ///
    /// Returns an error when the repository cannot be accessed or its metadata cannot be loaded.
    pub fn open(repo_path: impl AsRef<Path>) -> Result<Self> {
        let entities = EntityStore::open(repo_path)?;
        Ok(Self { entities })
    }

    /// Open the identity store using the specified repository lock mode.
    ///
    /// # Errors
    ///
    /// Returns an error when the repository cannot be accessed or the requested lock cannot be
    /// acquired.
    pub fn open_with_mode(repo_path: impl AsRef<Path>, mode: LockMode) -> Result<Self> {
        let entities = EntityStore::open_with_mode(repo_path, mode)?;
        Ok(Self { entities })
    }

    /// Open the identity store with a cache hook used to accelerate entity lookups.
    ///
    /// # Errors
    ///
    /// Returns an error when the repository cannot be accessed or when the cache initialization
    /// fails.
    pub fn open_with_cache(
        repo_path: impl AsRef<Path>,
        mode: LockMode,
        cache: Arc<dyn RepositoryCacheHook>,
    ) -> Result<Self> {
        let entities = EntityStore::open_with_cache(repo_path, mode, cache)?;
        Ok(Self { entities })
    }

    /// Create a new identity and persist the corresponding operation pack.
    ///
    /// # Errors
    ///
    /// Returns an error when the store fails to build or persist the operations, or when the final
    /// snapshot cannot be reconstructed.
    pub fn create_identity(&self, input: CreateIdentityInput) -> Result<IdentitySnapshot> {
        let CreateIdentityInput {
            replica_id,
            author,
            message,
            display_name,
            email,
            login,
            initial_signature,
            adopt_immediately,
            protections,
        } = input;

        let entity_id = IdentityId::new();
        let mut clock = LamportClock::new(replica_id.clone());
        let mut operations = Vec::new();
        let mut blobs = Vec::new();
        let mut parents: Vec<OperationId> = Vec::new();

        let (operation, blob) = build_operation(
            &mut clock,
            mem::take(&mut parents),
            IdentityEventKind::Created(IdentityCreated {
                display_name: display_name.clone(),
                email: email.clone(),
                login,
                initial_signature: initial_signature.clone(),
            }),
            author.clone(),
            message.clone(),
        )?;
        parents = vec![operation.id.clone()];
        operations.push(operation);
        blobs.push(blob);

        if adopt_immediately {
            let signature = initial_signature
                .as_ref()
                .map_or_else(|| format!("{display_name} <{email}>"), Clone::clone);
            let (operation, blob) = build_operation(
                &mut clock,
                mem::take(&mut parents),
                IdentityEventKind::Adopted(IdentityAdopted {
                    replica_id,
                    signature,
                }),
                author.clone(),
                message.clone(),
            )?;
            parents = vec![operation.id.clone()];
            operations.push(operation);
            blobs.push(blob);
        }

        for protection in protections {
            let (operation, blob) = build_operation(
                &mut clock,
                mem::take(&mut parents),
                IdentityEventKind::ProtectionAdded(IdentityProtectionAdded { protection }),
                author.clone(),
                message.clone(),
            )?;
            parents = vec![operation.id.clone()];
            operations.push(operation);
            blobs.push(blob);
        }

        let pack = OperationPack::new(entity_id.clone(), clock.snapshot(), operations, blobs)?;
        self.entities.persist_pack(pack)?;

        let snapshot = self.entities.load_entity(&entity_id)?;
        build_identity_snapshot(snapshot)
    }

    /// Load a single identity snapshot by its identifier.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying entity cannot be read or the snapshot reconstruction
    /// fails validation.
    pub fn load_identity(&self, identity_id: &IdentityId) -> Result<IdentitySnapshot> {
        let snapshot = self.entities.load_entity(identity_id)?;
        build_identity_snapshot(snapshot)
    }

    /// Retrieve all identities known to the store.
    ///
    /// # Errors
    ///
    /// Returns an error when the underlying entities cannot be listed or when a snapshot fails to
    /// deserialize.
    pub fn list_identities(&self) -> Result<Vec<IdentitySummary>> {
        let summaries = self.entities.list_entities()?;
        let mut identities = Vec::with_capacity(summaries.len());

        for summary in summaries {
            let entity_id = summary.entity_id.clone();
            let snapshot = self.entities.load_entity(&entity_id)?;
            match build_identity_snapshot(snapshot) {
                Ok(identity) => {
                    identities.push(IdentitySummary {
                        id: identity.id,
                        display_name: identity.display_name,
                        status: identity.status,
                        adopted_by: identity.adopted_by.clone(),
                        updated_at: identity.updated_at.clone(),
                    });
                }
                Err(Error::Validation(_)) => {}
                Err(err) => return Err(err),
            }
        }

        identities.sort_by(|a, b| a.updated_at.cmp(&b.updated_at));
        identities.reverse();
        Ok(identities)
    }

    /// Adopt a pending identity for the specified replica.
    ///
    /// # Errors
    ///
    /// Returns an error when the identity cannot be loaded, is in an unexpected state, or when the
    /// adoption operation fails to persist.
    pub fn adopt_identity(&self, input: AdoptIdentityInput) -> Result<AdoptIdentityOutcome> {
        let AdoptIdentityInput {
            identity_id,
            replica_id,
            author,
            message,
            signature,
        } = input;

        let snapshot = self.entities.load_entity(&identity_id)?;
        let heads = snapshot.heads.clone();
        if heads.len() != 1 {
            return Err(Error::conflict(format!(
                "identity {identity_id} has {} heads; resolve conflicts before adopting",
                heads.len()
            )));
        }

        let counter = snapshot.clock_snapshot.counter();
        let identity_snapshot = build_identity_snapshot(snapshot)?;
        if identity_snapshot.status != IdentityStatus::PendingAdoption {
            return Err(Error::validation(format!(
                "identity {identity_id} already adopted"
            )));
        }

        let mut clock = LamportClock::with_state(replica_id.clone(), counter);
        let (operation, blob) = build_operation(
            &mut clock,
            heads,
            IdentityEventKind::Adopted(IdentityAdopted {
                replica_id,
                signature,
            }),
            author,
            message,
        )?;

        let pack = OperationPack::new(
            identity_id.clone(),
            clock.snapshot(),
            vec![operation],
            vec![blob],
        )?;
        self.entities.persist_pack(pack)?;

        let updated = self.entities.load_entity(&identity_id)?;
        let snapshot = build_identity_snapshot(updated)?;
        Ok(AdoptIdentityOutcome {
            changed: true,
            snapshot,
        })
    }

    /// Add a protection rule to an identity.
    ///
    /// # Errors
    ///
    /// Returns an error when the identity cannot be loaded, the protection operation fails
    /// validation, or the resulting pack cannot be persisted.
    pub fn add_protection(&self, input: AddProtectionInput) -> Result<AddProtectionOutcome> {
        let AddProtectionInput {
            identity_id,
            replica_id,
            author,
            message,
            protection,
        } = input;

        let snapshot = self.entities.load_entity(&identity_id)?;
        let heads = snapshot.heads.clone();
        if heads.len() != 1 {
            return Err(Error::conflict(format!(
                "identity {identity_id} has {} heads; resolve conflicts before adding protection",
                heads.len()
            )));
        }

        let counter = snapshot.clock_snapshot.counter();
        let identity_snapshot = build_identity_snapshot(snapshot)?;
        if identity_snapshot.status == IdentityStatus::PendingAdoption {
            return Err(Error::validation(format!(
                "identity {identity_id} must be adopted before protections can be added"
            )));
        }

        if identity_snapshot
            .protections
            .iter()
            .any(|existing| existing == &protection)
        {
            return Ok(AddProtectionOutcome {
                changed: false,
                snapshot: identity_snapshot,
            });
        }

        let mut clock = LamportClock::with_state(replica_id, counter);
        let (operation, blob) = build_operation(
            &mut clock,
            heads,
            IdentityEventKind::ProtectionAdded(IdentityProtectionAdded { protection }),
            author,
            message,
        )?;

        let pack = OperationPack::new(
            identity_id.clone(),
            clock.snapshot(),
            vec![operation],
            vec![blob],
        )?;
        self.entities.persist_pack(pack)?;

        let updated = self.entities.load_entity(&identity_id)?;
        let snapshot = build_identity_snapshot(updated)?;
        Ok(AddProtectionOutcome {
            changed: true,
            snapshot,
        })
    }

    /// Find the identity adopted by the given replica, if any.
    ///
    /// # Errors
    ///
    /// Returns an error when the repository cannot be queried or when the retrieved snapshot fails
    /// to deserialize.
    pub fn find_adopted_by_replica(&self, replica: &ReplicaId) -> Result<Option<IdentitySnapshot>> {
        let summaries = self.entities.list_entities()?;
        let mut latest: Option<IdentitySnapshot> = None;

        for summary in summaries {
            let entity_id = summary.entity_id.clone();
            let snapshot = self.entities.load_entity(&entity_id)?;
            let identity = match build_identity_snapshot(snapshot) {
                Ok(identity) => identity,
                Err(Error::Validation(_)) => continue,
                Err(err) => return Err(err),
            };

            if identity
                .adopted_by
                .as_ref()
                .is_some_and(|value| value == replica)
            {
                let is_newer = latest
                    .as_ref()
                    .is_none_or(|current| identity.updated_at > current.updated_at);
                if is_newer {
                    latest = Some(identity);
                }
            }
        }

        Ok(latest)
    }
}

fn build_operation(
    clock: &mut LamportClock,
    parents: Vec<OperationId>,
    event: IdentityEventKind,
    author: String,
    message: Option<String>,
) -> Result<(Operation, OperationBlob)> {
    let payload = match event {
        IdentityEventKind::Created(data) => StoredEventPayload::Created(data),
        IdentityEventKind::Adopted(data) => StoredEventPayload::Adopted(data),
        IdentityEventKind::ProtectionAdded(data) => StoredEventPayload::ProtectionAdded(data),
        IdentityEventKind::Unknown { .. } => {
            return Err(Error::validation(
                "cannot persist unknown identity event payload",
            ));
        }
    };
    let blob = encode_event(&payload)?;

    let timestamp = clock.tick()?;
    let op_id = OperationId::new(timestamp);
    let metadata = OperationMetadata::new(author, message);
    let operation = Operation::new(op_id.clone(), parents, blob.digest().clone(), metadata);

    Ok((operation, blob))
}

fn encode_event(payload: &StoredEventPayload) -> Result<OperationBlob> {
    let (event_type, data_value) = match payload {
        StoredEventPayload::Created(data) => ("created", serde_json::to_value(data)?),
        StoredEventPayload::Adopted(data) => ("adopted", serde_json::to_value(data)?),
        StoredEventPayload::ProtectionAdded(data) => {
            ("protection_added", serde_json::to_value(data)?)
        }
    };

    let value = json!({
        "version": EVENT_VERSION,
        "type": event_type,
        "data": data_value,
    });

    let bytes = serde_json::to_vec(&value)?;
    Ok(OperationBlob::from_bytes(bytes))
}

fn build_identity_snapshot(entity: EntitySnapshot) -> Result<IdentitySnapshot> {
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
            .and_then(Value::as_u64)
            .and_then(|v| u8::try_from(v).ok());
        let event_type = value
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_string);

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
            Decoded::Known { payload } => match payload {
                StoredEventPayload::Created(data) => IdentityEventKind::Created(data),
                StoredEventPayload::Adopted(data) => IdentityEventKind::Adopted(data),
                StoredEventPayload::ProtectionAdded(data) => {
                    IdentityEventKind::ProtectionAdded(data)
                }
            },
            Decoded::Unknown {
                version,
                event_type,
                raw,
            } => IdentityEventKind::Unknown {
                version,
                event_type,
                raw,
            },
        };

        events.push(IdentityEvent {
            id: operation.id,
            timestamp,
            metadata,
            payload,
        });
    }

    if events.is_empty() {
        return Err(Error::validation(format!(
            "identity {entity_id} has no events"
        )));
    }

    let mut display_name: Option<String> = None;
    let mut email: Option<String> = None;
    let mut login: Option<String> = None;
    let mut status: Option<IdentityStatus> = None;
    let mut adopted_by: Option<ReplicaId> = None;
    let mut signature: Option<String> = None;
    let mut protections: Vec<IdentityProtection> = Vec::new();

    for event in &events {
        match &event.payload {
            IdentityEventKind::Created(data) => {
                if display_name.is_some() {
                    return Err(Error::validation(format!(
                        "identity {entity_id} has multiple creation events"
                    )));
                }
                display_name = Some(data.display_name.clone());
                email = Some(data.email.clone());
                login = data.login.clone();
                signature = data.initial_signature.clone();
                status = Some(IdentityStatus::PendingAdoption);
            }
            IdentityEventKind::Adopted(data) => {
                if adopted_by.is_some() {
                    return Err(Error::validation(format!(
                        "identity {entity_id} has multiple adoption events"
                    )));
                }
                adopted_by = Some(data.replica_id.clone());
                signature = Some(data.signature.clone());
                status = Some(IdentityStatus::Adopted);
            }
            IdentityEventKind::ProtectionAdded(data) => {
                if status != Some(IdentityStatus::Adopted)
                    && status != Some(IdentityStatus::Protected)
                {
                    return Err(Error::validation(format!(
                        "identity {entity_id} protection added before adoption"
                    )));
                }

                if !protections.contains(&data.protection) {
                    protections.push(data.protection.clone());
                }
                status = Some(IdentityStatus::Protected);
            }
            IdentityEventKind::Unknown { .. } => {}
        }
    }

    let display_name = display_name.ok_or_else(|| {
        Error::validation(format!(
            "identity {entity_id} missing creation event in history"
        ))
    })?;
    let email = email.ok_or_else(|| {
        Error::validation(format!("identity {entity_id} missing email in history"))
    })?;
    let status = status.ok_or_else(|| {
        Error::validation(format!(
            "identity {entity_id} missing resolved status in history"
        ))
    })?;

    let created_at = events
        .first()
        .map(|event| event.timestamp.clone())
        .ok_or_else(|| Error::validation("identity history missing creation timestamp"))?;
    let updated_at = events
        .last()
        .map_or_else(|| created_at.clone(), |event| event.timestamp.clone());

    Ok(IdentitySnapshot {
        id: entity_id,
        display_name,
        email,
        login,
        status,
        adopted_by,
        signature,
        protections,
        created_at,
        updated_at,
        clock_snapshot,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mile::{CreateMileInput, MileStatus, MileStore};
    use tempfile::TempDir;

    fn init_store() -> (TempDir, IdentityStore) {
        let temp = tempfile::tempdir().expect("create temp dir");
        let store = IdentityStore::open(temp.path()).expect("open store");
        (temp, store)
    }

    #[test]
    fn create_identity_initial_state() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let snapshot = store
            .create_identity(CreateIdentityInput {
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: Some("create identity".into()),
                display_name: "Alice".into(),
                email: "alice@example.com".into(),
                login: Some("alice".into()),
                initial_signature: None,
                adopt_immediately: false,
                protections: vec![],
            })
            .expect("create identity");

        assert_eq!(snapshot.display_name, "Alice");
        assert_eq!(snapshot.status, IdentityStatus::PendingAdoption);
        assert!(snapshot.adopted_by.is_none());
        assert!(snapshot.signature.is_none());
        assert!(snapshot.protections.is_empty());
    }

    #[test]
    fn adopt_identity_updates_status() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let identity = store
            .create_identity(CreateIdentityInput {
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: None,
                display_name: "Alice".into(),
                email: "alice@example.com".into(),
                login: None,
                initial_signature: None,
                adopt_immediately: false,
                protections: vec![],
            })
            .expect("create identity");

        let signature = "Alice <alice@example.com>".to_string();
        let outcome = store
            .adopt_identity(AdoptIdentityInput {
                identity_id: identity.id.clone(),
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: Some("adopt identity".into()),
                signature: signature.clone(),
            })
            .expect("adopt identity");

        assert!(outcome.changed);
        assert_eq!(outcome.snapshot.status, IdentityStatus::Adopted);
        assert_eq!(outcome.snapshot.adopted_by, Some(replica.clone()));
        assert_eq!(
            outcome.snapshot.signature.as_deref(),
            Some(signature.as_str())
        );
    }

    #[test]
    fn duplicate_adoption_is_rejected() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let identity = store
            .create_identity(CreateIdentityInput {
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: None,
                display_name: "Alice".into(),
                email: "alice@example.com".into(),
                login: None,
                initial_signature: None,
                adopt_immediately: false,
                protections: vec![],
            })
            .expect("create identity");

        store
            .adopt_identity(AdoptIdentityInput {
                identity_id: identity.id.clone(),
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: None,
                signature: "Alice <alice@example.com>".into(),
            })
            .expect("adopt identity");

        let result = store.adopt_identity(AdoptIdentityInput {
            identity_id: identity.id.clone(),
            replica_id: replica.clone(),
            author: "tester <tester@example.com>".into(),
            message: None,
            signature: "Alice <alice@example.com>".into(),
        });

        assert!(matches!(result, Err(Error::Validation(_))));
    }

    #[test]
    fn protection_requires_adoption() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let identity = store
            .create_identity(CreateIdentityInput {
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: None,
                display_name: "Alice".into(),
                email: "alice@example.com".into(),
                login: None,
                initial_signature: None,
                adopt_immediately: false,
                protections: vec![],
            })
            .expect("create identity");

        let result = store.add_protection(AddProtectionInput {
            identity_id: identity.id.clone(),
            replica_id: replica.clone(),
            author: "tester <tester@example.com>".into(),
            message: None,
            protection: IdentityProtection {
                kind: ProtectionKind::Pgp,
                fingerprint: "FP".into(),
                armored_public_key: None,
            },
        });

        assert!(matches!(result, Err(Error::Validation(_))));
    }

    #[test]
    fn duplicate_protection_is_idempotent() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let identity = store
            .create_identity(CreateIdentityInput {
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: None,
                display_name: "Alice".into(),
                email: "alice@example.com".into(),
                login: None,
                initial_signature: None,
                adopt_immediately: false,
                protections: vec![],
            })
            .expect("create identity");

        store
            .adopt_identity(AdoptIdentityInput {
                identity_id: identity.id.clone(),
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: None,
                signature: "Alice <alice@example.com>".into(),
            })
            .expect("adopt identity");

        let first = store
            .add_protection(AddProtectionInput {
                identity_id: identity.id.clone(),
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: None,
                protection: IdentityProtection {
                    kind: ProtectionKind::Pgp,
                    fingerprint: "FP".into(),
                    armored_public_key: Some("KEY".into()),
                },
            })
            .expect("add protection");
        assert!(first.changed);
        assert_eq!(first.snapshot.protections.len(), 1);

        let second = store
            .add_protection(AddProtectionInput {
                identity_id: identity.id.clone(),
                replica_id: replica.clone(),
                author: "tester <tester@example.com>".into(),
                message: None,
                protection: IdentityProtection {
                    kind: ProtectionKind::Pgp,
                    fingerprint: "FP".into(),
                    armored_public_key: Some("KEY".into()),
                },
            })
            .expect("add protection duplicate");

        assert!(!second.changed);
        assert_eq!(second.snapshot.protections.len(), 1);
    }

    #[test]
    fn find_adopted_returns_latest_snapshot() {
        let (_tmp, store) = init_store();
        let replica = ReplicaId::new("replica-a");

        let first = store
            .create_identity(CreateIdentityInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                display_name: "First".into(),
                email: "first@example.com".into(),
                login: None,
                initial_signature: None,
                adopt_immediately: false,
                protections: vec![],
            })
            .expect("create identity A");

        store
            .adopt_identity(AdoptIdentityInput {
                identity_id: first.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                signature: "First <first@example.com>".into(),
            })
            .expect("adopt identity A");

        let second = store
            .create_identity(CreateIdentityInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                display_name: "Second".into(),
                email: "second@example.com".into(),
                login: None,
                initial_signature: None,
                adopt_immediately: false,
                protections: vec![],
            })
            .expect("create identity B");

        store
            .adopt_identity(AdoptIdentityInput {
                identity_id: second.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                signature: "Second <second@example.com>".into(),
            })
            .expect("adopt identity B");

        store
            .add_protection(AddProtectionInput {
                identity_id: second.id.clone(),
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                protection: IdentityProtection {
                    kind: ProtectionKind::Pgp,
                    fingerprint: "FP-B".into(),
                    armored_public_key: None,
                },
            })
            .expect("protect identity B");

        let resolved = store
            .find_adopted_by_replica(&replica)
            .expect("find adopted")
            .expect("identity present");
        assert_eq!(resolved.id, second.id);
    }

    #[test]
    fn list_identities_skips_other_entities() {
        let temp = tempfile::tempdir().expect("create temp dir");
        let replica = ReplicaId::new("replica-a");

        {
            let mile_store = MileStore::open(temp.path()).expect("open mile store");
            mile_store
                .create_mile(CreateMileInput {
                    replica_id: replica.clone(),
                    author: "tester".into(),
                    message: None,
                    title: "First Mile".into(),
                    description: None,
                    initial_status: MileStatus::Open,
                    initial_comment: None,
                    labels: vec![],
                })
                .expect("create mile");
        }

        let store = IdentityStore::open(temp.path()).expect("open identity store");
        store
            .create_identity(CreateIdentityInput {
                replica_id: replica.clone(),
                author: "tester".into(),
                message: None,
                display_name: "Alice".into(),
                email: "alice@example.com".into(),
                login: None,
                initial_signature: None,
                adopt_immediately: true,
                protections: vec![],
            })
            .expect("create identity");

        let identities = store.list_identities().expect("list identities");
        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].display_name, "Alice");
    }
}
