# Repository Guidelines

## Project Structure & Module Organization
The workspace is orchestrated from the root `Cargo.toml` and split into three crates under `crates/`. `git-mile` houses the CLI entrypoint, `git-mile-core` provides the task and event domain logic, and `git-mile-store-git` implements Git-backed persistence. Shared configuration files such as `rustfmt.toml`, `clippy.toml`, and `deny.toml` live at the repository root so formatting, linting, and dependency policies stay consistent across the workspace.

## Build, Test, and Development Commands
Use `cargo fmt` to apply the repository formatting rules before every commit. `cargo clippy --workspace --all-targets --all-features` enforces lint expectations across every crate, including binaries, libraries, and examples. Run `cargo test --workspace --all-features` to execute unit and integration tests and verify cross-crate behavior. For local experimentation, `cargo run --package git-mile -- --help` prints the CLI usage summary without mutating the store.

## Coding Style & Naming Conventions
All Rust sources follow the formatting captured in `rustfmt.toml`; rely on `cargo fmt` rather than manual adjustments. Prefer idiomatic Rust naming: `snake_case` for functions, methods, and modules; `CamelCase` for types and traits; and `SCREAMING_SNAKE_CASE` for constants. When introducing new modules, mirror the existing folder layout inside each crate so domain logic remains in `git-mile-core`, persistence details stay in `git-mile-store-git`, and CLI wiring is confined to `git-mile`.

## Testing Guidelines
Unit tests typically reside beside the modules they cover using Rust’s inline `#[cfg(test)]` pattern, while integration tests belong under each crate’s `tests/` directory when cross-module coverage is needed. Aim to exercise new logic with deterministic scenarios and validate CRDT merge edge cases. Always run `cargo test --workspace --all-features` before pushing to ensure the task tracker flows still converge. If a change relies on Git fixtures, isolate them under `tests/fixtures/` and document any required repository state in the test module header.

## Commit & Pull Request Guidelines
Follow the conventional prefix style already present in history (`feat:`, `build:`, `ci:`, etc.) so commit intent is obvious. Commits should be tightly scoped: format, lint, and test before committing, then add only the relevant hunks with `git add -p`. Pull requests must summarize the problem, outline the solution, and link related issues. When UI- or behavior-facing changes occur in the CLI, include example command output or updated task snapshots to illustrate the result.

## Environment & Tooling Tips
The pinned Rust toolchain in `rust-toolchain.toml` and the optional Nix flake (`flake.nix`) keep contributors on compatible compilers. Use `cargo deny` with the provided `deny.toml` when auditing dependencies, and prefer UUIDv7 utilities that match the existing event identifiers to avoid merge conflicts in the Git-backed store.
