# git-mile-hooks

Hook execution system for git-mile task tracker.

## Overview

This crate provides a Git-like hooks system for git-mile, allowing custom scripts to run before and after task operations.

## Features

- **Pre-hooks**: Can validate and reject operations
- **Post-hooks**: Run after operations complete
- **Timeout handling**: Configurable execution timeout
- **JSON I/O**: Event data passed via stdin/stdout
- **Flexible configuration**: Enable/disable hooks globally or individually

## Usage

```rust
use git_mile_hooks::{HookExecutor, HookKind, HookContext, HooksConfig};
use std::path::PathBuf;

// Configure hooks
let config = HooksConfig {
    enabled: true,
    disabled: vec![],
    timeout: 30,
    async_post_hooks: false,
    hooks_dir: PathBuf::from(".git-mile/hooks"),
};

// Create executor
let executor = HookExecutor::new(config, PathBuf::from(".git-mile"));

// Execute a hook
let context = HookContext::new(&event);
let result = executor.execute(HookKind::PreTaskCreate, &context)?;

// Check result
if result.exit_code == 0 {
    println!("Hook succeeded");
} else {
    eprintln!("Hook failed: {}", result.stderr);
}
```

## Hook Types

### Pre-hooks (can reject operations)
- `pre-task-create`
- `pre-task-update`
- `pre-state-change`
- `pre-comment-add`
- `pre-relation-change`
- `pre-event`

### Post-hooks (notification only)
- `post-task-create`
- `post-task-update`
- `post-state-change`
- `post-comment-add`
- `post-relation-change`
- `post-event`

## Configuration

Hooks are configured in `.git-mile/config.toml`:

```toml
[hooks]
enabled = true
disabled = []
timeout = 30
async_post_hooks = false
```

## Hook Scripts

Hook scripts should:

1. Be executable (`chmod +x`)
2. Be located in `.git-mile/hooks/`
3. Accept JSON on stdin
4. Return exit code 0 for success
5. Write error messages to stderr

Example hook script:

```bash
#!/bin/bash
# .git-mile/hooks/pre-task-create

# Read JSON input
INPUT=$(cat)

# Extract task title
TITLE=$(echo "$INPUT" | jq -r '.event.kind.title')

# Validate
if [ -z "$TITLE" ]; then
    echo "Task title cannot be empty" >&2
    exit 1
fi

exit 0
```

## Error Handling

- **NotFound**: Hook script doesn't exist or hooks are disabled
- **Rejected**: Pre-hook returned non-zero exit code
- **Timeout**: Hook exceeded configured timeout
- **ExecutionFailed**: Hook failed to execute
- **Io**: I/O error occurred
- **Json**: JSON serialization/deserialization failed

## Testing

Run tests with:

```bash
cargo test --package git-mile-hooks
```

Note: Some tests require filesystem setup and are marked `#[ignore]`. Run them with:

```bash
cargo test --package git-mile-hooks -- --ignored
```

## Documentation

See [hooks.md](../../docs/hooks.md) for detailed documentation and examples.

## License

Same as the git-mile project.
