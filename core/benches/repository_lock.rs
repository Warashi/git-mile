use criterion::{Criterion, criterion_group, criterion_main};
use git_mile_core::{LockMode, RepositoryLock};
use git2::Repository;

fn bench_repository_lock(c: &mut Criterion) {
    let temp = tempfile::tempdir().expect("create temp dir");
    let repo = Repository::init_bare(temp.path()).expect("init repo");
    let path = repo.path().to_path_buf();

    c.bench_function("lock_write_cycle", |b| {
        b.iter(|| {
            let guard = RepositoryLock::acquire(path.as_path(), LockMode::Write)
                .expect("acquire write lock");
            drop(guard);
        });
    });

    c.bench_function("lock_read_cycle", |b| {
        b.iter(|| {
            let guard =
                RepositoryLock::acquire(path.as_path(), LockMode::Read).expect("acquire read lock");
            drop(guard);
        });
    });
}

criterion_group!(benches, bench_repository_lock);
criterion_main!(benches);
