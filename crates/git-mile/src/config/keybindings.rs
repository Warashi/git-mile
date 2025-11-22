//! Keybindings configuration for the TUI.

use anyhow::{anyhow, bail, Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;
use std::path::PathBuf;

macro_rules! vec_of_strings {
    ($($s:expr),* $(,)?) => {
        vec![$($s.to_string()),*]
    };
}

/// Keybindings configuration for all TUI views.
#[derive(Debug, Clone, Deserialize)]
pub struct KeyBindingsConfig {
    /// Keybindings for the task list view.
    pub task_list: TaskListKeyBindings,
    /// Keybindings for the tree view.
    pub tree_view: TreeViewKeyBindings,
    /// Keybindings for the state picker.
    pub state_picker: StatePickerKeyBindings,
    /// Keybindings for the comment viewer.
    pub comment_viewer: ViewerKeyBindings,
    /// Keybindings for the description viewer.
    pub description_viewer: ViewerKeyBindings,
}

/// Keybindings for the task list view.
#[derive(Debug, Clone, Deserialize)]
pub struct TaskListKeyBindings {
    /// Quit the application.
    pub quit: Vec<String>,
    /// Move down in the list.
    pub down: Vec<String>,
    /// Move up in the list.
    pub up: Vec<String>,
    /// Open tree view.
    pub open_tree: Vec<String>,
    /// Jump to parent task.
    pub jump_to_parent: Vec<String>,
    /// Refresh task list.
    pub refresh: Vec<String>,
    /// Add a comment to the selected task.
    pub add_comment: Vec<String>,
    /// Edit the selected task.
    pub edit_task: Vec<String>,
    /// Create a new task.
    pub create_task: Vec<String>,
    /// Create a subtask of the selected task.
    pub create_subtask: Vec<String>,
    /// Copy task ID to clipboard.
    pub copy_task_id: Vec<String>,
    /// Open state picker.
    pub open_state_picker: Vec<String>,
    /// Open comment viewer.
    pub open_comment_viewer: Vec<String>,
    /// Open description viewer.
    pub open_description_viewer: Vec<String>,
    /// Edit filter.
    pub edit_filter: Vec<String>,
}

/// Keybindings for the tree view.
#[derive(Debug, Clone, Deserialize)]
pub struct TreeViewKeyBindings {
    /// Close tree view.
    pub close: Vec<String>,
    /// Move down in the tree.
    pub down: Vec<String>,
    /// Move up in the tree.
    pub up: Vec<String>,
    /// Collapse tree node.
    pub collapse: Vec<String>,
    /// Expand tree node.
    pub expand: Vec<String>,
    /// Jump to selected task.
    pub jump: Vec<String>,
}

/// Keybindings for the state picker.
#[derive(Debug, Clone, Deserialize)]
pub struct StatePickerKeyBindings {
    /// Close state picker.
    pub close: Vec<String>,
    /// Move down in the picker.
    pub down: Vec<String>,
    /// Move up in the picker.
    pub up: Vec<String>,
    /// Select the highlighted state.
    pub select: Vec<String>,
}

/// Keybindings for viewers (comments and description).
#[derive(Debug, Clone, Deserialize)]
pub struct ViewerKeyBindings {
    /// Close viewer.
    pub close: Vec<String>,
    /// Scroll down.
    pub scroll_down: Vec<String>,
    /// Scroll up.
    pub scroll_up: Vec<String>,
    /// Scroll down fast (half page).
    pub scroll_down_fast: Vec<String>,
    /// Scroll up fast (half page).
    pub scroll_up_fast: Vec<String>,
}

impl Default for KeyBindingsConfig {
    fn default() -> Self {
        Self {
            task_list: TaskListKeyBindings::default(),
            tree_view: TreeViewKeyBindings::default(),
            state_picker: StatePickerKeyBindings::default(),
            comment_viewer: ViewerKeyBindings::default(),
            description_viewer: ViewerKeyBindings::default(),
        }
    }
}

impl Default for TaskListKeyBindings {
    fn default() -> Self {
        Self {
            quit: vec_of_strings!["q", "Q", "Esc"],
            down: vec_of_strings!["j", "J", "Down"],
            up: vec_of_strings!["k", "K", "Up"],
            open_tree: vec_of_strings!["Enter"],
            jump_to_parent: vec_of_strings!["p", "P"],
            refresh: vec_of_strings!["r", "R"],
            add_comment: vec_of_strings!["c", "C"],
            edit_task: vec_of_strings!["e", "E"],
            create_task: vec_of_strings!["n", "N"],
            create_subtask: vec_of_strings!["s", "S"],
            copy_task_id: vec_of_strings!["y", "Y"],
            open_state_picker: vec_of_strings!["t", "T"],
            open_comment_viewer: vec_of_strings!["v", "V"],
            open_description_viewer: vec_of_strings!["d", "D"],
            edit_filter: vec_of_strings!["f", "F"],
        }
    }
}

impl Default for TreeViewKeyBindings {
    fn default() -> Self {
        Self {
            close: vec_of_strings!["q", "Q", "Esc"],
            down: vec_of_strings!["j", "J", "Down"],
            up: vec_of_strings!["k", "K", "Up"],
            collapse: vec_of_strings!["h", "H"],
            expand: vec_of_strings!["l", "L"],
            jump: vec_of_strings!["Enter"],
        }
    }
}

impl Default for StatePickerKeyBindings {
    fn default() -> Self {
        Self {
            close: vec_of_strings!["q", "Q", "Esc"],
            down: vec_of_strings!["j", "J", "Down"],
            up: vec_of_strings!["k", "K", "Up"],
            select: vec_of_strings!["Enter"],
        }
    }
}

impl Default for ViewerKeyBindings {
    fn default() -> Self {
        Self {
            close: vec_of_strings!["q", "Q", "Esc"],
            scroll_down: vec_of_strings!["j", "J"],
            scroll_up: vec_of_strings!["k", "K"],
            scroll_down_fast: vec_of_strings!["Ctrl+d"],
            scroll_up_fast: vec_of_strings!["Ctrl+u"],
        }
    }
}

/// Returns the default configuration file path.
///
/// On Linux/macOS: `~/.config/git-mile/config.toml`
/// On Windows: `%APPDATA%\git-mile\config.toml`
pub fn default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|dir| dir.join("git-mile").join("config.toml"))
}

