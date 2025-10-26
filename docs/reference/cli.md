# CLI Command Reference

This reference captures the primary commands exposed by `git-mile`, including
the new description, comment, and label flows introduced for the M2 milestone.
All commands accept the global flags `--repo`, `--replica`, `--author`, and
`--email`; when omitted the CLI discovers sensible defaults from the ambient
Git configuration and environment.

## Global Editing Flags

Many commands share the following flags for supplying rich content:

| Flag | Description |
| ---- | ----------- |
| `--description <TEXT>` / `--description-file <PATH>` | Provide a Markdown description inline or via a file. Mutually exclusive. |
| `--comment <TEXT>` / `--comment-file <PATH>` | Supply comment content inline or from a file. Mutually exclusive. |
| `--editor` / `--no-editor` | Force launching (or skipping) `$GIT_MILE_EDITOR` / `$EDITOR` for comment entry. |
| `--allow-empty` | Permit empty comment bodies when using files or editors. |
| `--label <NAME>` / `--label-file <PATH>` | Attach one or more labels. Top-level commands treat labels as additive sets. |

## `git mile create`

Create issues and milestones with optional description, initial comment, and
labels in a single transaction.

```bash
# milestone with description, comment, labels
git-mile create milestone "Q4 Launch Readiness" \
  --description-file docs/launch-readiness.md \
  --comment "Kickoff: align on launch criteria" \
  --label roadmap --label coordination

# issue defaults to open; add --draft for draft issues
git-mile create issue "Finalize onboarding copy" \
  --description "Track copy updates for the new flow" \
  --label ux --label docs
```

Key flags:

* `--description`, `--description-file`
* `--comment`, `--comment-file`, `--editor`, `--no-editor`, `--allow-empty`
* `--label`, `--label-file`
* `--message <TEXT>` – override the operation commit message
* `--draft` – create issues or milestones in the `draft` status
* `--json` – emit a structured response (ID, status, labels, comment summary)

## `git mile comment`

Append comments to existing issues or milestones. Comments are normalized as
Markdown before they are persisted.

```bash
# quote an existing comment by ID before replying
git-mile comment milestone 12345678-... \
  --quote 87d3772e-... \
  --comment-file reply.md

# use the configured editor with a templated header
git-mile comment issue 87654321-... --editor
```

Key flags:

* `--comment`, `--comment-file`, `--editor`, `--no-editor`
* `--allow-empty` – permit empty bodies (otherwise rejected)
* `--quote <COMMENT_ID>` – pre-fill the editor with a quoted comment body
* `--dry-run` – format the comment without writing it
* `--json` – emit `{ created, comment_id, body, timestamp }`

## `git mile label`

Add or remove labels on issues and milestones.

```bash
# add labels and drop a stale one in a single invocation
git-mile label milestone 12345678-... --add release --add qa --remove pending

# declare the exact label set (empties any labels not listed)
git-mile label issue 87654321-... --set backend --set docs
```

Operations:

* `--add <LABEL>` – add labels (deduplicated)
* `--remove <LABEL>` – remove labels when present
* `--set <LABEL>` – replace the entire label set
* `--clear` – strip all labels
* `--json` – emit `{ added, removed, current }`

## `git mile list`

Surface summaries for milestones and issues. By default the CLI prints a table
with ID, status, title, labels, comment count, and last update.

```bash
# condense closed items unless --all is supplied
git-mile list milestone --long

# custom column order with JSON output for automation
git-mile list issue --columns id,title,status,comments --json
```

Flags:

* `--all` – include closed items (default filters them)
* `--long` – append description preview and latest comment excerpt
* `--columns id,title,status,labels,comments,updated` – choose column order
* `--format table|raw|json` – presentation (overridden by `--json`)
* `--json` – equivalent to `--format json`

To revert to the legacy list format temporarily, set
`GIT_MILE_LIST_LEGACY=1`.

## `git mile show <MILESTONE_ID>`

Display milestone details with Markdown-rendered description, comment timeline,
labels, and metadata. The CLI automatically truncates older comments; supply a
larger limit for long-lived discussions.

```bash
# render the last 10 comments in the timeline
git-mile show 12345678-... --limit-comments 10

# capture structured output for tooling
git-mile show 12345678-... --json | jq '.comments[0].body'
```

Flags:

* `--limit-comments <N>` – display only the most recent N comments (default 20)
* `--json` – emit `{ id, title, status, description, labels, comments[], stats }`

> **Note:** `git mile show` currently targets milestones. Issue-specific detail
> output is planned for a follow-up milestone; in the meantime `git mile list issue --json`
> surfaces equivalent structured data.

## MCP Integration Mode

`git mile mcp-server` exposes the core query engine over the Model Context
Protocol (MCP) so that external clients such as Claude Desktop can drive the
same list/show flows as the CLI.

### Quick start

```bash
# launch the stdio MCP server for the current repository
git mile mcp-server --repo $(pwd) --log-level info --handshake-timeout 30
```

1. Start the server in the repository you want the client to inspect.
2. Point your MCP-compatible client at the `git-mile` binary and pass the
   working directory (many clients expose this as `command` + `args` or
   `cwd`).
3. Trigger a `git_mile.list` or `git_mile.show` action inside the client to
   confirm the connection.

The server emits structured logs on stderr and stays alive until the client
requests shutdown or the optional idle timeout elapses.

### Flags and configuration

- `--log-level <trace|debug|info|warn|error>` tunes the tracing output.
- `--handshake-timeout <SECONDS>` aborts initialization if the client does not
  send `initialize` within the allotted time.
- `--idle-shutdown <SECONDS>` optionally stops the server after a period with no
  in-flight requests.
- `--protocol v1` selects the current MCP protocol surface.

See `docs/reference/mcp-server.md` for full flag details.

### External clients

Detailed setup guides for Claude Desktop, Cursor, and other MCP clients live in
`docs/reference/mcp-clients.md`. Each guide documents the required command
template, expected environment variables, and known quirks.

### Known constraints

- Only a single client connection is supported per server instance today.
- Requests are processed sequentially; long-running queries block subsequent
  calls until they finish.
- The server shares the repository lock with the CLI. Avoid running mutating
  commands in parallel with an MCP session.
- Windows terminals must forward CTRL+C to terminate both the CLI wrapper and
  the spawned MCP server process.

## Troubleshooting

* When an editor produces empty content, re-run with `--allow-empty` or provide
  a comment via `--comment`.
* The CLI acquires a write lock for `create`, `comment`, and `label` commands.
  If another process holds the lock, the command waits—use `--message` to help
  identify queued operations in the history.
* Commands that mutate labels deduplicate additions and ignore removals for
  labels not currently applied; check the JSON response to confirm the effective
  change.
