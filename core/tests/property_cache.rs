#![cfg(feature = "property-tests")]

use std::time::Duration;

use git_mile_core::clock::{LamportClock, ReplicaId};
use git_mile_core::dag::{BlobRef, EntityId, EntitySnapshot, Operation, OperationBlob, OperationId, OperationMetadata};
use git_mile_core::repo::cache::CacheLoadOutcome;
use git_mile_core::repo::{CacheConfig, CacheNamespace, CacheRepository, PersistentCache};
use proptest::prelude::*;
use tempfile::tempdir;

#[derive(Clone, Debug)]
enum CacheOp {
    Put,
    Invalidate,
    Fetch,
}

fn operation_strategy() -> impl Strategy<Value = CacheOp> {
    prop_oneof![
        Just(CacheOp::Put),
        Just(CacheOp::Invalidate),
        Just(CacheOp::Fetch),
    ]
}

fn sample_snapshot(entity_id: &EntityId) -> EntitySnapshot {
    let replica = ReplicaId::new("property-cache");
    let mut clock = LamportClock::new(replica.clone());
    let ts = clock.tick().expect("tick clock");
    let op_id = OperationId::new(ts.clone());
    let op = Operation::new(
        op_id.clone(),
        vec![],
        BlobRef::from_bytes(b"payload"),
        OperationMetadata::new("tester", Some("init".to_string())),
    );

    EntitySnapshot {
        entity_id: entity_id.clone(),
        clock_snapshot: clock.snapshot(),
        heads: vec![op_id],
        operations: vec![op],
        blobs: vec![OperationBlob::from_bytes(b"payload".to_vec())],
    }
}

proptest! {
    #[test]
    fn persistent_cache_matches_naive_store(ops in prop::collection::vec(operation_strategy(), 1..32)) {
        let temp = tempdir().expect("tempdir");
        git2::Repository::init(temp.path()).expect("init repo");

        let mut config = CacheConfig::for_repo(temp.path()).expect("config");
        config.maintenance_interval = Duration::from_secs(0);

        let cache = PersistentCache::open(config).expect("open cache");
        let entity_id = EntityId::new();
        let snapshot = sample_snapshot(&entity_id);
        let mut naive: Option<EntitySnapshot> = None;

        for op in ops {
            match op {
                CacheOp::Put => {
                    cache.put(CacheNamespace::Issues, &entity_id, &snapshot).expect("put");
                    naive = Some(snapshot.clone());
                }
                CacheOp::Invalidate => {
                    cache.invalidate(CacheNamespace::Issues, &entity_id).expect("invalidate");
                    naive = None;
                }
                CacheOp::Fetch => {
                    match cache.get(CacheNamespace::Issues, &entity_id).expect("get") {
                        CacheLoadOutcome::Hit(value) => {
                            prop_assert!(naive.is_some());
                            prop_assert_eq!(value, naive.clone().expect("naive value"));
                        }
                        CacheLoadOutcome::Miss => {
                            prop_assert!(naive.is_none());
                        }
                        CacheLoadOutcome::Stale => {
                            // With maintenance disabled and long TTL, stale results should not appear.
                            prop_assert!(naive.is_none());
                        }
                    }
                }
            }
        }
    }
}
