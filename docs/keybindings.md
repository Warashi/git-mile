# Keybindings Configuration

## Overview

git-mile TUI keybindings can be fully customized via a TOML configuration file. This allows you to adapt the interface to your preferred workflow and avoid conflicts with terminal multiplexer or shell keybindings.

## Quick Start

1. **Generate default configuration**:
   ```bash
   git-mile config init-keybindings
   ```

   This creates `~/.config/git-mile/config.toml` with all current default keybindings documented.

2. **Edit the configuration file**:
   ```bash
   $EDITOR ~/.config/git-mile/config.toml
   ```

3. **Restart the TUI** to apply changes:
   ```bash
   git-mile tui
   ```

## Configuration File Location

### Default Paths

- **Linux/macOS**: `~/.config/git-mile/config.toml` (XDG Base Directory)
- **Windows**: `%APPDATA%\git-mile\config.toml`

### Custom Paths

You can specify a custom path when generating the configuration:

```bash
git-mile config init-keybindings --output /path/to/custom/config.toml
```

However, the TUI will only automatically load from the default path. Custom paths are useful for:
- Version controlling your configuration
- Testing different keybinding schemes
- Sharing configurations across machines

## Configuration Format

The configuration file is divided into five sections, one for each TUI view:

```toml
[task_list]        # Main task list view
[tree_view]        # Hierarchical tree view
[state_picker]     # State selection popup
[comment_viewer]   # Comment viewer panel
[description_viewer]  # Description viewer panel
```

### Example Configuration

```toml
[task_list]
quit = ["q", "Q", "Esc"]
down = ["j", "J", "Down"]
up = ["k", "K", "Up"]
open_tree = ["Enter"]
jump_to_parent = ["p", "P"]
refresh = ["r", "R"]
add_comment = ["c", "C"]
edit_task = ["e", "E"]
create_task = ["n", "N"]
create_subtask = ["s", "S"]
copy_task_id = ["y", "Y"]
open_state_picker = ["t", "T"]
open_comment_viewer = ["v", "V"]
open_description_viewer = ["d", "D"]
edit_filter = ["f", "F"]

[tree_view]
close = ["q", "Q", "Esc"]
down = ["j", "J", "Down"]
up = ["k", "K", "Up"]
collapse = ["h", "H"]
expand = ["l", "L"]
jump = ["Enter"]

[state_picker]
close = ["q", "Q", "Esc"]
down = ["j", "J", "Down"]
up = ["k", "K", "Up"]
select = ["Enter"]

[comment_viewer]
close = ["q", "Q", "Esc"]
scroll_down = ["j", "J"]
scroll_up = ["k", "K"]
scroll_down_fast = ["Ctrl+d"]
scroll_up_fast = ["Ctrl+u"]

[description_viewer]
close = ["q", "Q", "Esc"]
scroll_down = ["j", "J"]
scroll_up = ["k", "K"]
scroll_down_fast = ["Ctrl+d"]
scroll_up_fast = ["Ctrl+u"]
```

## Supported Key Formats

### Single Characters

```toml
quit = ["q", "x"]  # Both 'q' and 'x' will quit
down = ["j"]       # Only 'j' moves down
```

**Note**: Keys are case-sensitive. `"j"` and `"J"` are different keys.

### Special Keys

```toml
open_tree = ["Enter"]
close = ["Esc"]
refresh = ["Tab"]
delete = ["Delete"]
```

Supported special keys:
- `Enter`, `Esc`, `Tab`, `Backspace`, `Delete`, `Insert`
- `Up`, `Down`, `Left`, `Right` (arrow keys)
- `Home`, `End`, `PageUp`, `PageDown`

### Modified Keys

```toml
scroll_down_fast = ["Ctrl+d"]
jump_to_parent = ["Alt+p"]
expand = ["Shift+Right"]
```

Supported modifiers:
- `Ctrl` or `Control`
- `Alt`
- `Shift`

You can combine modifiers (though terminal support varies):
```toml
special_action = ["Ctrl+Alt+Delete"]
```

### Multiple Keys per Action

Each action can have multiple keybindings:

```toml
quit = ["q", "Q", "Esc"]  # Any of these quits
down = ["j", "Down"]      # Both j and Down arrow work
```

The TUI help text displays only the first key in each list.

## Available Actions

### Task List View

| Action | Description | Default |
|--------|-------------|---------|
| `quit` | Exit the application | `["q", "Q", "Esc"]` |
| `down` | Move down in the list | `["j", "J", "Down"]` |
| `up` | Move up in the list | `["k", "K", "Up"]` |
| `open_tree` | Open tree view | `["Enter"]` |
| `jump_to_parent` | Jump to parent task | `["p", "P"]` |
| `refresh` | Refresh task list | `["r", "R"]` |
| `add_comment` | Add comment | `["c", "C"]` |
| `edit_task` | Edit current task | `["e", "E"]` |
| `create_task` | Create new task | `["n", "N"]` |
| `create_subtask` | Create subtask | `["s", "S"]` |
| `copy_task_id` | Copy task ID | `["y", "Y"]` |
| `open_state_picker` | Open state picker | `["t", "T"]` |
| `open_comment_viewer` | View comments | `["v", "V"]` |
| `open_description_viewer` | View description | `["d", "D"]` |
| `edit_filter` | Edit filter | `["f", "F"]` |

### Tree View

