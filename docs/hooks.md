# git-mile Hooks

git-mile provides a hook system similar to Git hooks, allowing you to execute custom scripts before and after task operations.

## Overview

Hooks are scripts that run automatically at specific points in the task lifecycle. They can be used for:

- Custom validation of task state transitions
- External notifications (Slack, GitHub Issues, etc.)
- Automatic labeling or assignment
- Logging and auditing

## Hook Types

### Pre-hooks (can reject operations)

- `pre-task-create` - Before creating a new task
- `pre-task-update` - Before updating a task
- `pre-state-change` - Before changing task state
- `pre-comment-add` - Before adding a comment
- `pre-relation-change` - Before modifying parent/child relationships
- `pre-event` - Before any event

**Pre-hooks can reject an operation by exiting with a non-zero status code.**

### Post-hooks (notification only)

- `post-task-create` - After creating a new task
- `post-task-update` - After updating a task
- `post-state-change` - After changing task state
- `post-comment-add` - After adding a comment
- `post-relation-change` - After modifying parent/child relationships
- `post-event` - After any event

**Post-hooks cannot cancel operations. If they fail, a warning is logged but the operation completes.**

## Configuration

Configure hooks in `.git-mile/config.toml`:

```toml
[hooks]
# Enable or disable all hooks
enabled = true

# List of specific hooks to disable
disabled = []

# Timeout in seconds for hook execution
timeout = 30

# Run post-hooks asynchronously (not implemented yet)
async_post_hooks = false

# Custom hooks directory (defaults to .git-mile/hooks)
# hooks_dir = ".git-mile/hooks"
```

## Hook Scripts

Hook scripts must be:

1. **Executable**: Set execute permission with `chmod +x`
2. **Located in `.git-mile/hooks/`**: Use the hook type name (e.g., `pre-task-create`)
3. **Accept JSON on stdin**: Event data is provided as JSON
4. **Write JSON to stdout** (optional): Can modify the event
5. **Use stderr for error messages**: Captured and shown to the user

### Input Format

Hooks receive a JSON object on stdin with the following structure:

```json
{
  "event": {
    "schema": "git-mile-event@1",
    "id": "019a9440-2270-72f1-8306-0bf4ea84d34e",
    "ts": "2025-11-18T00:00:00Z",
    "actor": {
      "name": "John Doe",
      "email": "john@example.com"
    },
    "task": "019a9440-2270-72f1-8306-0bf4ea84d34e",
    "kind": {
      "type": "taskCreated",
      "title": "Implement feature X",
      "labels": ["feature"],
      "assignees": ["john"],
      "description": "...",
      "state": "state/todo",
      "state_kind": "todo"
    }
  }
}
```

### Exit Codes

- **0**: Success (pre-hooks allow operation to proceed)
- **Non-zero**: Failure (pre-hooks reject the operation, post-hooks log a warning)

### Error Handling

- **Pre-hook failure**: Operation is cancelled, error message shown to user
- **Post-hook failure**: Warning logged, operation already completed
- **Timeout**: Hook is killed after the configured timeout

## Example Scripts

### Example 1: Validate State Transitions

Prevent skipping states (e.g., todo → done without in_progress):

```bash
#!/bin/bash
# .git-mile/hooks/pre-state-change

set -e

# Read JSON input
INPUT=$(cat)

# Extract current and new state
CURRENT_STATE=$(echo "$INPUT" | jq -r '.event.kind.from // "null"')
NEW_STATE=$(echo "$INPUT" | jq -r '.event.kind.to')

# Validate transition
if [ "$CURRENT_STATE" = "state/todo" ] && [ "$NEW_STATE" = "state/done" ]; then
    echo "Cannot transition directly from todo to done. Must go through in_progress first." >&2
    exit 1
fi

exit 0
```

### Example 2: Slack Notification

Send notifications to Slack when tasks are created:

```bash
#!/bin/bash
# .git-mile/hooks/post-task-create

set -e

INPUT=$(cat)
TITLE=$(echo "$INPUT" | jq -r '.event.kind.title')
TASK_ID=$(echo "$INPUT" | jq -r '.event.task')
ACTOR=$(echo "$INPUT" | jq -r '.event.actor.name')

curl -X POST "$SLACK_WEBHOOK_URL" \
    -H 'Content-Type: application/json' \
    -d "{\"text\": \"New task created by $ACTOR: $TITLE ($TASK_ID)\"}"

exit 0
```

### Example 3: Automatic Labeling

Automatically add labels based on title keywords:

```bash
#!/bin/bash
# .git-mile/hooks/pre-task-create

set -e

INPUT=$(cat)
TITLE=$(echo "$INPUT" | jq -r '.event.kind.title')

# Add "bug" label if title contains "fix" or "bug"
if echo "$TITLE" | grep -iq -E "fix|bug"; then
    OUTPUT=$(echo "$INPUT" | jq '.event.kind.labels += ["bug"]')
    echo "$OUTPUT"
    exit 0
fi

# Otherwise, pass through unchanged
echo "$INPUT"
exit 0
```

## Testing Hooks

### Manual Testing

1. Create a hook script:
   ```bash
   cat > .git-mile/hooks/pre-task-create << 'EOF'
   #!/bin/bash
   echo "Hook executed!" >&2
   exit 0
   EOF
   chmod +x .git-mile/hooks/pre-task-create
   ```

2. Test it:
   ```bash
   git-mile new "Test task"
   ```

3. Check for the "Hook executed!" message

### Unit Testing

Hook scripts can be tested independently:

```bash
echo '{"event":{"kind":{"type":"taskCreated","title":"Test"}}}' | \
    .git-mile/hooks/pre-task-create
```

## Troubleshooting

### Hook not executing

- Check `hooks.enabled = true` in `.git-mile/config.toml`
- Verify hook is not in `hooks.disabled` list
- Ensure script has execute permission (`chmod +x`)
- Check script is in correct location (`.git-mile/hooks/`)
- Verify hook name matches exactly (use `-` not `_`)

### Hook times out

- Increase `hooks.timeout` in config
- Optimize hook script (avoid expensive operations)
- Use `async_post_hooks = true` for post-hooks (when implemented)

### Hook rejects operation incorrectly

- Test hook independently with sample JSON
- Check exit code and stderr output
- Add debug logging to hook script

## Security Considerations

- **Never commit secrets** in hook scripts
- Use environment variables for sensitive data
- Be careful with untrusted input (sanitize JSON fields)
- Limit hook execution permissions
- Review hook scripts before installation

## Phase 1 Limitations

The current implementation (Phase 1) has these limitations:

- Only `pre-task-create` and `post-task-create` hooks are integrated
- Other hook types are defined but not yet called by TaskWriter
- Post-hooks run synchronously (`async_post_hooks` not implemented)
- No hook output modification support (reading stdout)

These will be addressed in Phase 2 and beyond.
