# git-mile

Rust workspace for experimenting with Git powered workflows.

## Workspace Layout
- `core`: shared library APIs that back the CLI experience.
- `cli`: binary crate that exposes the library functionality to end users.

## Documentation
- [Persistence design](docs/persistence.md)
- [Identity lifecycle](docs/identity.md)

## CLI Usage
The `git-mile` binary exposes mile and identity workflows alongside debugging utilities:

```bash
# bootstrap a repository (creates one if needed)
git-mile init --repo path/to/repo

# create a mile with optional metadata overrides
git-mile create mile "Ship onboarding flow" --description "Track onboarding improvements"

# inspect the current state of miles
git-mile list --format table
git-mile show <MILE_ID> --json

# record state transitions
git-mile open <MILE_ID>
git-mile close <MILE_ID> --message "Reached GA quality"

# identity lifecycle management
git-mile create identity --display-name "Alice" --email "alice@example.com" --adopt
git-mile identity list --format table
git-mile identity adopt <IDENTITY_ID> --signature "Alice <alice@example.com>"
git-mile identity protect <IDENTITY_ID> --pgp-fingerprint ABC12345

# legacy DAG commands remain available for debugging
git-mile entity-debug list
```

Global flags `--repo`, `--replica`, `--author`, and `--email` apply to every command; when omitted the CLI resolves values from the ambient Git configuration and host environment. Mile operations also have namespaced aliases (`git-mile mile create`, `git-mile mile list`) for parity with the new identity namespace.

## Tooling
- `cargo fmt-all` / `cargo fmt --all --check` to enforce Rustfmt with edition 2024 settings.
- `cargo lint` wraps Clippy across the entire workspace with warnings treated as errors.
- `cargo test-all` runs tests for every crate; this powers CI as well.
- `just ci` runs the full formatting, lint, and test pipeline locally (requires `just`, provided by the dev shell).

## Continuous Integration
GitHub Actions execute formatting, linting, and testing on every push and pull request via `.github/workflows/ci.yml`, mirroring the local commands.
