use std::cmp::Ordering;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct ReplicaId(String);

impl ReplicaId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReplicaId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<ReplicaId> for String {
    fn from(value: ReplicaId) -> Self {
        value.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct LamportTimestamp {
    counter: u64,
    replica_id: ReplicaId,
}

impl LamportTimestamp {
    pub fn new(counter: u64, replica_id: ReplicaId) -> Self {
        Self {
            counter,
            replica_id,
        }
    }

    pub fn counter(&self) -> u64 {
        self.counter
    }

    pub fn replica_id(&self) -> &ReplicaId {
        &self.replica_id
    }
}

impl Ord for LamportTimestamp {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.counter.cmp(&other.counter) {
            Ordering::Equal => self.replica_id.cmp(&other.replica_id),
            order => order,
        }
    }
}

impl PartialOrd for LamportTimestamp {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for LamportTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.counter, self.replica_id)
    }
}

#[derive(Clone, Debug)]
pub struct LamportClock {
    counter: u64,
    replica_id: ReplicaId,
}

impl LamportClock {
    pub fn new(replica_id: ReplicaId) -> Self {
        Self {
            counter: 0,
            replica_id,
        }
    }

    pub fn with_state(replica_id: ReplicaId, counter: u64) -> Self {
        Self {
            counter,
            replica_id,
        }
    }

    pub fn from_snapshot(snapshot: LamportTimestamp) -> Self {
        Self {
            counter: snapshot.counter(),
            replica_id: snapshot.replica_id().clone(),
        }
    }

    pub fn counter(&self) -> u64 {
        self.counter
    }

    pub fn replica_id(&self) -> &ReplicaId {
        &self.replica_id
    }

    pub fn snapshot(&self) -> LamportTimestamp {
        LamportTimestamp::new(self.counter, self.replica_id.clone())
    }

    /// Advance the clock and return the new timestamp.
    ///
    /// # Errors
    ///
    /// Returns an error when the counter would overflow the maximum representable value.
    pub fn tick(&mut self) -> Result<LamportTimestamp> {
        self.counter = self.counter.checked_add(1).ok_or(Error::ClockOverflow)?;
        Ok(self.snapshot())
    }

    pub fn merge(&mut self, remote: &LamportTimestamp) -> LamportTimestamp {
        if remote.counter > self.counter {
            self.counter = remote.counter;
        }

        self.snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tick_advances_clock_and_retains_replica() {
        let replica = ReplicaId::new("replica-a");
        let mut clock = LamportClock::new(replica.clone());

        let ts1 = clock.tick().expect("tick should succeed");
        let ts2 = clock.tick().expect("tick should succeed");

        assert_eq!(ts1.replica_id(), &replica);
        assert_eq!(ts2.replica_id(), &replica);
        assert!(ts1 < ts2);
        assert_eq!(ts1.counter(), 1);
        assert_eq!(ts2.counter(), 2);
    }

    #[test]
    fn merge_advances_to_remote_counter_when_greater() {
        let replica = ReplicaId::new("replica-a");
        let mut clock = LamportClock::with_state(replica.clone(), 2);

        let remote_timestamp = LamportTimestamp::new(5, ReplicaId::new("other"));

        let merged = clock.merge(&remote_timestamp);

        assert_eq!(merged.counter(), 5);
        assert_eq!(clock.counter(), 5);
        assert_eq!(merged.replica_id(), &replica);
    }

    #[test]
    fn merge_does_not_rewind_clock() {
        let replica = ReplicaId::new("replica-a");
        let mut clock = LamportClock::with_state(replica.clone(), 10);

        let remote_timestamp = LamportTimestamp::new(7, ReplicaId::new("other"));

        let merged = clock.merge(&remote_timestamp);

        assert_eq!(merged.counter(), 10);
        assert_eq!(clock.counter(), 10);
    }

    #[test]
    fn timestamps_are_totally_ordered_using_replica_id_as_tie_breaker() {
        let ts_a = LamportTimestamp::new(5, ReplicaId::new("a"));
        let ts_b = LamportTimestamp::new(5, ReplicaId::new("b"));
        let later_ts = LamportTimestamp::new(6, ReplicaId::new("a"));

        assert!(ts_a < later_ts);
        assert!(ts_a < ts_b);
        assert!(ts_b > ts_a);
    }
}