/// Ensures the config directory exists and returns the default config path.
pub fn ensure_config_dir() -> Result<PathBuf> {
    let path = default_config_path().context("Could not determine config directory")?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    Ok(path)
}

/// Parse a key string into a KeyEvent.
///
/// # Examples
/// - "j" -> KeyCode::Char('j')
/// - "Enter" -> KeyCode::Enter
/// - "Ctrl+d" -> KeyCode::Char('d') with CONTROL modifier
pub fn parse_key(s: &str) -> Result<KeyEvent> {
    let parts: Vec<&str> = s.split('+').collect();

    if parts.is_empty() {
        bail!("Empty key string");
    }

    let mut modifiers = KeyModifiers::NONE;
    let key_part = if parts.len() > 1 {
        // Parse modifiers
        for &modifier in &parts[..parts.len() - 1] {
            match modifier {
                "Ctrl" | "Control" => modifiers |= KeyModifiers::CONTROL,
                "Alt" => modifiers |= KeyModifiers::ALT,
                "Shift" => modifiers |= KeyModifiers::SHIFT,
                other => bail!("Unknown modifier: {}", other),
            }
        }
        parts[parts.len() - 1]
    } else {
        parts[0]
    };

    let code = parse_key_code(key_part)?;

    Ok(KeyEvent::new(code, modifiers))
}

