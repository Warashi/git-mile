# Remote Synchronization

git-mile supports distributed task tracking through remote synchronization. Tasks are stored as Git refs under `refs/git-mile/tasks/*`, which can be pushed to and pulled from remote repositories using standard Git protocols.

## Overview

### Architecture

git-mile's event sourcing design combined with CRDTs (Conflict-free Replicated Data Types) enables offline-first, distributed task tracking:

- **Events are immutable**: All task changes are stored as append-only events
- **CRDT convergence**: Concurrent edits from different repositories merge automatically without conflicts
- **Git-backed storage**: Events are stored as Git commits, leveraging Git's existing infrastructure for remotes, authentication, and synchronization

### Sync Operations

- **Push**: Upload local task refs to a remote repository
- **Pull**: Fetch task refs from a remote and merge them with local refs

When refs diverge (both local and remote have new commits), git-mile automatically creates merge commits. The CRDT design ensures that the final task state converges regardless of merge order.

## Setting Up Remotes

git-mile uses your repository's existing Git remotes. Before pushing or pulling tasks, configure a remote:

```bash
# Add a remote (if not already configured)
git remote add origin https://github.com/username/repo.git

# Verify remotes
git remote -v
```

## Commands

### Push

Push local task refs to a remote repository:

```bash
# Push to default remote (origin)
git-mile push

# Push to a specific remote
git-mile push --remote upstream

# Force push (overwrites remote refs - use with caution)
git-mile push --force
```

**Options:**
- `--remote <name>` or `-r <name>`: Specify remote name (default: `origin`)
- `--force` or `-f`: Force push, overwriting remote refs

**When to use:**
- After creating or updating tasks locally
- To share tasks with collaborators
- To back up tasks to a remote server

### Pull

Fetch and merge task refs from a remote repository:

```bash
# Pull from default remote (origin)
git-mile pull

# Pull from a specific remote
git-mile pull --remote upstream
```

**Options:**
- `--remote <name>` or `-r <name>`: Specify remote name (default: `origin`)

**When to use:**
- To fetch tasks created by collaborators
- To synchronize after working offline
- Before making changes to minimize merge conflicts

## Workflows

### Basic Workflow: Single User

```bash
# Initial setup
git clone https://github.com/username/repo.git
cd repo

# Create and push tasks
git-mile new --title "Implement feature X"
git-mile push

# On another machine
git clone https://github.com/username/repo.git
cd repo
git-mile pull
git-mile ls
```

### Collaboration Workflow

```bash
# Alice creates a task
git-mile new --title "Review PR #123"
git-mile push

# Bob fetches the task
git-mile pull
git-mile ls

# Bob adds a comment
git-mile comment --task <task-id> --message "I can review this"
git-mile push

# Alice pulls the update
git-mile pull
git-mile show --task <task-id>
```

### Offline Work and Sync

```bash
# Work offline - create multiple tasks
git-mile new --title "Task 1"
git-mile new --title "Task 2"
git-mile new --title "Task 3"

# Later, when online
git-mile pull   # Fetch remote changes
git-mile push   # Upload local changes
```

### Handling Concurrent Modifications

git-mile's CRDT design handles concurrent edits automatically:

```bash
# Scenario: Both Alice and Bob modify the same task offline

# Alice adds label "priority:high"
git-mile new --title "Fix bug" --label "priority:high"
git-mile push

# Bob (before pulling) adds label "type:bug" to the same task
git-mile new --title "Fix bug" --label "type:bug"
git-mile push  # This will fail - remote has diverged

# Bob pulls to merge
git-mile pull

# Now the task has both labels: ["priority:high", "type:bug"]
# Bob can now push
git-mile push

# Alice pulls the merged result
git-mile pull
```

The CRDT ensures both labels are preserved. Sets (labels, assignees, parent/child links) use ORSWOT, and single values (title, state, description) use Last-Write-Wins with UUIDv7 timestamp tie-breaking.

## Authentication

git-mile uses Git's authentication mechanisms:

- **HTTPS**: Uses credential helpers configured in `~/.gitconfig`
- **SSH**: Uses SSH keys configured in `~/.ssh/`

Configure authentication as you would for regular Git operations:

```bash
# HTTPS with credential caching
git config --global credential.helper cache

# SSH
ssh-add ~/.ssh/id_ed25519
```

## Troubleshooting

### Remote not found

**Error:**
```
Error: Remote 'origin' not found
```

**Solution:**
```bash
# Verify remotes
git remote -v

# Add missing remote
git remote add origin https://github.com/username/repo.git
```

### Push rejected (non-fast-forward)

**Error:**
```
Failed to push to remote 'origin'
```

**Solution:**
```bash
# Pull first to merge remote changes
git-mile pull

# Then push
git-mile push

# Or force push (only if you're certain)
git-mile push --force
```

### Authentication failed

**Error:**
```
Failed to push to remote 'origin'
Caused by: authentication required
```

**Solution:**
```bash
# For HTTPS: Configure credential helper
git config --global credential.helper store

# For SSH: Add your SSH key
ssh-add ~/.ssh/id_ed25519

# Verify SSH connection
ssh -T git@github.com
```

### No tasks to push

**Message:**
```
No task refs to push
```

This is informational, not an error. You haven't created any tasks yet:

```bash
git-mile new --title "First task"
git-mile push
```

## Best Practices

1. **Pull before push**: Minimize merge conflicts by pulling recent changes before pushing your work
2. **Push regularly**: Back up your work and share progress with collaborators frequently
3. **Avoid force push**: Only use `--force` when you're certain you want to overwrite remote refs
4. **Use descriptive task titles**: Makes collaboration easier when others pull your tasks
5. **Commit working tree changes separately**: git-mile refs are independent of your working tree; commit code changes with `git commit` as usual

## Advanced Topics

### Custom Remotes

You can use any Git remote protocol:

```bash
# File-based (local network)
git remote add backup file:///mnt/backup/repo.git

# SSH with custom port
git remote add server ssh://git@server:2222/repo.git

# Multiple remotes
git remote add origin https://github.com/user/repo.git
git remote add backup https://gitlab.com/user/repo.git

git-mile push --remote origin
git-mile push --remote backup
```

### Ref Namespace

git-mile stores tasks under `refs/git-mile/tasks/<task-id>`. You can inspect these refs directly:

```bash
# List all task refs
git for-each-ref refs/git-mile/tasks/

# View a specific task's commit history
git log refs/git-mile/tasks/<task-id>

# View event JSON
git show refs/git-mile/tasks/<task-id>
```

### Integration with Git Hooks

You can automate push/pull operations using Git hooks:

```bash
# .git/hooks/post-commit
#!/bin/bash
git-mile push --remote origin 2>/dev/null || true
```

Make the hook executable:

```bash
chmod +x .git/hooks/post-commit
```

## Related Documentation

- [CLAUDE.md](../CLAUDE.md): Architecture overview and event sourcing design
- [hooks.md](hooks.md): Hook system for custom task automation
