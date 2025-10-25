# git-mile

Rust workspace for experimenting with Git powered workflows.

## Workspace Layout
- `core`: shared library APIs that back the CLI experience.
- `cli`: binary crate that exposes the library functionality to end users.

## Documentation
- [CLI command reference](docs/reference/cli.md)
- [Lifecycle guide](docs/guides/lifecycle.md)
- [Persistence design](docs/persistence.md)
- [Identity lifecycle](docs/identity.md)
- [Concurrency and locking](docs/concurrency.md)

## CLI Usage
The `git-mile` binary exposes mile and identity workflows alongside debugging utilities:

```bash
# bootstrap a repository (creates one if needed)
git-mile init --repo path/to/repo

# create a milestone with description, initial comment, and labels
git-mile create milestone "Ship onboarding flow" \
  --description "Track onboarding improvements" \
  --comment "Kickoff: align on success criteria" \
  --label roadmap --label onboarding

# inspect enriched milestone data
git-mile list milestone --long --columns id,title,status,labels,comments
git-mile show <MILE_ID> --limit-comments 10

# collaborate with comments and labels
git-mile comment milestone <MILE_ID> --comment "ETA moved forward"
git-mile label issue <ISSUE_ID> --add ready-for-review --remove backlog

# record state transitions
git-mile open <MILE_ID> --message "Begin execution"
git-mile close <MILE_ID> --message "Reached GA quality"

# identity lifecycle management
git-mile create identity --display-name "Alice" --email "alice@example.com" --adopt
git-mile list identity --format table
git-mile adopt identity <IDENTITY_ID> --signature "Alice <alice@example.com>"
git-mile protect identity <IDENTITY_ID> --pgp-fingerprint ABC12345

# legacy DAG commands remain available for debugging
git-mile entity-debug list
```

Global flags `--repo`, `--replica`, `--author`, and `--email` apply to every command; when omitted the CLI resolves values from the ambient Git configuration and host environment.

Concurrent invocations coordinate via a repository lock: read-only commands
share a read lock, while commands that mutate repository state wait for an
exclusive write lock. See [Concurrency and locking](docs/concurrency.md) for
details.

## Tooling
- `cargo fmt-all` / `cargo fmt --all --check` to enforce Rustfmt with edition 2024 settings.
- `cargo lint` wraps Clippy across the entire workspace with warnings treated as errors.
- `cargo test-all` runs tests for every crate; this powers CI as well.
- `just ci` runs the full formatting, lint, and test pipeline locally (requires `just`, provided by the dev shell).

## Continuous Integration
GitHub Actions execute formatting, linting, and testing on every push and pull request via `.github/workflows/ci.yml`, mirroring the local commands.
