# `git mile mcp-server`

`git mile mcp-server` launches the Model Context Protocol (MCP) endpoint that
bridges external tools to the `git_mile_core` query engine. The server speaks the
`git_mile.list` and `git_mile.show` tools over stdio and reuses the same filters,
sorting rules, and JSON payloads as the CLI.

## Usage

```bash
git mile mcp-server \
  --repo /path/to/git-mile-repo \
  --log-level info \
  --handshake-timeout 30 \
  --idle-shutdown 300
```

When invoked without `--repo`, the command walks up from the current working
directory to find the nearest repository root (same behaviour as other CLI
commands). The server blocks the foreground process and listens for a single MCP
client connection via stdio.

## Options

| Flag | Description |
| ---- | ----------- |
| `--log-level <trace|debug|info|warn|error>` | Controls tracing output written to stderr. |
| `--handshake-timeout <SECONDS>` | Fails the session if the client does not complete `initialize` within the timeout (default: `30`). |
| `--idle-shutdown <SECONDS>` | Stops the server after the connection stays idle for the specified number of seconds. When omitted the server waits indefinitely. |
| `--protocol <v1>` | Selects the protocol surface. Future protocol revisions will be gated behind additional enum values. |

Environment variables:

- `GIT_MILE_REPO` — alternative to `--repo`.
- `RUST_LOG` — overrides the minimum log level (compatible with `tracing`).
- `GIT_MILE_CACHE_DIR` — aligns cache placement with the CLI.

## Lifecycle

1. The server performs repository discovery and initializes the query engine.
2. Once ready, it negotiates an MCP session via `rmcp` and exposes the
   `git_mile.list` / `git_mile.show` tools.
3. Requests are executed sequentially. Responses reuse the CLI JSON schema.
4. The session ends when:
   - the client calls `shutdown`, or
   - `--idle-shutdown` fires, or
   - stdin closes or CTRL+C is delivered.

The process exits with code `0` on graceful shutdown. Errors propagate as
non-zero exit codes and surface a diagnostic on stderr.

## Logging

- Logs default to the `info` level; raise to `debug` or `trace` for protocol
  inspection.
- Each request/response pair is annotated with a correlation ID to help match
  client traces.
- Errors map internal `anyhow::Error` values to MCP error codes and are logged
  alongside the MCP payload.

## Compatibility

- Protocol version `v1` advertises `git_mile.list` and `git_mile.show` tools and
  handles the standard `initialize`, `shutdown`, and `ping` MCP messages.
- The server expects UTF-8 JSON payloads with newline delimiters (JSON Lines).
- Only one concurrent client is supported; additional client attempts should
  spawn a separate server process.

Refer to `docs/reference/mcp-protocol.md` (ID-92) for the detailed message
schemas.