| Action | Description | Default |
|--------|-------------|---------|
| `close` | Close tree view | `["q", "Q", "Esc"]` |
| `down` | Move down | `["j", "J", "Down"]` |
| `up` | Move up | `["k", "K", "Up"]` |
| `collapse` | Collapse node | `["h", "H"]` |
| `expand` | Expand node | `["l", "L"]` |
| `jump` | Jump to selected task | `["Enter"]` |

### State Picker

| Action | Description | Default |
|--------|-------------|---------|
| `close` | Close picker | `["q", "Q", "Esc"]` |
| `down` | Move down | `["j", "J", "Down"]` |
| `up` | Move up | `["k", "K", "Up"]` |
| `select` | Select state | `["Enter"]` |

### Comment/Description Viewers

| Action | Description | Default |
|--------|-------------|---------|
| `close` | Close viewer | `["q", "Q", "Esc"]` |
| `scroll_down` | Scroll down | `["j", "J"]` |
| `scroll_up` | Scroll up | `["k", "K"]` |
| `scroll_down_fast` | Half-page down | `["Ctrl+d"]` |
| `scroll_up_fast` | Half-page up | `["Ctrl+u"]` |

## Validation Rules

When loading your configuration, git-mile validates:

1. **All required actions are defined**: Every action must have at least one key
2. **No key conflicts within a view**: A key can't be bound to multiple actions in the same view
3. **Valid key expressions**: All key strings must be parseable

### Example: Key Conflict Error

```toml
[task_list]
quit = ["j"]
down = ["j"]  # ❌ Error: 'j' is already bound to 'quit'
```

**Fix**: Use different keys for each action:
```toml
[task_list]
quit = ["q"]
down = ["j"]  # ✓ OK
```

### Keys Can Be Shared Across Views

The same key can be used in different views without conflict:

```toml
[task_list]
quit = ["q"]  # ✓ OK

[tree_view]
close = ["q"]  # ✓ OK - different view
```

## Use Cases and Examples

### Vim-like Navigation

Default keybindings already use vim-style `hjkl`:

```toml
[task_list]
down = ["j"]
up = ["k"]
# ... other actions

[tree_view]
collapse = ["h"]
expand = ["l"]
down = ["j"]
up = ["k"]
```

### Arrow-only Navigation

Prefer arrow keys exclusively:

```toml
[task_list]
down = ["Down"]
up = ["Up"]
open_tree = ["Right"]
jump_to_parent = ["Left"]
# ... define all other actions
```

### Tmux-friendly Bindings

Avoid `Ctrl+d` if you use it in tmux:

```toml
[comment_viewer]
scroll_down_fast = ["Ctrl+f"]  # Instead of Ctrl+d
scroll_up_fast = ["Ctrl+b"]    # Instead of Ctrl+u
```

### Single-key Bindings

Minimal configuration with no alternatives:

```toml
[task_list]
quit = ["q"]
down = ["j"]
up = ["k"]
# ... one key per action
```

### Dvorak Layout

Adapt to Dvorak keyboard layout:

```toml
[task_list]
down = ["h"]  # Dvorak: h is where j is on QWERTY
up = ["t"]    # Dvorak: t is where k is on QWERTY
# ... adjust other keys as needed
```

## Troubleshooting

### Configuration Not Loading

**Symptom**: Changes to config file don't take effect

**Checklist**:
1. Verify file path: `~/.config/git-mile/config.toml`
2. Check file permissions (must be readable)
3. Restart `git-mile tui` (configuration is loaded at startup)
4. Look for error messages when TUI starts

### Invalid TOML Syntax

**Symptom**: TUI fails to start or shows parse error

**Solution**:
- Validate TOML syntax: https://www.toml-lint.com/
- Check for:
  - Unmatched quotes
  - Missing commas in arrays
  - Unescaped backslashes

```toml
# ❌ Invalid
quit = ["q", "Q"  # Missing closing bracket

# ✓ Valid
quit = ["q", "Q"]
```

### Key Conflicts Detected

**Symptom**: Error message about multiple actions bound to same key

**Solution**: Review the conflict message and assign unique keys:

```
Error: Key 'j' is bound to multiple actions in task_list: ["quit", "down"]
```

Fix by changing one of the conflicting bindings.

### Unknown Key Name

**Symptom**: Error about invalid key expression

**Solution**: Check spelling and supported key names:

```toml
# ❌ Invalid
quit = ["Escape"]  # Should be "Esc"

# ✓ Valid
quit = ["Esc"]
```

### Empty Keybinding

**Symptom**: Error about missing at least one key

**Solution**: Every action must have at least one key:

```toml
# ❌ Invalid
quit = []

# ✓ Valid
quit = ["q"]
```

## Advanced Topics

### Help Text Generation

The TUI bottom bar dynamically generates help text from your configuration. Only the **first key** in each action's list is displayed:

```toml
quit = ["x", "q", "Esc"]  # Help shows: x:終了
```

### Configuration Versioning

You can version control your keybindings:

```bash
# Generate config
git-mile config init-keybindings

# Add to git
git add ~/.config/git-mile/config.toml
git commit -m "Add custom git-mile keybindings"

# Share across machines
git push origin main
```

### Fallback Behavior

If the configuration file doesn't exist, git-mile uses hardcoded defaults identical to the generated configuration.

## See Also

- [README.md](../README.md) - Main documentation
- [CLAUDE.md](../CLAUDE.md) - Development guidelines
