use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};
use uuid::Uuid;

/// Identifier of a task (UUID v7).
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
pub struct TaskId(pub Uuid);

impl TaskId {
    #[must_use]
    /// Generate a fresh task identifier.
    pub fn new() -> Self {
        // UUID version 7 keeps the temporal ordering used for CRDT convergence.
        Self(Uuid::now_v7())
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for TaskId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl Serialize for TaskId {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for TaskId {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Identifier of an event (UUID v7).
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
pub struct EventId(pub Uuid);

impl EventId {
    #[must_use]
    /// Generate a fresh event identifier.
    pub fn new() -> Self {
        // UUID version 7 keeps the temporal ordering used for CRDT convergence.
        Self(Uuid::now_v7())
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for EventId {
    type Err = uuid::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Uuid::parse_str(s)?))
    }
}

impl Serialize for EventId {
    fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for EventId {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_id_uses_uuid_v7() {
        let id = TaskId::new();
        assert_eq!(id.0.get_version_num(), 7);
    }

    #[test]
    fn event_id_uses_uuid_v7() {
        let id = EventId::new();
        assert_eq!(id.0.get_version_num(), 7);
    }

    #[test]
    fn task_id_roundtrip() {
        let uuid = Uuid::now_v7();
        let parsed: TaskId = uuid.to_string().parse().expect("must parse task id");
        assert_eq!(parsed.0, uuid);
    }

    #[test]
    fn event_id_roundtrip() {
        let uuid = Uuid::now_v7();
        let parsed: EventId = uuid
            .to_string()
            .parse()
            .expect("must parse event id");
        assert_eq!(parsed.0, uuid);
    }
}
