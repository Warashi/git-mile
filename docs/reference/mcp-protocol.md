# MCP Protocol Specification (git-mile)

`git mile mcp-server` exposes the Model Context Protocol (MCP) over stdio. This
document captures the contract that external clients rely on for the `git_mile`
tool suite.

## Supported methods

| Method | Direction | Notes |
| --- | --- | --- |
| `initialize` | Client → Server | Announces client capabilities. Server replies with version `2024-11-05` and `tools` capability enabled. |
| `tools/list` | Client → Server | Optional discovery call. Returns `git_mile.list` and `git_mile.show`. |
| `tools/call` (`git_mile.list`) | Client → Server | Mirrors the CLI `list` command, including filtering, sorting, cursoring. |
| `tools/call` (`git_mile.show`) | Client → Server | Returns a full entity payload, equivalent to `git mile show --json`. |
| `ping` | Either direction | Used by some clients to check liveness; echoed without side effects. |
| `shutdown` | Client → Server | Requests graceful termination. Server responds with `null` and closes the transport. |

The server does not implement resources, prompts, or sampling in the current
revision.

## Tool schemas

### `git_mile.list`

```jsonc
// Call arguments
{
  "entity": "milestone" | "issue",
  "filter": "status == \"open\"" /* optional Git Mile filter DSL */,
  "sort": ["updated:desc"],        // optional; array of field[:order]
  "limit": 20,                    // optional page size
  "cursor": "opaque-cursor",      // optional pagination cursor
  "includeClosed": false          // include closed entities when true
}
```

Response payload (serialized into `CallToolResult.content[0].text` as JSON):

```jsonc
{
  "items": [
    {
      "id": "8903a9b6-...",
      "title": "Milestone Alpha",
      "status": "open",
      "labels": ["alpha"],
      "stats": { "comment_count": 1, "open_issues": 2 }
    }
  ],
  "nextCursor": null
}
```

When the query returns additional pages, `nextCursor` carries the opaque cursor.

### `git_mile.show`

```jsonc
// Call arguments
{
  "entity": "milestone" | "issue",
  "id": "UUID (entity identifier)"
}
```

Response payload mirrors the CLI JSON output and is serialized into
`CallToolResult.content[0].text`:

```jsonc
{
  "id": "8903a9b6-...",
  "title": "Milestone Alpha",
  "status": "open",
  "description": "...",
  "labels": ["alpha"],
  "comments": [{ "author": "...", "body": "...", "ts": "..." }],
  "stats": { "comment_count": 1 }
}
```

## Error model

| MCP error code | Meaning | git-mile mapping |
| --- | --- | --- |
| `-32600` (`InvalidRequest`) | Malformed JSON or missing fields | Triggered when argument decoding fails. |
| `-32601` (`MethodNotFound`) | Unknown tool name | Returned when a client calls a non-existent tool. |
| `-32602` (`InvalidParams`) | Fails validation (bad filter, cursor) | Maps the CLI validation errors. |
| `-32001` (`ServerError`) | Internal failure | Wraps unexpected `anyhow::Error`; message holds the debug summary. |

Tool errors also set `CallToolResult.is_error = true` and include a JSON `content`
entry with `{ "error": { "code": "...", "message": "..." } }`.

## Reference session

A reference handshake + list/show session is recorded in
`docs/reference/examples/mcp-session-list-show.json`. Each entry captures the
direction (`client` vs. `server`) and the full JSON frame emitted on the wire.

```jsonc
[
  {
    "direction": "client",
    "payload": { "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { ... } }
  },
  {
    "direction": "server",
    "payload": { "jsonrpc": "2.0", "id": 1, "result": { ... } }
  }
]
```

The example was captured with `npx @modelcontextprotocol/inspector` 0.5.1 connected
to `git mile mcp-server`. Tests under `core/tests/mcp_protocol.rs` validate that
the JSON remains well-formed.

## Compatibility report

- **Claude Desktop 0.7.0 (macOS)** — Works with default configuration.
  - Required settings: `command = "/usr/local/bin/git-mile"`, `args = ["mcp-server"]`.
  - Verified flows: `git_mile.list` (milestone + issue), `git_mile.show`.
  - Observed quirk: long-running list queries (>5s) display a generic spinner; no functional issue.
- **Model Context Protocol Inspector 0.5.1** — Used for regression testing and to produce the reference session log.
- Additional clients can be configured following `docs/reference/mcp-clients.md`;
  please add notes there when new validations are performed.

## Change management

- Protocol version: `2024-11-05`.
- Backwards compatibility policy: additive changes only (new optional fields,
  new tool methods). Breaking changes require bumping the protocol enum to `v2`
  and documenting migration steps.
- Track updates in `CHANGELOG.md` under the “MCP” heading and refresh the
  reference session whenever response shapes change.
