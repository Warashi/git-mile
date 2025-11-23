# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Repository Guidelines

### Project Structure & Module Organization
The workspace is orchestrated from the root `Cargo.toml` and split into five crates under `crates/`:
- **`git-mile-core`**: Domain logic for task identifiers, event types, CRDT snapshot computation, and filtering—no I/O
- **`git-mile-store-git`**: Git-backed persistence layer wrapping `git2`, providing event storage and retrieval
- **`git-mile-hooks`**: Hook system for executing custom scripts before/after task operations
- **`git-mile-app`**: Application layer integrating core, store, and hooks with TaskWriter/TaskReader
- **`git-mile`**: CLI entrypoint with commands, TUI (ratatui), and MCP server

Shared configuration files such as `rustfmt.toml`, `clippy.toml`, and `deny.toml` live at the repository root so formatting, linting, and dependency policies stay consistent across the workspace.

### Build, Test, and Development Commands
Use `cargo fmt` to apply the repository formatting rules before every commit. `cargo clippy --workspace --all-targets --all-features` enforces lint expectations across every crate, including binaries, libraries, and examples. Run `cargo test --workspace --all-features` to execute unit and integration tests and verify cross-crate behavior. For local experimentation, `cargo run --package git-mile -- --help` prints the CLI usage summary without mutating the store.

To run a single test by name:
```bash
cargo test --package git-mile-core test_name
```

To build a release binary:
```bash
cargo build --release --package git-mile
```

Install locally from source:
```bash
cargo install --path crates/git-mile
```

### Coding Style & Naming Conventions
All Rust sources follow the formatting captured in `rustfmt.toml`; rely on `cargo fmt` rather than manual adjustments. Prefer idiomatic Rust naming: `snake_case` for functions, methods, and modules; `CamelCase` for types and traits; and `SCREAMING_SNAKE_CASE` for constants. When introducing new modules, mirror the existing folder layout inside each crate so domain logic remains in `git-mile-core`, persistence details stay in `git-mile-store-git`, and CLI wiring is confined to `git-mile`.

### Testing Guidelines
Unit tests typically reside beside the modules they cover using Rust's inline `#[cfg(test)]` pattern, while integration tests belong under each crate's `tests/` directory when cross-module coverage is needed. Aim to exercise new logic with deterministic scenarios and validate CRDT merge edge cases. Always run `cargo test --workspace --all-features` before pushing to ensure the task tracker flows still converge. If a change relies on Git fixtures, isolate them under `tests/fixtures/` and document any required repository state in the test module header.

### Commit & Pull Request Guidelines
Follow the conventional prefix style already present in history (`feat:`, `build:`, `ci:`, etc.) so commit intent is obvious. Commits should be tightly scoped: format, lint, and test before committing, then add only the relevant hunks with `git add -p`. Pull requests must summarize the problem, outline the solution, and link related issues. When UI- or behavior-facing changes occur in the CLI, include example command output or updated task snapshots to illustrate the result.

### Environment & Tooling Tips
The pinned Rust toolchain in `rust-toolchain.toml` and the optional Nix flake (`flake.nix`) keep contributors on compatible compilers. Use `cargo deny` with the provided `deny.toml` when auditing dependencies, and prefer UUIDv7 utilities that match the existing event identifiers to avoid merge conflicts in the Git-backed store.

## Architecture Overview

### Event Sourcing + CRDT Design
git-mile is built on event sourcing combined with CRDTs (Conflict-free Replicated Data Types) to enable offline-first, distributed task tracking. All task changes are stored as immutable events with UUIDv7 identifiers that guarantee temporal ordering. Events are never modified—only appended—ensuring a complete audit trail.

The current state of any task is computed by replaying its event history through CRDT operations. This design allows concurrent edits from different repositories to merge automatically without conflicts:
- **Sets** (labels, assignees, parent/child links): Use ORSWOT (Observed-Remove Set Without Tombstones)
- **Single values** (title, state, description): Use LWW (Last-Write-Wins) registers with UUIDv7 tie-breaking

### Git-Backed Storage
Events are stored as Git commits under `refs/git-mile/tasks/<task-id>`. Each event becomes a commit with an empty tree (to avoid touching the working directory) and a JSON-encoded event in the commit message. The commit format is:
```
git-mile-event: <event-id>

<JSON event payload>
```

Parent commits form a chain representing the event history. Actor information is embedded in Git's author/committer fields. This approach lets standard Git operations (fetch, merge, push) synchronize tasks across repositories while preserving conflict-free CRDT properties.

### Crate Responsibilities

**git-mile-core**: Domain logic for task identifiers, event types, CRDT snapshot computation, and filtering. Contains no persistence or I/O logic—purely functional event replay and state materialization.

**git-mile-store-git**: Persistence layer wrapping `git2`. Provides `GitStore::append_event()` to commit new events and `GitStore::load_events()` to reconstruct event history. Includes an LRU cache (capacity 256) to optimize repeated event decoding.

**git-mile-hooks**: Hook execution system that runs custom scripts before/after task operations. Pre-hooks can reject operations by returning non-zero exit codes. Hooks receive event data as JSON on stdin and can optionally modify events by writing to stdout. Configuration lives in `.git-mile/config.toml` with options for enabling/disabling hooks, setting timeouts, and specifying which hooks to skip.

