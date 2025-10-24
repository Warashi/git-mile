# Git-backed Entity DAG Persistence

This document describes how the `git-mile` workspace persists entity graphs to Git, how Lamport clocks are maintained, and the tooling that interacts with the store.

## High-level Architecture

- The `git_mile_core` crate exposes `EntityStore`, a façade for reading and writing operation packs backed by a Git repository (local or bare).
- The `core::mile` module composes the raw entity primitives into `MileStore`, adding typed snapshots, event semantics (`created`, `status_changed`), and status transitions that power the CLI.
- The DAG of operations for each entity is encoded inside Git commits under the reference namespace `refs/git-mile/entities/<entity_id>`.
- Lamport clocks provide a total order over operations while preserving replica identifiers for tie-breaking.

## Git Object Layout

Each entity ID corresponds to a Git reference. The tip of the reference always points at a commit whose tree uses the following layout:

```
clock.json              # Serialized LamportTimestamp representing the latest logical clock.
index.json              # JSON record of current heads.
blobs/
  <sha256>.blob         # Raw operation payloads (one blob per digest).
pack/
  <counter>-<replica>-<suffix>/
    id                  # Text form of the OperationId (counter@replica).
    meta.json           # OperationMetadata (author, message, etc).
    parents             # One parent OperationId per line (can be empty).
    payload             # Blob digest referenced by this operation.
```

Key points:

- Blob digests are SHA-256 hashes of payload bytes. Deduplication happens automatically because `OperationBlob::from_bytes` computes the digest and `EntityStore` only writes missing blobs.
- Directory names under `pack/` are derived from the Lamport counter, a sanitized replica identifier, and a hash suffix. The canonical OperationId is still read from the `id` file.
- `index.json` currently contains a single field `heads`, storing the list of operation IDs that are considered heads. When the file is missing the loader recomputes the head set from the DAG.

## Lamport Clock Semantics

- `ReplicaId` is a simple wrapper around a string (often a host name or CLI identifier).
- `LamportClock::tick` increments the local counter while guarding against overflow. `LamportClock::merge` advances the local counter when encountering external timestamps.
- `LamportTimestamp` implements total ordering via `(counter, replica_id)` which feeds directly into `OperationId` ordering and filesystem layout.

## Persisting Operation Packs

`EntityStore::persist_pack` executes the following steps:

1. Load (or initialize) the existing entity snapshot from Git.
2. Validate and insert any blob payloads contained in the pack.
3. Insert each `Operation` in topological order, ensuring all referenced parents exist either in the current pack or prior history.
4. Update the head set: new operations become heads while any referenced parents are removed.
5. Merge Lamport clock snapshots, keeping the maximum timestamp.
6. Write a new commit for the entity reference with the updated tree structure.

Trying to insert duplicate operation IDs or referencing unknown parents returns a validation error (`Error::validation`).

## Mile Event Schema

`MileStore` stores all state transitions as JSON payloads inside operation blobs. Each payload follows a tagged envelope:

```json
{
  "version": 1,
  "type": "created",
  "data": {
    "title": "Ship onboarding flow",
    "description": "Track onboarding improvements",
    "status": "open"
  }
}
```

Supported event variants:

- `created` — emitted once per mile, capturing the title, optional description, and initial status (`draft`, `open`, or `closed`).
- `status_changed` — records a transition to a new status (`data.status`), preserving the Lamport timestamp and metadata author/message.

Unknown event types are surfaced as `MileEventKind::Unknown` during snapshot reconstruction; CLI consumers include them in history listings while skipping state transitions so that future schema changes degrade gracefully.

### Conflict Resolution

`EntityStore::resolve_conflicts` provides a small set of head-selection strategies:

- `MergeStrategy::Ours` keeps the lexicographically greatest OperationId.
- `MergeStrategy::Theirs` keeps the smallest OperationId.
- `MergeStrategy::Manual` accepts an explicit list of heads that must be a subset of the current head set.

The current implementation simply updates the stored head set; producing a merge operation is planned for future work.

## CLI Integration

The `git-mile` binary now layers mile-friendly verbs on top of `MileStore`:

```bash
# Initialise or reuse a repository
git-mile init --repo path/to/repo

# Manage miles end-to-end
git-mile create "Ship onboarding flow" --description "Track onboarding improvements"
git-mile list --format table
git-mile show <MILE_ID> --json
git-mile open <MILE_ID>
git-mile close <MILE_ID> --message "Reached GA quality"

# Low-level DAG helpers remain available for debugging
git-mile entity-debug list
git-mile entity-debug show <ENTITY_ID>
git-mile entity-debug resolve <ENTITY_ID> --strategy manual --head <OP_ID>
```

- `create` resolves author/email from Git config (overridable via `--author` / `--email`) and accepts per-command messages that flow into `OperationMetadata`.
- `list` filters closed miles unless `--all` is specified and supports `--format table|json|raw`.
- `show` renders either a human-friendly description or the JSON snapshot emitted by `MileStore`.
- `open` / `close` record status transitions via `change_status`, returning idempotent warnings when the desired state already matches.
- `entity-debug` mirrors the previous `entity` namespace for advanced inspection and conflict resolution.

## Testing Strategy

- `git_mile_core` contains unit tests for Lamport clocks, DAG validation, Git round-trips, and conflict resolution semantics using bare repositories.
- The CLI crate adds tests that exercise both the new mile verbs and the legacy entity helpers against temporary repositories to ensure end-to-end behaviour.

These tests run via `cargo test-all`, which is also wired into CI.
