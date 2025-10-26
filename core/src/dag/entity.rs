use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::clock::{LamportTimestamp, ReplicaId};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct EntityId(Uuid);

impl EntityId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }

    pub fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }

    pub fn as_uuid(&self) -> &Uuid {
        &self.0
    }
}

impl Default for EntityId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for EntityId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for EntityId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl From<EntityId> for Uuid {
    fn from(value: EntityId) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct OperationId {
    timestamp: LamportTimestamp,
}

impl OperationId {
    pub fn new(timestamp: LamportTimestamp) -> Self {
        Self { timestamp }
    }

    pub fn timestamp(&self) -> &LamportTimestamp {
        &self.timestamp
    }

    pub fn replica_id(&self) -> &ReplicaId {
        self.timestamp.replica_id()
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.timestamp.fmt(f)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ParseOperationIdError {
    #[error("operation id must contain '@': {0}")]
    MissingReplicaSeparator(String),
    #[error("operation id has invalid logical time '{value}': {source}")]
    InvalidCounter {
        value: String,
        #[source]
        source: std::num::ParseIntError,
    },
}

impl FromStr for OperationId {
    type Err = ParseOperationIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let trimmed = s.trim();
        let (counter_part, replica_part) = trimmed
            .split_once('@')
            .ok_or_else(|| ParseOperationIdError::MissingReplicaSeparator(trimmed.to_string()))?;

        let counter: u64 =
            counter_part
                .parse()
                .map_err(|source| ParseOperationIdError::InvalidCounter {
                    value: counter_part.to_string(),
                    source,
                })?;

        Ok(Self::new(LamportTimestamp::new(
            counter,
            ReplicaId::new(replica_part),
        )))
    }
}

impl PartialOrd for OperationId {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OperationId {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.timestamp.cmp(&other.timestamp)
    }
}

impl From<OperationId> for LamportTimestamp {
    fn from(value: OperationId) -> Self {
        value.timestamp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{LamportClock, ReplicaId};

    #[test]
    fn operation_id_orders_by_timestamp() {
        let replica = ReplicaId::new("replica");
        let mut clock = LamportClock::new(replica.clone());
        let ts1 = clock.tick().unwrap();
        let ts2 = clock.tick().unwrap();

        let id1 = OperationId::new(ts1);
        let id2 = OperationId::new(ts2);

        assert!(id1 < id2);
        assert_eq!(id1.replica_id(), &replica);
    }

    #[test]
    fn entity_id_roundtrip_string_form() {
        let entity = EntityId::new();
        let parsed = EntityId::from_str(&entity.to_string()).expect("should parse");
        assert_eq!(entity, parsed);
    }
}
