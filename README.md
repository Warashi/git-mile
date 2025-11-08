# git-mile

**git-mile** is a Git-backed task tracker that stores task events as commits under `refs/git-mile/tasks/*`. It provides a conflict-free, offline-first approach to task management using event sourcing and CRDTs (Conflict-free Replicated Data Types).

## Key Features

- **Git-native storage**: Tasks stored as immutable events in Git commits, never touching your working tree
- **Offline-first**: Work independently and merge task changes automatically using CRDTs
- **Event sourcing**: All changes represented as append-only events with UUIDv7 identifiers
- **Terminal UI**: Interactive multi-panel interface for browsing and editing tasks
- **MCP integration**: Model Context Protocol server for AI/Claude integration
- **Rich task model**: Titles, states, labels, assignees, descriptions, comments, and hierarchical relationships

## Architecture

The workspace consists of three crates:

- **`git-mile-core`**: Domain logic for task IDs, events, snapshots, and CRDT operations
- **`git-mile-store-git`**: Git repository persistence layer
- **`git-mile`**: CLI entry point with commands, TUI, and MCP server

## Data Model

**git-mile** uses CRDTs from the [`crdts`](https://docs.rs/crdts/) crate to ensure conflict-free merging:

- **Sets** (labels, assignees, relations): Represented as ORSWOT (Observed-Remove Set Without Tombstones), allowing concurrent additions and removals to merge naturally
- **Single values** (title, state, description): Stored as LWW (Last-Write-Wins) registers, converging based on event timestamps and UUIDv7 total ordering
- **Snapshots**: Materialized views of CRDT state, computed via `TaskSnapshot::replay` or `TaskSnapshot::apply` for consistent reads
- **State kinds**: Workflow states are classified as `todo`/`in_progress`/`blocked`/`done`/`backlog`, and the resolved kind is embedded into every `TaskCreated` and `TaskStateSet` event so downstream clients never need to infer it again (see `docs/state-kind-persistence.md` for schema and migration details)

## Installation

```bash
cargo install --path crates/git-mile
```

Or build from source:

```bash
cargo build --release --package git-mile
```

## Quick Start

```bash
# Create a new task
git-mile new "Implement feature X" --state todo --labels feature,priority:high

# Launch the interactive TUI
git-mile tui

# List all tasks
git-mile ls

# Show task details as JSON
git-mile show <task-id>

# Add a comment
git-mile comment <task-id> "Working on this now"

# Start MCP server for AI integration
git-mile mcp
```

## Commands Reference

### `new` - Create a Task

Create a new task with optional metadata:

```bash
git-mile new "Task title" \
  --state todo \
  --labels backend,api \
  --assignees alice,bob \
  --description "Detailed description" \
  --parent <parent-task-id>
```

**Options**:
- `--state`: Initial state (e.g., todo, in_progress, done)
- `--labels`: Comma-separated labels
- `--assignees`: Comma-separated assignee names
- `--description`: Long-form task description
- `--parent`: Link to parent task for hierarchical organization
- `--actor-name`, `--actor-email`: Override default actor info

### `comment` - Add a Comment

Add a comment to an existing task:

```bash
git-mile comment <task-id> "Comment body in markdown"
```

### `show` - Display Task Snapshot

Output the current state of a task as JSON:

```bash
git-mile show <task-id>
```

### `ls` - List Tasks

Filter and display tasks using snapshot data. The default output is a compact table:

```bash
git-mile ls
```

Apply filters and switch to JSON output when integrating with other tools:

```bash
git-mile ls \
  --state state/in_progress \
  --label priority/high \
  --assignee alice \
  --text "refactor parser" \
  --format json
```

**Filter options**:

- `--state, -s <value>`: Match tasks whose workflow state equals the provided value. Repeat to match multiple states.
- `--label, -l <value>`: Require tasks to include the given label. Repeat to require multiple labels (logical AND).
- `--assignee, -a <value>`: Match tasks assigned to any of the provided actors.
- `--text <substring>`: Case-insensitive substring search across title, description, state, labels, and assignees.

**Format options**:

- `--format table` (default): Prints a human-readable table showing ID, state, title, labels, assignees, and last update timestamp.
- `--format json`: Emits an array of serialized `TaskSnapshot` objects for downstream scripting.

### `tui` - Interactive Terminal UI

Launch the full-featured terminal interface:

```bash
git-mile tui
```

**TUI Controls**:
- `j`/`k` or `↓`/`↑`: Navigate task list
- `Enter`: Open hierarchical tree view
- `e`: Edit current task
- `n`: Create new task
- `s`: Create subtask of current task
- `c`: Add comment to current task
- `r`: Refresh view
- `p`: Jump to parent task
- `q`: Quit

**TUI Layout**:
- **Left panel**: Task list sorted by update time
- **Top-right panel**: Task details with breadcrumb navigation to parents
- **Middle-right panel**: Subtasks list
- **Bottom-right panel**: Comments history

### `mcp` - Model Context Protocol Server

Start an MCP server exposing task operations to AI tools:

```bash
git-mile mcp
```

**Available MCP Tools**:
- `list_tasks`: Retrieve tasks (optionally filtered by `states`, `labels`, `assignees`, `text`)
- `get_task`: Fetch a single task snapshot by ID
- `create_task`: Create new task with metadata
- `update_task`: Modify task properties
- `add_comment`: Add comment to task
- `update_comment`: Edit existing comment
- `list_workflow_states`: Return allowed workflow states plus the current default

`get_task` accepts a JSON payload like `{"task_id": "<UUIDv7>"}` and returns the serialized `TaskSnapshot` for that task, matching the data shown in the CLI/TUI views. Every snapshot now includes a `state_kind` field next to `state`; legacy tasks created before this release will show `null` until they are backfilled (see `docs/state-kind-persistence.md`).

## Configuration

**Actor information** (name and email for events) is resolved in this order:
1. Command-line flags: `--actor-name`, `--actor-email`
2. Environment variables: `GIT_MILE_ACTOR_NAME`, `GIT_MILE_ACTOR_EMAIL`
3. Git author variables: `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL`
4. Git config: `user.name`, `user.email`
5. Defaults: `"git-mile"`, `"git-mile@localhost"`

**Editor** (for TUI edit operations) is resolved from:
1. `GIT_MILE_EDITOR`
2. `VISUAL`
3. `EDITOR`
4. Default: `vi`

**Repository location**:
- Use `--repo <path>` to specify a Git repository outside the current directory

**Workflow states** (optional):
- Define `.git-mile/config.toml` in the repository root to restrict valid states per project
- Add `kind` (`todo`, `in_progress`, `blocked`, `done`, `backlog`) to classify each state so CLI/TUI can render kind markers and enable kind filters
- The resolved `kind` is embedded into each `TaskCreated`/`TaskStateSet` event so downstream clients never need to guess it later
- TUI/CLI/MCP will validate `state` values and show hints when this file lists allowed entries
- Set `default_state` to automatically apply a state when new tasks omit it
- When the file is missing, git-mile falls back to a built-in workflow (`state/todo`, `state/in-progress`, `state/done`). Set `states = []` if you prefer an unrestricted setup instead.

```toml
[workflow]
states = [
  { value = "state/todo", label = "Todo", kind = "todo" },
  { value = "state/in-progress", label = "In Progress", kind = "in_progress" },
  { value = "state/done", label = "Done", kind = "done" }
]
default_state = "state/todo"
```

## Development

### Build and Test

```bash
# Format code
cargo fmt

# Run linter
cargo clippy --workspace --all-targets --all-features

# Run tests
cargo test --workspace --all-features
```

### Commit Guidelines

Follow conventional commit prefixes (`feat:`, `fix:`, `build:`, `ci:`, etc.) and ensure all changes are formatted, linted, and tested before committing:

```bash
cargo fmt && cargo clippy --workspace --all-targets --all-features && cargo test --workspace --all-features
git add -p
git commit
```

## How It Works

1. **Event Storage**: Each task is identified by a UUIDv7. Events are stored as JSON in commit messages under `refs/git-mile/tasks/<task-id>`
2. **Event Types**: TaskCreated, StateSet, TitleSet, LabelsAdded/Removed, AssigneesAdded/Removed, CommentAdded, ChildLinked/Unlinked, etc.
3. **Snapshot Computation**: The current state of a task is computed by replaying all events through CRDT operations
4. **Conflict Resolution**: Concurrent edits merge automatically—sets use ORSWOT logic, single values use LWW with UUIDv7 tie-breaking
5. **Git Integration**: Standard Git operations (fetch, merge, push) propagate task changes across repositories

## License

MIT

## Contributing

Contributions are welcome! Please ensure:
- Code follows `rustfmt.toml` formatting rules
- All clippy lints pass with `--all-features`
- Tests pass with `cargo test --workspace --all-features`
- Commits follow conventional commit style
- Changes preserve CRDT convergence guarantees

## Acknowledgments

Built with:
- [`crdts`](https://docs.rs/crdts/) for conflict-free data structures
- [`ratatui`](https://ratatui.rs/) for terminal UI
- [`git2`](https://docs.rs/git2/) for Git operations
- [`rmcp`](https://docs.rs/rmcp/) for Model Context Protocol