**git-mile-app**: Orchestration layer providing `TaskWriter` and `TaskReader` that integrate core, store, and hooks. TaskWriter handles event creation and hook execution, while TaskReader provides filtered snapshot views. This layer ensures hooks run at the correct lifecycle points and manages error handling/rollback.

**git-mile**: Application layer with three interfaces:
1. CLI commands (`new`, `comment`, `show`, `ls`) for scripting and automation
2. TUI (`tui` command) with ratatui-based interactive interface for browsing/editing tasks
3. MCP server (`mcp` command) exposing task operations via Model Context Protocol for AI integration

All three interfaces share the same `TaskFilter` logic and operate on identical `TaskSnapshot` views computed from core CRDT operations.

## Configuration

### Keybindings

TUI keybindings can be customized via `~/.config/git-mile/config.toml` (or `%APPDATA%\git-mile\config.toml` on Windows).

**Generate default configuration**:
```bash
git-mile config init-keybindings
```

**Configuration structure**:
- File format: TOML
- Location: `crates/git-mile/src/config/keybindings.rs`
- Default path: `~/.config/git-mile/config.toml` (XDG Base Directory specification)
- Sections: `task_list`, `tree_view`, `state_picker`, `comment_viewer`, `description_viewer`

**Key features**:
- Multiple keys per action: Each action can be bound to multiple keys (e.g., `quit = ["q", "Q", "Esc"]`)
- Modifier support: Ctrl, Alt, Shift modifiers (e.g., `scroll_down_fast = ["Ctrl+d"]`)
- Special keys: Enter, Esc, arrow keys, etc.
- Dynamic help text: TUI bottom bar generates help from configuration (displays first key in each list)
- Validation: Checks for key conflicts, empty bindings, and invalid key expressions

**Implementation details**:
- Configuration loaded at TUI startup via `load_keybindings_config()`
- Falls back to hardcoded defaults if config file doesn't exist
- Validation via `validate_config()` before use
- Key matching uses `KeyBindingsConfig::matches()` method
- Help text generation via `KeyBindingsConfig::generate_help_text()`

See [docs/keybindings.md](docs/keybindings.md) for user documentation.

### Key Design Decisions

**Snapshots are never persisted**: Task snapshots are always recomputed from events. This ensures consistency and avoids cache invalidation complexity.

**UUIDv7 for total ordering**: Events and tasks use UUIDv7 identifiers which embed timestamps, providing the total order required by CRDT LWW registers.

**Workflow state classification**: States have configurable `kind` values (`todo`, `in_progress`, `blocked`, `done`, `backlog`) embedded in events, allowing semantic filtering without inference.

**Zero unsafe code**: The workspace forbids `unsafe_code` and bans `unwrap_used`/`expect_used` to enforce exhaustive error handling via `Result` types.

### Configuration

**`.git-mile/config.toml`** (optional) configures workflow states and hooks:

```toml
[workflow]
# Define allowed states with semantic kinds for filtering/rendering
states = [
  { value = "state/todo", label = "Todo", kind = "todo" },
  { value = "state/in-progress", label = "In Progress", kind = "in_progress" },
  { value = "state/done", label = "Done", kind = "done" }
]
# Automatically apply this state when creating tasks without --state
default_state = "state/todo"

[hooks]
# Master switch for all hooks
enabled = true
# List specific hooks to skip even when enabled=true
disabled = []
# Maximum execution time in seconds before killing hook
timeout = 30
```

**Workflow state kinds** (`todo`, `in_progress`, `blocked`, `done`, `backlog`) are embedded into `TaskCreated` and `TaskStateSet` events so clients never need to infer them. The TUI/CLI validate state values against this config and show hints when states are misconfigured.

**Hook scripts** must be executable files in `.git-mile/hooks/` named after the hook type (e.g., `pre-task-create`). They receive JSON event data on stdin and can reject operations (pre-hooks) by exiting non-zero. See `docs/hooks.md` for detailed hook documentation and examples. Current implementation (Phase 1) only integrates `pre-task-create` and `post-task-create` hooks; other hooks are defined but not yet called.

## Common Workflows

### Running the TUI locally
```bash
cargo run --package git-mile -- tui
```

### Testing CRDT convergence
When changing event replay logic or CRDT operations, add tests that apply events in different orders and verify the snapshots converge to identical states.

### Adding new event types
1. Define the event variant in `git-mile-core/src/event.rs`
2. Implement replay logic in `git-mile-core/src/lib.rs` (`TaskSnapshot::apply`)
3. Add serialization tests to ensure round-trip JSON encoding
4. Update relevant command handlers in `git-mile/src/commands/mod.rs`

### Validating Git storage round-trips
Integration tests in `git-mile-store-git` verify that events committed via `append_event()` can be loaded via `load_events()` and produce identical deserialized structures.

### Working with hooks
When adding or modifying hook functionality:
1. Define hook types in `git-mile-hooks/src/lib.rs` using the `HookKind` enum
2. Implement hook execution logic in the hooks crate (runner, context, input/output)
3. Integrate hooks into `TaskWriter` in `git-mile-app/src/lib.rs` at the appropriate lifecycle points
4. Add integration tests in `git-mile-app/tests/` to verify hook execution and error handling
5. Update `docs/hooks.md` with examples and behavior documentation

Hook integration follows a phased approach. Phase 1 (completed) integrated task creation hooks. Phase 2 will add hooks for updates, comments, state changes, and parent/child links.
