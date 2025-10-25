use git_mile_core::{CreateMileInput, LockMode, MileStatus, MileStore, ReplicaId};
use git2::Repository;
use std::sync::{Arc, Barrier, Mutex};
use std::thread;

#[test]
fn concurrent_mile_creations_are_serialized() {
    let temp = tempfile::tempdir().expect("create temp dir");
    Repository::init_bare(temp.path()).expect("init repo");
    let repo_path = temp.path().to_path_buf();

    let barrier = Arc::new(Barrier::new(2));
    let results = Arc::new(Mutex::new(Vec::new()));

    let spawn_worker = |index: usize| {
        let barrier = barrier.clone();
        let results = results.clone();
        let repo_path = repo_path.clone();
        thread::spawn(move || {
            barrier.wait();
            let store = MileStore::open_with_mode(repo_path.as_path(), LockMode::Write)
                .expect("open mile store");
            let replica = ReplicaId::new(format!("replica-{index}"));
            let snapshot = store
                .create_mile(CreateMileInput {
                    replica_id: replica,
                    author: format!("tester-{index}"),
                    message: Some(format!("create-{index}")),
                    title: format!("Concurrent Mile {index}"),
                    description: None,
                    initial_status: MileStatus::Open,
                    initial_comment: None,
                    labels: vec![],
                })
                .expect("create mile");
            results.lock().expect("lock results").push(snapshot.id);
        })
    };

    let left = spawn_worker(0);
    let right = spawn_worker(1);

    left.join().expect("left worker finished");
    right.join().expect("right worker finished");

    let store = MileStore::open_with_mode(repo_path.as_path(), LockMode::Read)
        .expect("open mile store for read");
    let miles = store.list_miles().expect("list miles");
    assert_eq!(miles.len(), 2);

    let created = results.lock().expect("lock results");
    for id in created.iter() {
        assert!(miles.iter().any(|mile| &mile.id == id));
    }
}
