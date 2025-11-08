# State Kind Persistence & Migration

State kinds classify workflow states (e.g. `todo`, `in_progress`, `done`) so filters, the TUI, and MCP consumers can reason about a task without loading project-specific workflow config. This document explains how the information is stored in Git, how older events remain compatible, and which migration paths are available for repositories created before `state_kind` was introduced.

## Event Schema

`git-mile` persists every task mutation as a commit whose message body contains a JSON blob matching the `git-mile-event@1` schema. Two event kinds now embed the derived state kind:

```jsonc
{
  "schema": "git-mile-event@1",
  "id": "01JB3TQR0QNSERY2FYR6TR15H8",
  "task": "01JB3TQMTKD4Q2Z33N0JFPC54E",
  "actor": { "name": "alice", "email": "alice@example.com" },
  "ts": "2024-11-08T12:34:56.789Z",
  "kind": {
    "type": "taskCreated",
    "title": "Implement filter UI",
    "state": "state/in-progress",
    "state_kind": "in_progress"
  }
}
```

```jsonc
{
  "kind": {
    "type": "taskStateSet",
    "state": "state/done",
    "state_kind": "done"
  }
}
```

No schema bump is required because the new `state_kind` property is optional and defaults to `null` when it is absent. All writers (CLI, TUI, MCP server) attach the kind that was resolved from the workflow at the time the event was authored, ensuring downstream tools do not need to re-run the resolution logic.

## Backward Compatibility

- Events that predate this change simply omit the `state_kind` field. The Git store deserializes those commits successfully and the field defaults to `None`.
- `TaskSnapshot::replay` and `TaskFilter` treat a missing kind as "unknown", so new filters that include/exclude kinds will skip legacy tasks until they are backfilled.
- New tests (`append_event_serializes_state_kind_into_commit_body`, `load_events_accepts_commits_without_state_kind_field`, and `replay_handles_legacy_events_missing_state_kind_payload`) protect these guarantees.

## Migration Options

1. **Append corrective events (recommended)**  
   For each legacy task, re-apply its current state using either the TUI editor (`git-mile tui` → `e` → select the same workflow state) or the MCP `update_task` tool. The newly appended `TaskStateSet` event will carry the resolved kind while keeping history intact.

2. **Scripted backfill via MCP**  
   Automate the above by calling the MCP server (`git-mile mcp`) from any MCP client, iterating over `list_tasks`/`get_task` results, and issuing `update_task` requests such as:
   ```jsonc
   {
     "tool": "update_task",
     "params": {
       "task_id": "01JB3TQMTKD4Q2Z33N0JFPC54E",
       "state": "state/in-progress"
     }
   }
   ```
   Feeding each task's current `state` back into `update_task` generates an event that now includes `state_kind`.

3. **History rewrite (advanced)**  
   If you need every historical event to embed the field, rewrite the commits under `refs/git-mile/tasks/*` with `git filter-repo` (or an equivalent tool) by editing each JSON payload to add the inferred `state_kind`. This approach requires coordination with collaborators because it rewrites shared refs.

Until a task is backfilled, its `state_kind` will remain `null` in CLI/TUI/MCP outputs. All other attributes continue to work, so migration can be incremental.
