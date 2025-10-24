# git-mile

Rust workspace for experimenting with Git powered workflows.

## Workspace Layout
- `core`: shared library APIs that back the CLI experience.
- `cli`: binary crate that exposes the library functionality to end users.

## Documentation
- [Persistence design](docs/persistence.md)

## Tooling
- `cargo fmt-all` / `cargo fmt --all --check` to enforce Rustfmt with edition 2024 settings.
- `cargo lint` wraps Clippy across the entire workspace with warnings treated as errors.
- `cargo test-all` runs tests for every crate; this powers CI as well.
- `just ci` runs the full formatting, lint, and test pipeline locally (requires `just`, provided by the dev shell).

## Continuous Integration
GitHub Actions execute formatting, linting, and testing on every push and pull request via `.github/workflows/ci.yml`, mirroring the local commands.
