use git_mile_core::{LockMode, RepositoryLock};
use git2::Repository;
use std::io::ErrorKind;

#[test]
fn write_lock_prevents_second_writer() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let repo = Repository::init_bare(temp.path()).expect("init repo");
    let path = repo.path().to_path_buf();

    let _first =
        RepositoryLock::acquire(path.as_path(), LockMode::Write).expect("acquire first write lock");
    let err = match RepositoryLock::try_acquire(path.as_path(), LockMode::Write) {
        Ok(guard) => {
            drop(guard);
            panic!("second writer should be blocked");
        }
        Err(err) => err,
    };
    assert_eq!(err.kind(), ErrorKind::WouldBlock);
}

#[test]
fn read_locks_can_coexist() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let repo = Repository::init_bare(temp.path()).expect("init repo");
    let path = repo.path().to_path_buf();

    let _reader =
        RepositoryLock::acquire(path.as_path(), LockMode::Read).expect("acquire read lock");
    let _second_reader = RepositoryLock::try_acquire(path.as_path(), LockMode::Read)
        .expect("second read lock should succeed");
}

#[test]
fn read_lock_blocks_writer() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let repo = Repository::init_bare(temp.path()).expect("init repo");
    let path = repo.path().to_path_buf();

    let _reader =
        RepositoryLock::acquire(path.as_path(), LockMode::Read).expect("acquire read lock");
    let err = match RepositoryLock::try_acquire(path.as_path(), LockMode::Write) {
        Ok(guard) => {
            drop(guard);
            panic!("writer should be blocked while read lock held");
        }
        Err(err) => err,
    };
    assert_eq!(err.kind(), ErrorKind::WouldBlock);
}
