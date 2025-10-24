# Identity Lifecycle

`git-mile` models contributor identities as event-sourced entities, mirroring the persistence model used for miles. Each identity is anchored in the Git repository under `refs/git-mile/entities/<id>` and progresses through a small state machine.

## States

- `pending_adoption` — the initial state after `git-mile create identity`. Metadata (display name, email, optional login) is known but no replica has adopted the identity yet.
- `adopted` — a replica records a canonical signature string via `git-mile adopt identity`. Subsequent adoption attempts yield validation errors until conflicts are resolved manually.
- `protected` — at least one protection (e.g. PGP fingerprint) has been registered through `git-mile protect identity`. Duplicate protections are ignored, keeping the operation history idempotent.

Whenever a snapshot cannot be decoded (for example due to unknown event variants), it is skipped by both identity and mile listings so that mixed histories remain resilient.

## Commands

```bash
# Create a new identity (optionally adopt immediately)
git-mile create identity \
  --display-name "Alice Example" \
  --email "alice@example.com" \
  --login alice \
  --adopt

# List known identities in a repository
git-mile list identity --format table

# Adopt an identity for the current replica
git-mile adopt identity <IDENTITY_ID> --signature "Alice Example <alice@example.com>"

# Add a PGP protection (repeat --pgp-fingerprint to register multiple keys)
git-mile protect identity <IDENTITY_ID> --pgp-fingerprint ABC12345
```

All identity operations accept the global `--repo`, `--replica`, `--author`, and `--email` overrides just like the mile subcommands. Author metadata recorded in `OperationMetadata` defaults to the adopted identity signature when present, falling back to Git configuration otherwise.

## Protection Metadata

`protect identity` currently supports PGP fingerprints. Each entry stores the fingerprint plus an optional ASCII-armoured public key (when provided via `--armored-key`). Duplicate registrations (same kind and fingerprint) return success with `changed = false`, allowing CLI invocations to remain idempotent.

## Replica Resolution

When mile commands (or other identity-aware features) need an author signature, `git-mile` resolves values in the following order:

1. CLI overrides (`--author`, `--email`).
2. An identity adopted by the current replica via `IdentityStore::find_adopted_by_replica`.
3. Repository-local Git config (`user.name`, `user.email`).
4. Global Git config.
5. Fallback value `git-mile <git-mile@example.com>`.

This ensures repositories with adopted identities automatically use consistent author metadata without sacrificing compatibility for existing flows.
