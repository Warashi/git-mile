use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};
use ulid::Ulid;

/// Identifier of a task (ULID).
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
pub struct TaskId(pub Ulid);

impl TaskId {
    #[must_use]
    /// Generate a fresh task identifier.
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for TaskId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Ulid::from_string(s)?))
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

/// Identifier of an event (ULID).
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash, Debug, Default)]
pub struct EventId(pub Ulid);

impl EventId {
    #[must_use]
    /// Generate a fresh event identifier.
    pub fn new() -> Self {
        Self(Ulid::new())
    }
}

impl fmt::Display for EventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for EventId {
    type Err = ulid::DecodeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(Ulid::from_string(s)?))
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
