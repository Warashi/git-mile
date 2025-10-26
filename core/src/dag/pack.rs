use std::collections::HashSet;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::clock::LamportTimestamp;
use crate::error::{Error, Result};

use super::entity::{EntityId, OperationId};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct BlobRef(String);

impl BlobRef {
    /// Create a blob reference from a hexadecimal digest.
    ///
    /// # Errors
    ///
    /// Returns an error when the provided digest is not a 64-character hexadecimal string.
    pub fn new(digest: impl AsRef<str>) -> Result<Self> {
        let digest = digest.as_ref();
        if digest.len() != 64 || !digest.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(Error::validation(format!(
                "blob digest must be 64 hex characters: {digest}"
            )));
        }

        Ok(Self(digest.to_ascii_lowercase()))
    }

    #[must_use]
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let digest = Sha256::digest(bytes);
        Self(hex::encode(digest))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for BlobRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for BlobRef {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self> {
        Self::new(s)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationMetadata {
    pub author: String,
    pub message: Option<String>,
}

impl OperationMetadata {
    #[must_use]
    pub fn new(author: impl Into<String>, message: impl Into<Option<String>>) -> Self {
        Self {
            author: author.into(),
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Operation {
    pub id: OperationId,
    pub parents: Vec<OperationId>,
    pub payload: BlobRef,
    pub metadata: OperationMetadata,
}

impl Operation {
    #[must_use]
    pub const fn new(
        id: OperationId,
        parents: Vec<OperationId>,
        payload: BlobRef,
        metadata: OperationMetadata,
    ) -> Self {
        Self {
            id,
            parents,
            payload,
            metadata,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OperationBlob {
    digest: BlobRef,
    #[serde(with = "serde_bytes")]
    data: Vec<u8>,
}

impl OperationBlob {
    #[must_use]
    pub fn from_bytes(data: Vec<u8>) -> Self {
        let digest = BlobRef::from_bytes(&data);
        Self { digest, data }
    }

    #[must_use]
    pub const fn digest(&self) -> &BlobRef {
        &self.digest
    }

    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OperationPack {
    pub entity_id: EntityId,
    pub clock_snapshot: LamportTimestamp,
    pub operations: Vec<Operation>,
    pub content_blobs: Vec<OperationBlob>,
}

impl OperationPack {
    /// Build a validated operation pack.
    ///
    /// # Errors
    ///
    /// Returns an error when the pack fails validation, such as containing duplicate operations or
    /// missing referenced blobs.
    pub fn new(
        entity_id: EntityId,
        clock_snapshot: LamportTimestamp,
        operations: Vec<Operation>,
        content_blobs: Vec<OperationBlob>,
    ) -> Result<Self> {
        let pack = Self {
            entity_id,
            clock_snapshot,
            operations,
            content_blobs,
        };

        pack.validate()?;
        Ok(pack)
    }

    /// Validate the internal consistency of the pack.
    ///
    /// # Errors
    ///
    /// Returns an error when duplicate operations are present, dependencies are unordered, or
    /// referenced blobs are missing.
    pub fn validate(&self) -> Result<()> {
        self.ensure_unique_operations()?;
        self.ensure_topological_order()?;
        self.ensure_payload_blobs_present()?;
        self.ensure_blob_uniqueness()?;
        Ok(())
    }

    fn ensure_unique_operations(&self) -> Result<()> {
        let mut ids = HashSet::new();
        for op in &self.operations {
            if !ids.insert(op.id.clone()) {
                return Err(Error::validation(format!(
                    "duplicate operation id detected: {}",
                    op.id
                )));
            }
        }

        Ok(())
    }

    fn ensure_topological_order(&self) -> Result<()> {
        let pack_ids: HashSet<_> = self.operations.iter().map(|op| op.id.clone()).collect();
        let mut seen = HashSet::new();

        for op in &self.operations {
            for parent in &op.parents {
                if pack_ids.contains(parent) && !seen.contains(parent) {
                    return Err(Error::validation(format!(
                        "operation {} references parent {} that appears later in the pack",
                        op.id, parent
                    )));
                }
            }
            seen.insert(op.id.clone());
        }

        Ok(())
    }

    fn ensure_blob_uniqueness(&self) -> Result<()> {
        let mut digests = HashSet::new();
        for blob in &self.content_blobs {
            if !digests.insert(blob.digest.clone()) {
                return Err(Error::validation(format!(
                    "duplicate blob digest detected: {}",
                    blob.digest()
                )));
            }
        }

        Ok(())
    }

    fn ensure_payload_blobs_present(&self) -> Result<()> {
        let blob_index: HashSet<_> = self
            .content_blobs
            .iter()
            .map(|blob| blob.digest.clone())
            .collect();

        for op in &self.operations {
            if !blob_index.contains(&op.payload) {
                return Err(Error::validation(format!(
                    "operation {} references missing payload blob {}",
                    op.id, op.payload
                )));
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{LamportClock, ReplicaId};
    use crate::dag::entity::OperationId;

    fn sample_blob(data: &[u8]) -> OperationBlob {
        OperationBlob::from_bytes(data.to_vec())
    }

    fn sample_operation(
        clock: &mut LamportClock,
        parents: Vec<OperationId>,
        blob: &OperationBlob,
    ) -> Operation {
        let id = OperationId::new(clock.tick().unwrap());
        Operation::new(
            id,
            parents,
            blob.digest().clone(),
            OperationMetadata::new("tester", Some("op".to_string())),
        )
    }

    #[test]
    fn operation_pack_rejects_duplicate_operations() {
        let entity = EntityId::new();
        let mut clock = LamportClock::new(ReplicaId::new("replica"));
        let blob = sample_blob(b"payload");
        let op = sample_operation(&mut clock, vec![], &blob);

        let pack = OperationPack {
            entity_id: entity,
            clock_snapshot: clock.snapshot(),
            operations: vec![op.clone(), op],
            content_blobs: vec![blob],
        };

        assert!(pack.validate().is_err());
    }

    #[test]
    fn operation_pack_enforces_topological_order_for_internal_parents() {
        let entity = EntityId::new();
        let mut clock = LamportClock::new(ReplicaId::new("replica"));
        let blob1 = sample_blob(b"a");
        let blob2 = sample_blob(b"b");

        let op1 = sample_operation(&mut clock, vec![], &blob1);
        let op2 = sample_operation(&mut clock, vec![op1.id.clone()], &blob2);

        let pack = OperationPack {
            entity_id: entity,
            clock_snapshot: clock.snapshot(),
            operations: vec![op2, op1],
            content_blobs: vec![blob1, blob2],
        };

        assert!(pack.validate().is_err());
    }

    #[test]
    fn operation_pack_allows_external_parents() {
        let entity = EntityId::new();
        let mut clock = LamportClock::new(ReplicaId::new("replica"));
        let blob = sample_blob(b"a");
        let external_parent = OperationId::new(LamportTimestamp::new(1, ReplicaId::new("other")));

        let op = sample_operation(&mut clock, vec![external_parent], &blob);

        let pack = OperationPack {
            entity_id: entity,
            clock_snapshot: clock.snapshot(),
            operations: vec![op],
            content_blobs: vec![blob],
        };

        assert!(pack.validate().is_ok());
    }

    #[test]
    fn operation_pack_requires_payload_blobs() {
        let entity = EntityId::new();
        let mut clock = LamportClock::new(ReplicaId::new("replica"));
        let blob = sample_blob(b"a");
        let op = sample_operation(&mut clock, vec![], &blob);

        let pack = OperationPack {
            entity_id: entity,
            clock_snapshot: clock.snapshot(),
            operations: vec![op],
            content_blobs: vec![],
        };

        assert!(pack.validate().is_err());
    }
}
