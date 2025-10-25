# Repository Concurrency Strategy

This document captures the locking primitives that protect the Git-backed
entity store and explains how callers should select the appropriate access
mode when interacting with the `git-mile` core crates.

## Locking Basics

- `RepositoryLock` acquires an advisory filesystem lock under
  `<repo>/.git/git-mile/lock` (or `repo/git-mile/lock` for bare
  repositories).
- Locks are process-wide: dropping the guard releases the lock automatically.
- Two `LockMode` variants are available:
  - `Read` — shared lock. Multiple readers may coexist, but any writer waits.
  - `Write` — exclusive lock. Guarantees serialized access to repository
    mutations.

The lock directory is created on demand. On network filesystems the
platform-provided semantics apply; document this risk when deploying.

## Store APIs

`EntityStore`, `MileStore`, and `IdentityStore` expose `open_with_mode` in
addition to the legacy `open` helper (which now defaults to `Write`). Use the
following rules of thumb:

- Read-only operations (`list_miles`, `load_mile`, `list_identities`,
  `find_adopted_by_replica`, `EntityStore::list_entities`, etc.) should open
  stores with `LockMode::Read`.
- Mutating operations (`create_mile`, `change_status`, `create_identity`,
  `adopt_identity`, `persist_pack`, conflict resolution) must request a
  `Write` lock.
- Never hold a write-locked store across a call that may open another
  write-locked store. Drop the guard (let the variable go out of scope) first
  to avoid deadlocks.

Each store keeps an `Arc<dyn RepositoryCacheHook>` that receives callbacks
after pack persistence or snapshot loads. The default implementation is a
no-op, but integrating caches only requires providing a new hook
implementation when opening the store.

## CLI Behaviour

The CLI now opens stores with explicit modes:

- Listing commands (`mile list`, `identity list`, `entity-debug list/show`)
  acquire `LockMode::Read`.
- Mutating commands (`mile create/open/close`, identity adoption and
  protections, `entity-debug resolve`) acquire `LockMode::Write`.
- `resolve_identity` opens the identity store with `LockMode::Read` and drops
  it before invoking any mile operations, ensuring nested commands do not
  deadlock.

## Testing and Benchmarks

- `core/tests/repository_lock.rs` covers lock semantics (shared reads,
  write exclusivity, read-blocks-write).
- `core/tests/concurrency.rs` spawns concurrent mile creations to verify that
  write locks serialize DAG mutations.
- `cargo bench -p git_mile_core repository_lock` benchmarks coarse lock
  acquisition overhead for both read and write cycles (powered by Criterion).

## Limitations

- File locks rely on OS advisory semantics; behaviour on certain network
  filesystems (notably NFS) may differ. Production deployments should prefer
  local disks or verify locking guarantees on their storage backend.
- Locks do not attempt to detect process crashes. Crash-safe recovery is
  provided by relying on the underlying OS to release file descriptors.