fn parse_key_code(s: &str) -> Result<KeyCode> {
    match s {
        "Enter" => Ok(KeyCode::Enter),
        "Esc" => Ok(KeyCode::Esc),
        "Backspace" => Ok(KeyCode::Backspace),
        "Left" => Ok(KeyCode::Left),
        "Right" => Ok(KeyCode::Right),
        "Up" => Ok(KeyCode::Up),
        "Down" => Ok(KeyCode::Down),
        "Home" => Ok(KeyCode::Home),
        "End" => Ok(KeyCode::End),
        "PageUp" => Ok(KeyCode::PageUp),
        "PageDown" => Ok(KeyCode::PageDown),
        "Tab" => Ok(KeyCode::Tab),
        "Delete" => Ok(KeyCode::Delete),
        "Insert" => Ok(KeyCode::Insert),
        s if s.len() == 1 => {
            let ch = s.chars().next().ok_or_else(|| anyhow!("Empty char"))?;
            Ok(KeyCode::Char(ch))
        }
        other => bail!("Unknown key: {}", other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_keybindings() {
        let config = KeyBindingsConfig::default();

        // TaskList のデフォルト値を確認
        assert_eq!(config.task_list.quit, vec!["q", "Q", "Esc"]);
        assert_eq!(config.task_list.down, vec!["j", "J", "Down"]);
        assert_eq!(config.task_list.up, vec!["k", "K", "Up"]);
        assert_eq!(config.task_list.open_tree, vec!["Enter"]);
        assert_eq!(config.task_list.jump_to_parent, vec!["p", "P"]);
        assert_eq!(config.task_list.refresh, vec!["r", "R"]);
        assert_eq!(config.task_list.add_comment, vec!["c", "C"]);
        assert_eq!(config.task_list.edit_task, vec!["e", "E"]);
        assert_eq!(config.task_list.create_task, vec!["n", "N"]);
        assert_eq!(config.task_list.create_subtask, vec!["s", "S"]);
        assert_eq!(config.task_list.copy_task_id, vec!["y", "Y"]);
        assert_eq!(config.task_list.open_state_picker, vec!["t", "T"]);
        assert_eq!(config.task_list.open_comment_viewer, vec!["v", "V"]);
        assert_eq!(config.task_list.open_description_viewer, vec!["d", "D"]);
        assert_eq!(config.task_list.edit_filter, vec!["f", "F"]);

        // TreeView のデフォルト値を確認
        assert_eq!(config.tree_view.close, vec!["q", "Q", "Esc"]);
        assert_eq!(config.tree_view.down, vec!["j", "J", "Down"]);
        assert_eq!(config.tree_view.up, vec!["k", "K", "Up"]);
        assert_eq!(config.tree_view.collapse, vec!["h", "H"]);
        assert_eq!(config.tree_view.expand, vec!["l", "L"]);
        assert_eq!(config.tree_view.jump, vec!["Enter"]);

        // StatePicker のデフォルト値を確認
        assert_eq!(config.state_picker.close, vec!["q", "Q", "Esc"]);
        assert_eq!(config.state_picker.down, vec!["j", "J", "Down"]);
        assert_eq!(config.state_picker.up, vec!["k", "K", "Up"]);
        assert_eq!(config.state_picker.select, vec!["Enter"]);

        // Viewer のデフォルト値を確認
        assert_eq!(config.comment_viewer.close, vec!["q", "Q", "Esc"]);
        assert_eq!(config.comment_viewer.scroll_down, vec!["j", "J"]);
        assert_eq!(config.comment_viewer.scroll_up, vec!["k", "K"]);
        assert_eq!(config.comment_viewer.scroll_down_fast, vec!["Ctrl+d"]);
        assert_eq!(config.comment_viewer.scroll_up_fast, vec!["Ctrl+u"]);
    }

    #[test]
    fn test_deserialize_from_toml() {
        let toml_str = r#"
            [task_list]
            quit = ["q", "Q"]
            down = ["j"]
            up = ["k"]
            open_tree = ["Enter"]
            jump_to_parent = ["p"]
            refresh = ["r"]
            add_comment = ["c"]
            edit_task = ["e"]
            create_task = ["n"]
            create_subtask = ["s"]
            copy_task_id = ["y"]
            open_state_picker = ["t"]
            open_comment_viewer = ["v"]
            open_description_viewer = ["d"]
            edit_filter = ["f"]

            [tree_view]
            close = ["q"]
            down = ["j"]
            up = ["k"]
            collapse = ["h"]
            expand = ["l"]
            jump = ["Enter"]

            [state_picker]
            close = ["q"]
            down = ["j"]
            up = ["k"]
            select = ["Enter"]

            [comment_viewer]
            close = ["q"]
            scroll_down = ["j"]
            scroll_up = ["k"]
            scroll_down_fast = ["Ctrl+d"]
            scroll_up_fast = ["Ctrl+u"]

            [description_viewer]
            close = ["q"]
            scroll_down = ["j"]
            scroll_up = ["k"]
            scroll_down_fast = ["Ctrl+d"]
            scroll_up_fast = ["Ctrl+u"]
        "#;

        let config: KeyBindingsConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.task_list.quit, vec!["q", "Q"]);
        assert_eq!(config.task_list.down, vec!["j"]);
    }

    #[test]
    fn test_default_config_path() {
        let path = default_config_path();
        assert!(path.is_some());

        if let Some(path) = path {
            assert!(path.to_string_lossy().contains("git-mile"));
            assert!(path.to_string_lossy().ends_with("config.toml"));
        }
    }

    #[test]
    fn test_parse_simple_key() {
        let key = parse_key("j").unwrap();
        assert_eq!(key.code, KeyCode::Char('j'));
        assert_eq!(key.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn test_parse_uppercase_key() {
        let key = parse_key("J").unwrap();
        assert_eq!(key.code, KeyCode::Char('J'));
        assert_eq!(key.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn test_parse_special_key() {
        let key = parse_key("Enter").unwrap();
        assert_eq!(key.code, KeyCode::Enter);
        assert_eq!(key.modifiers, KeyModifiers::NONE);

        let key = parse_key("Esc").unwrap();
        assert_eq!(key.code, KeyCode::Esc);
        assert_eq!(key.modifiers, KeyModifiers::NONE);
    }

    #[test]
    fn test_parse_modified_key() {
        let key = parse_key("Ctrl+d").unwrap();
        assert_eq!(key.code, KeyCode::Char('d'));
        assert_eq!(key.modifiers, KeyModifiers::CONTROL);

        let key = parse_key("Alt+k").unwrap();
        assert_eq!(key.code, KeyCode::Char('k'));
        assert_eq!(key.modifiers, KeyModifiers::ALT);
    }

    #[test]
    fn test_parse_invalid_key() {
        assert!(parse_key("InvalidKey").is_err());
        assert!(parse_key("").is_err());
    }

    #[test]
    fn test_case_sensitivity() {
        let lower = parse_key("j").unwrap();
        let upper = parse_key("J").unwrap();

        assert_eq!(lower.code, KeyCode::Char('j'));
        assert_eq!(upper.code, KeyCode::Char('J'));
        assert_ne!(lower.code, upper.code);
    }
}
