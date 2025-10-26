# MCP Client Guides

This guide captures how to connect popular MCP-compatible clients to
`git mile mcp-server`. Each client launches the `git-mile` binary, wires stdio,
and forwards the repository root via working-directory configuration.

## Claude Desktop

Claude Desktop (>= 0.7.0) can launch a custom MCP server by adding the following
snippet to `~/Library/Application Support/Claude/claude_desktop_config.json`
on macOS or the equivalent path on Windows.

```json
{
  "mcpServers": {
    "git-mile": {
      "command": "/usr/local/bin/git-mile",
      "args": ["mcp-server", "--repo", "/Users/you/work/git-mile"],
      "autoRestart": true,
      "allowedAttributions": ["list", "show"]
    }
  }
}
```

Tips:

- Claude passes the working directory implicitly; supply `--repo` when the
  repository is outside Claude's cwd.
- Rotate `--log-level debug` when inspecting protocol messages; the client UI
  surfaces stderr output in the session details pane.
- Claude reuses the previous session; restart the app after updating git-mile.

## Cursor

Cursor supports MCP servers through `.cursor/mcp.json` inside the workspace.

```json
{
  "servers": [
    {
      "name": "git-mile",
      "command": "git-mile",
      "args": ["mcp-server"],
      "cwd": "${workspaceRoot}",
      "env": {
        "RUST_LOG": "git_mile_core::mcp=debug"
      }
    }
  ]
}
```

Notes:

- `${workspaceRoot}` expands to the project directory; ensure it matches the
  git-mile repository root or pass `--repo`.
- Cursor terminates the server when the workspace closes. Use
  `--idle-shutdown 0` to keep the server alive until manual shutdown.
- The MCP inspector inside Cursor shows request/response JSON for quick
  debugging.

## Model Context Protocol Inspector

Run `npx @modelcontextprotocol/inspector@latest` to debug interactions. Launch
the inspector, then connect it to git-mile:

```bash
npx @modelcontextprotocol/inspector connect \
  --command git-mile \
  --arg mcp-server \
  --arg --repo \
  --arg $(pwd)
```

The inspector renders every MCP frame, making it ideal for verifying new
methods or troubleshooting parsing errors.

## Common troubleshooting

- Ensure the git-mile binary is on the PATH visible to the client process.
- Windows users may need to wrap the command with `cmd /C` depending on the
  client shell integration.
- If the client reports a handshake timeout, increase
  `--handshake-timeout` (e.g. to 60 seconds) and check for stale stdin/stdout
  redirections.
- Use `GIT_MILE_CACHE_DIR` to differentiate caches between multiple repositories
  when running concurrent sessions.
