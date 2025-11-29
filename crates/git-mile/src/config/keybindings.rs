//! Keybindings configuration for the TUI.

#![allow(
    clippy::cognitive_complexity,
    clippy::uninlined_format_args,
    clippy::map_unwrap_or,
    clippy::enum_glob_use,
    clippy::collapsible_if,
    clippy::unused_self
)]

use anyhow::{Context, Result, anyhow, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

macro_rules! vec_of_strings {
    ($($s:expr),* $(,)?) => {
        vec![$($s.to_string()),*]
    };
}

/// Top-level configuration for git-mile.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// TUI configuration.
    pub tui: TuiConfig,
}

/// TUI-specific configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TuiConfig {
    /// Keybindings configuration.
    pub keybindings: KeyBindingsConfig,
}

/// Keybindings configuration for all TUI views.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[allow(dead_code)]
pub fn ensure_config_dir() -> Result<PathBuf> {
    let path = default_config_path().context("Could not determine config directory")?;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create config directory: {}", parent.display()))?;
    }

    Ok(path)
}

/// Generate default configuration as TOML string.
pub fn generate_default_config_toml() -> Result<String> {
    let config = Config::default();

    // Serialize to TOML
    let toml_str = toml::to_string_pretty(&config).context("デフォルト設定のシリアライズに失敗しました")?;

    // Add header comment
    let header = r#"# git-mile Configuration
#
# This file allows you to customize git-mile.
#
# [tui.keybindings]
# Each action can have multiple key bindings.
#
# Supported key formats:
# - Single characters: "j", "k", "a", "1"
# - Special keys: "Enter", "Esc", "Tab", "Backspace", "Delete"
# - Arrow keys: "Up", "Down", "Left", "Right"
# - Navigation keys: "Home", "End", "PageUp", "PageDown"
# - Modified keys: "Ctrl+d", "Alt+k", "Shift+Up"
#
# Note: When this file exists, ALL default keybindings are disabled.
# Make sure to define all actions you need.

"#;

    Ok(format!("{}{}", header, toml_str))
}

/// Load configuration from a TOML file.
///
/// # Arguments
/// - `path`: Optional path to the config file. If `None`, uses the default path.
///
/// # Returns
/// - `Ok(Some(config))` if the file exists and was successfully parsed
/// - `Ok(None)` if the file does not exist
/// - `Err(_)` if there was an error reading or parsing the file
pub fn load_config(path: Option<&Path>) -> Result<Option<Config>> {
    let config_path = match path {
        Some(p) => p.to_path_buf(),
        None => match default_config_path() {
            Some(p) => p,
            None => return Ok(None),
        },
    };

    // ファイルが存在しない場合は None を返す
    if !config_path.exists() {
        return Ok(None);
    }

    // ファイルを読み込んでパース
    let content = std::fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config file: {}", config_path.display()))?;

    // Parse as Config directly
    let config: Config = toml::from_str(&content)
        .with_context(|| format!("Failed to parse config file: {}", config_path.display()))?;

    Ok(Some(config))
}

/// Parse a key string into a `KeyEvent`.
///
/// # Examples
/// - "j" -> `KeyCode::Char('j')`
/// - "Enter" -> `KeyCode::Enter`
/// - "Ctrl+d" -> `KeyCode::Char('d')` with CONTROL modifier
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

/// Validate the configuration.
///
/// Checks for:
/// - Key conflicts within each view
/// - Invalid key expressions
/// - Empty key bindings
pub fn validate_config_struct(config: &Config) -> Result<()> {
    validate_tui_config(&config.tui)
}

/// Validate the TUI configuration.
///
/// Checks for:
/// - Key conflicts within each view
/// - Invalid key expressions
/// - Empty key bindings
pub fn validate_tui_config(config: &TuiConfig) -> Result<()> {
    validate_keybindings_config(&config.keybindings)
}

/// Validate the keybindings configuration.
///
/// Checks for:
/// - Key conflicts within each view
/// - Invalid key expressions
/// - Empty key bindings
pub fn validate_keybindings_config(config: &KeyBindingsConfig) -> Result<()> {
    validate_non_empty_bindings(config)?;
    validate_key_expressions(config)?;
    validate_keybindings(config)?;
    Ok(())
}

/// Validate that all keybinding fields have at least one key.
fn validate_non_empty_bindings(config: &KeyBindingsConfig) -> Result<()> {
    macro_rules! check_non_empty {
        ($field:expr, $name:expr) => {
            if $field.is_empty() {
                bail!("{} must have at least one key binding", $name);
            }
        };
    }

    // TaskList
    check_non_empty!(config.task_list.quit, "task_list.quit");
    check_non_empty!(config.task_list.down, "task_list.down");
    check_non_empty!(config.task_list.up, "task_list.up");
    check_non_empty!(config.task_list.open_tree, "task_list.open_tree");
    check_non_empty!(config.task_list.jump_to_parent, "task_list.jump_to_parent");
    check_non_empty!(config.task_list.refresh, "task_list.refresh");
    check_non_empty!(config.task_list.add_comment, "task_list.add_comment");
    check_non_empty!(config.task_list.edit_task, "task_list.edit_task");
    check_non_empty!(config.task_list.create_task, "task_list.create_task");
    check_non_empty!(config.task_list.create_subtask, "task_list.create_subtask");
    check_non_empty!(config.task_list.copy_task_id, "task_list.copy_task_id");
    check_non_empty!(config.task_list.open_state_picker, "task_list.open_state_picker");
    check_non_empty!(
        config.task_list.open_comment_viewer,
        "task_list.open_comment_viewer"
    );
    check_non_empty!(
        config.task_list.open_description_viewer,
        "task_list.open_description_viewer"
    );
    check_non_empty!(config.task_list.edit_filter, "task_list.edit_filter");

    // TreeView
    check_non_empty!(config.tree_view.close, "tree_view.close");
    check_non_empty!(config.tree_view.down, "tree_view.down");
    check_non_empty!(config.tree_view.up, "tree_view.up");
    check_non_empty!(config.tree_view.collapse, "tree_view.collapse");
    check_non_empty!(config.tree_view.expand, "tree_view.expand");
    check_non_empty!(config.tree_view.jump, "tree_view.jump");

    // StatePicker
    check_non_empty!(config.state_picker.close, "state_picker.close");
    check_non_empty!(config.state_picker.down, "state_picker.down");
    check_non_empty!(config.state_picker.up, "state_picker.up");
    check_non_empty!(config.state_picker.select, "state_picker.select");

    // Viewers
    check_non_empty!(config.comment_viewer.close, "comment_viewer.close");
    check_non_empty!(config.comment_viewer.scroll_down, "comment_viewer.scroll_down");
    check_non_empty!(config.comment_viewer.scroll_up, "comment_viewer.scroll_up");
    check_non_empty!(
        config.comment_viewer.scroll_down_fast,
        "comment_viewer.scroll_down_fast"
    );
    check_non_empty!(
        config.comment_viewer.scroll_up_fast,
        "comment_viewer.scroll_up_fast"
    );

    check_non_empty!(config.description_viewer.close, "description_viewer.close");
    check_non_empty!(
        config.description_viewer.scroll_down,
        "description_viewer.scroll_down"
    );
    check_non_empty!(
        config.description_viewer.scroll_up,
        "description_viewer.scroll_up"
    );
    check_non_empty!(
        config.description_viewer.scroll_down_fast,
        "description_viewer.scroll_down_fast"
    );
    check_non_empty!(
        config.description_viewer.scroll_up_fast,
        "description_viewer.scroll_up_fast"
    );

    Ok(())
}

/// Validate that all key expressions can be parsed.
fn validate_key_expressions(config: &KeyBindingsConfig) -> Result<()> {
    macro_rules! validate_keys {
        ($field:expr, $name:expr) => {
            for key in $field {
                parse_key(key).with_context(|| format!("Invalid key '{}' in {}", key, $name))?;
            }
        };
    }

    // TaskList
    validate_keys!(&config.task_list.quit, "task_list.quit");
    validate_keys!(&config.task_list.down, "task_list.down");
    validate_keys!(&config.task_list.up, "task_list.up");
    validate_keys!(&config.task_list.open_tree, "task_list.open_tree");
    validate_keys!(&config.task_list.jump_to_parent, "task_list.jump_to_parent");
    validate_keys!(&config.task_list.refresh, "task_list.refresh");
    validate_keys!(&config.task_list.add_comment, "task_list.add_comment");
    validate_keys!(&config.task_list.edit_task, "task_list.edit_task");
    validate_keys!(&config.task_list.create_task, "task_list.create_task");
    validate_keys!(&config.task_list.create_subtask, "task_list.create_subtask");
    validate_keys!(&config.task_list.copy_task_id, "task_list.copy_task_id");
    validate_keys!(&config.task_list.open_state_picker, "task_list.open_state_picker");
    validate_keys!(
        &config.task_list.open_comment_viewer,
        "task_list.open_comment_viewer"
    );
    validate_keys!(
        &config.task_list.open_description_viewer,
        "task_list.open_description_viewer"
    );
    validate_keys!(&config.task_list.edit_filter, "task_list.edit_filter");

    // TreeView
    validate_keys!(&config.tree_view.close, "tree_view.close");
    validate_keys!(&config.tree_view.down, "tree_view.down");
    validate_keys!(&config.tree_view.up, "tree_view.up");
    validate_keys!(&config.tree_view.collapse, "tree_view.collapse");
    validate_keys!(&config.tree_view.expand, "tree_view.expand");
    validate_keys!(&config.tree_view.jump, "tree_view.jump");

    // StatePicker
    validate_keys!(&config.state_picker.close, "state_picker.close");
    validate_keys!(&config.state_picker.down, "state_picker.down");
    validate_keys!(&config.state_picker.up, "state_picker.up");
    validate_keys!(&config.state_picker.select, "state_picker.select");

    // Viewers
    validate_keys!(&config.comment_viewer.close, "comment_viewer.close");
    validate_keys!(&config.comment_viewer.scroll_down, "comment_viewer.scroll_down");
    validate_keys!(&config.comment_viewer.scroll_up, "comment_viewer.scroll_up");
    validate_keys!(
        &config.comment_viewer.scroll_down_fast,
        "comment_viewer.scroll_down_fast"
    );
    validate_keys!(
        &config.comment_viewer.scroll_up_fast,
        "comment_viewer.scroll_up_fast"
    );

    validate_keys!(&config.description_viewer.close, "description_viewer.close");
    validate_keys!(
        &config.description_viewer.scroll_down,
        "description_viewer.scroll_down"
    );
    validate_keys!(
        &config.description_viewer.scroll_up,
        "description_viewer.scroll_up"
    );
    validate_keys!(
        &config.description_viewer.scroll_down_fast,
        "description_viewer.scroll_down_fast"
    );
    validate_keys!(
        &config.description_viewer.scroll_up_fast,
        "description_viewer.scroll_up_fast"
    );

    Ok(())
}

/// Validate that there are no key conflicts within each view.
fn validate_keybindings(config: &KeyBindingsConfig) -> Result<()> {
    validate_view_keybindings("task_list", collect_task_list_bindings(config))?;
    validate_view_keybindings("tree_view", collect_tree_view_bindings(config))?;
    validate_view_keybindings("state_picker", collect_state_picker_bindings(config))?;
    validate_view_keybindings("comment_viewer", collect_comment_viewer_bindings(config))?;
    validate_view_keybindings("description_viewer", collect_description_viewer_bindings(config))?;
    Ok(())
}

fn validate_view_keybindings(view_name: &str, bindings: HashMap<String, Vec<String>>) -> Result<()> {
    let mut key_to_actions: HashMap<String, Vec<String>> = HashMap::new();

    for (action, keys) in bindings {
        for key in keys {
            key_to_actions
                .entry(key.clone())
                .or_default()
                .push(action.clone());
        }
    }

    // 衝突をチェック
    for (key, actions) in key_to_actions {
        if actions.len() > 1 {
            bail!(
                "Key '{}' is bound to multiple actions in {}: {:?}",
                key,
                view_name,
                actions
            );
        }
    }

    Ok(())
}

fn collect_task_list_bindings(config: &KeyBindingsConfig) -> HashMap<String, Vec<String>> {
    let mut bindings = HashMap::new();
    bindings.insert("quit".to_string(), config.task_list.quit.clone());
    bindings.insert("down".to_string(), config.task_list.down.clone());
    bindings.insert("up".to_string(), config.task_list.up.clone());
    bindings.insert("open_tree".to_string(), config.task_list.open_tree.clone());
    bindings.insert(
        "jump_to_parent".to_string(),
        config.task_list.jump_to_parent.clone(),
    );
    bindings.insert("refresh".to_string(), config.task_list.refresh.clone());
    bindings.insert("add_comment".to_string(), config.task_list.add_comment.clone());
    bindings.insert("edit_task".to_string(), config.task_list.edit_task.clone());
    bindings.insert("create_task".to_string(), config.task_list.create_task.clone());
    bindings.insert(
        "create_subtask".to_string(),
        config.task_list.create_subtask.clone(),
    );
    bindings.insert("copy_task_id".to_string(), config.task_list.copy_task_id.clone());
    bindings.insert(
        "open_state_picker".to_string(),
        config.task_list.open_state_picker.clone(),
    );
    bindings.insert(
        "open_comment_viewer".to_string(),
        config.task_list.open_comment_viewer.clone(),
    );
    bindings.insert(
        "open_description_viewer".to_string(),
        config.task_list.open_description_viewer.clone(),
    );
    bindings.insert("edit_filter".to_string(), config.task_list.edit_filter.clone());
    bindings
}

fn collect_tree_view_bindings(config: &KeyBindingsConfig) -> HashMap<String, Vec<String>> {
    let mut bindings = HashMap::new();
    bindings.insert("close".to_string(), config.tree_view.close.clone());
    bindings.insert("down".to_string(), config.tree_view.down.clone());
    bindings.insert("up".to_string(), config.tree_view.up.clone());
    bindings.insert("collapse".to_string(), config.tree_view.collapse.clone());
    bindings.insert("expand".to_string(), config.tree_view.expand.clone());
    bindings.insert("jump".to_string(), config.tree_view.jump.clone());
    bindings
}

fn collect_state_picker_bindings(config: &KeyBindingsConfig) -> HashMap<String, Vec<String>> {
    let mut bindings = HashMap::new();
    bindings.insert("close".to_string(), config.state_picker.close.clone());
    bindings.insert("down".to_string(), config.state_picker.down.clone());
    bindings.insert("up".to_string(), config.state_picker.up.clone());
    bindings.insert("select".to_string(), config.state_picker.select.clone());
    bindings
}

fn collect_comment_viewer_bindings(config: &KeyBindingsConfig) -> HashMap<String, Vec<String>> {
    let mut bindings = HashMap::new();
    bindings.insert("close".to_string(), config.comment_viewer.close.clone());
    bindings.insert(
        "scroll_down".to_string(),
        config.comment_viewer.scroll_down.clone(),
    );
    bindings.insert("scroll_up".to_string(), config.comment_viewer.scroll_up.clone());
    bindings.insert(
        "scroll_down_fast".to_string(),
        config.comment_viewer.scroll_down_fast.clone(),
    );
    bindings.insert(
        "scroll_up_fast".to_string(),
        config.comment_viewer.scroll_up_fast.clone(),
    );
    bindings
}

fn collect_description_viewer_bindings(config: &KeyBindingsConfig) -> HashMap<String, Vec<String>> {
    let mut bindings = HashMap::new();
    bindings.insert("close".to_string(), config.description_viewer.close.clone());
    bindings.insert(
        "scroll_down".to_string(),
        config.description_viewer.scroll_down.clone(),
    );
    bindings.insert(
        "scroll_up".to_string(),
        config.description_viewer.scroll_up.clone(),
    );
    bindings.insert(
        "scroll_down_fast".to_string(),
        config.description_viewer.scroll_down_fast.clone(),
    );
    bindings.insert(
        "scroll_up_fast".to_string(),
        config.description_viewer.scroll_up_fast.clone(),
    );
    bindings
}

/// View type for keybinding context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ViewType {
    /// Task list view.
    TaskList,
    /// Tree view.
    TreeView,
    /// State picker.
    StatePicker,
    /// Comment viewer.
    CommentViewer,
    /// Description viewer.
    DescriptionViewer,
}

/// Action that can be performed in a view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // Common actions
    /// Quit or close.
    Quit,
    /// Move down.
    Down,
    /// Move up.
    Up,
    /// Close (for popups).
    Close,

    // TaskList specific
    /// Open tree view.
    OpenTree,
    /// Jump to parent task.
    JumpToParent,
    /// Refresh task list.
    Refresh,
    /// Add comment.
    AddComment,
    /// Edit task.
    EditTask,
    /// Create new task.
    CreateTask,
    /// Create subtask.
    CreateSubtask,
    /// Copy task ID.
    CopyTaskId,
    /// Open state picker.
    OpenStatePicker,
    /// Open comment viewer.
    OpenCommentViewer,
    /// Open description viewer.
    OpenDescriptionViewer,
    /// Edit filter.
    EditFilter,

    // TreeView specific
    /// Collapse tree node.
    Collapse,
    /// Expand tree node.
    Expand,
    /// Jump to selected task.
    Jump,

    // StatePicker specific
    /// Select state.
    Select,

    // Viewer specific
    /// Scroll down.
    ScrollDown,
    /// Scroll up.
    ScrollUp,
    /// Scroll down fast.
    ScrollDownFast,
    /// Scroll up fast.
    ScrollUpFast,
}

impl KeyBindingsConfig {
    /// Generate help text for a specific view.
    ///
    /// # Arguments
    /// * `view` - The view type to generate help for
    ///
    /// # Returns
    /// A formatted help string showing the keybindings for the view
    pub fn generate_help_text(&self, view: ViewType) -> String {
        match view {
            ViewType::TaskList => self.generate_task_list_help(),
            ViewType::TreeView => self.generate_tree_view_help(),
            ViewType::StatePicker => self.generate_state_picker_help(),
            ViewType::CommentViewer | ViewType::DescriptionViewer => self.generate_viewer_help(),
        }
    }

    fn generate_task_list_help(&self) -> String {
        format!(
            "{}:移動 {}:ツリー {}:新規 {}:子タスク {}:編集 {}:コメント {}:コメント表示 {}:説明表示 {}:再読込 {}:親へ {}:IDコピー {}:状態 {}:フィルタ {}:終了",
            self.format_key_pair(&self.task_list.down, &self.task_list.up),
            self.format_first_key(&self.task_list.open_tree),
            self.format_first_key(&self.task_list.create_task),
            self.format_first_key(&self.task_list.create_subtask),
            self.format_first_key(&self.task_list.edit_task),
            self.format_first_key(&self.task_list.add_comment),
            self.format_first_key(&self.task_list.open_comment_viewer),
            self.format_first_key(&self.task_list.open_description_viewer),
            self.format_first_key(&self.task_list.refresh),
            self.format_first_key(&self.task_list.jump_to_parent),
            self.format_first_key(&self.task_list.copy_task_id),
            self.format_first_key(&self.task_list.open_state_picker),
            self.format_first_key(&self.task_list.edit_filter),
            self.format_first_key(&self.task_list.quit),
        )
    }

    fn generate_tree_view_help(&self) -> String {
        format!(
            "{}:移動 {}:閉じる {}:開く {}:ジャンプ {}:閉じる",
            self.format_key_pair(&self.tree_view.down, &self.tree_view.up),
            self.format_first_key(&self.tree_view.collapse),
            self.format_first_key(&self.tree_view.expand),
            self.format_first_key(&self.tree_view.jump),
            self.format_first_key(&self.tree_view.close),
        )
    }

    fn generate_state_picker_help(&self) -> String {
        format!(
            "{}:移動 {}:決定 {}:キャンセル",
            self.format_key_pair(&self.state_picker.down, &self.state_picker.up),
            self.format_first_key(&self.state_picker.select),
            self.format_first_key(&self.state_picker.close),
        )
    }

    fn generate_viewer_help(&self) -> String {
        format!(
            "{}:スクロール {}/{}:半画面スクロール {}:閉じる",
            self.format_key_pair(&self.comment_viewer.scroll_down, &self.comment_viewer.scroll_up),
            self.format_first_key(&self.comment_viewer.scroll_down_fast),
            self.format_first_key(&self.comment_viewer.scroll_up_fast),
            self.format_first_key(&self.comment_viewer.close),
        )
    }

    /// Format the first key of a key binding list for display.
    fn format_first_key(&self, keys: &[String]) -> String {
        keys.first()
            .map(|k| self.format_key_display(k))
            .unwrap_or_else(|| "?".to_string())
    }

    /// Format two keys as a pair (e.g., "j/k" for down/up).
    fn format_key_pair(&self, down: &[String], up: &[String]) -> String {
        format!("{}/{}", self.format_first_key(down), self.format_first_key(up))
    }

    /// Format a key for display, converting special keys to readable symbols.
    fn format_key_display(&self, key: &str) -> String {
        match key {
            "Enter" => "↵".to_string(),
            "Esc" => "Esc".to_string(),
            "Tab" => "Tab".to_string(),
            "Backspace" => "BS".to_string(),
            "Delete" => "Del".to_string(),
            "Up" => "↑".to_string(),
            "Down" => "↓".to_string(),
            "Left" => "←".to_string(),
            "Right" => "→".to_string(),
            "Home" => "Home".to_string(),
            "End" => "End".to_string(),
            "PageUp" => "PgUp".to_string(),
            "PageDown" => "PgDn".to_string(),
            "Ctrl+d" => "Ctrl-d".to_string(),
            "Ctrl+u" => "Ctrl-u".to_string(),
            other if other.starts_with("Ctrl+") => other.replace('+', "-"),
            other if other.starts_with("Alt+") => other.replace('+', "-"),
            other => other.to_string(),
        }
    }

    /// Check if a key event matches a configured action in a view.
    ///
    /// # Arguments
    /// * `view` - The view type
    /// * `action` - The action to check
    /// * `key` - The key event to match
    ///
    /// # Returns
    /// `true` if the key matches any configured binding for this action
    pub fn matches(&self, view: ViewType, action: Action, key: &KeyEvent) -> bool {
        let keys = self.get_keys(view, action);

        for key_str in keys {
            if let Ok(expected) = parse_key(key_str) {
                if Self::key_event_matches(&expected, key) {
                    return true;
                }
            }
        }

        false
    }

    fn key_event_matches(expected: &KeyEvent, actual: &KeyEvent) -> bool {
        expected.code == actual.code && expected.modifiers == actual.modifiers
    }

    fn get_keys(&self, view: ViewType, action: Action) -> &[String] {
        use Action::*;
        use ViewType::*;

        match (view, action) {
            // TaskList
            (TaskList, Quit) => &self.task_list.quit,
            (TaskList, Down) => &self.task_list.down,
            (TaskList, Up) => &self.task_list.up,
            (TaskList, OpenTree) => &self.task_list.open_tree,
            (TaskList, JumpToParent) => &self.task_list.jump_to_parent,
            (TaskList, Refresh) => &self.task_list.refresh,
            (TaskList, AddComment) => &self.task_list.add_comment,
            (TaskList, EditTask) => &self.task_list.edit_task,
            (TaskList, CreateTask) => &self.task_list.create_task,
            (TaskList, CreateSubtask) => &self.task_list.create_subtask,
            (TaskList, CopyTaskId) => &self.task_list.copy_task_id,
            (TaskList, OpenStatePicker) => &self.task_list.open_state_picker,
            (TaskList, OpenCommentViewer) => &self.task_list.open_comment_viewer,
            (TaskList, OpenDescriptionViewer) => &self.task_list.open_description_viewer,
            (TaskList, EditFilter) => &self.task_list.edit_filter,

            // TreeView
            (TreeView, Close) => &self.tree_view.close,
            (TreeView, Down) => &self.tree_view.down,
            (TreeView, Up) => &self.tree_view.up,
            (TreeView, Collapse) => &self.tree_view.collapse,
            (TreeView, Expand) => &self.tree_view.expand,
            (TreeView, Jump) => &self.tree_view.jump,

            // StatePicker
            (StatePicker, Close) => &self.state_picker.close,
            (StatePicker, Down) => &self.state_picker.down,
            (StatePicker, Up) => &self.state_picker.up,
            (StatePicker, Select) => &self.state_picker.select,

            // CommentViewer
            (CommentViewer, Close) => &self.comment_viewer.close,
            (CommentViewer, ScrollDown) => &self.comment_viewer.scroll_down,
            (CommentViewer, ScrollUp) => &self.comment_viewer.scroll_up,
            (CommentViewer, ScrollDownFast) => &self.comment_viewer.scroll_down_fast,
            (CommentViewer, ScrollUpFast) => &self.comment_viewer.scroll_up_fast,

            // DescriptionViewer
            (DescriptionViewer, Close) => &self.description_viewer.close,
            (DescriptionViewer, ScrollDown) => &self.description_viewer.scroll_down,
            (DescriptionViewer, ScrollUp) => &self.description_viewer.scroll_up,
            (DescriptionViewer, ScrollDownFast) => &self.description_viewer.scroll_down_fast,
            (DescriptionViewer, ScrollUpFast) => &self.description_viewer.scroll_up_fast,

            // Invalid combinations
            _ => &[],
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(
        clippy::unwrap_used,
        clippy::useless_vec,
        clippy::cognitive_complexity,
        clippy::uninlined_format_args,
        clippy::single_char_pattern
    )]

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

    #[test]
    fn test_default_help_text_task_list() {
        let config = KeyBindingsConfig::default();
        let help = config.generate_help_text(ViewType::TaskList);

        // デフォルトのヘルプテキストが生成されること
        assert!(help.contains("移動"));
        assert!(help.contains("終了"));
        assert!(help.contains("新規"));
        assert!(help.contains("コメント"));
    }

    #[test]
    fn test_default_help_text_tree_view() {
        let config = KeyBindingsConfig::default();
        let help = config.generate_help_text(ViewType::TreeView);

        assert!(help.contains("移動"));
        assert!(help.contains("閉じる"));
        assert!(help.contains("開く"));
        assert!(help.contains("ジャンプ"));
    }

    #[test]
    fn test_default_help_text_state_picker() {
        let config = KeyBindingsConfig::default();
        let help = config.generate_help_text(ViewType::StatePicker);

        assert!(help.contains("移動"));
        assert!(help.contains("決定"));
        assert!(help.contains("キャンセル"));
    }

    #[test]
    fn test_default_help_text_viewer() {
        let config = KeyBindingsConfig::default();
        let help = config.generate_help_text(ViewType::CommentViewer);

        assert!(help.contains("スクロール"));
        assert!(help.contains("半画面スクロール"));
        assert!(help.contains("閉じる"));
    }

    #[test]
    fn test_custom_help_text() {
        let mut config = KeyBindingsConfig::default();
        config.task_list.quit = vec!["x".to_string()];
        config.task_list.down = vec!["n".to_string()];
        config.task_list.up = vec!["p".to_string()];

        let help = config.generate_help_text(ViewType::TaskList);

        // カスタムキーが反映されること
        assert!(help.contains("n/p:移動"));
        assert!(help.contains("x:終了"));
    }

    #[test]
    fn test_format_special_keys() {
        let config = KeyBindingsConfig::default();

        assert_eq!(config.format_key_display("Enter"), "↵");
        assert_eq!(config.format_key_display("Esc"), "Esc");
        assert_eq!(config.format_key_display("Ctrl+d"), "Ctrl-d");
        assert_eq!(config.format_key_display("Ctrl+u"), "Ctrl-u");
        assert_eq!(config.format_key_display("Up"), "↑");
        assert_eq!(config.format_key_display("Down"), "↓");
        assert_eq!(config.format_key_display("Left"), "←");
        assert_eq!(config.format_key_display("Right"), "→");
    }

    #[test]
    fn test_format_first_key() {
        let config = KeyBindingsConfig::default();

        assert_eq!(
            config.format_first_key(&vec!["j".to_string(), "J".to_string()]),
            "j"
        );
        assert_eq!(config.format_first_key(&vec!["Enter".to_string()]), "↵");
        assert_eq!(config.format_first_key(&vec![]), "?");
    }

    #[test]
    fn test_format_key_pair() {
        let config = KeyBindingsConfig::default();

        assert_eq!(
            config.format_key_pair(
                &vec!["j".to_string(), "J".to_string()],
                &vec!["k".to_string(), "K".to_string()]
            ),
            "j/k"
        );
        assert_eq!(
            config.format_key_pair(&vec!["Down".to_string()], &vec!["Up".to_string()]),
            "↓/↑"
        );
    }

    // Additional deserialization tests
    #[test]
    fn test_deserialize_partial_config_fails() {
        let toml = r#"
            [task_list]
            quit = ["q"]
            # down フィールドが欠けている
        "#;

        let result: Result<KeyBindingsConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_invalid_value() {
        let toml = r#"
            [task_list]
            quit = "not_an_array"
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
        "#;

        let result: Result<KeyBindingsConfig, _> = toml::from_str(toml);
        assert!(result.is_err());
    }

    #[test]
    fn test_serialize_deserialize_roundtrip() {
        let config = KeyBindingsConfig::default();
        let toml_str = toml::to_string(&config).unwrap();
        let deserialized: KeyBindingsConfig = toml::from_str(&toml_str).unwrap();

        assert_eq!(config.task_list.quit, deserialized.task_list.quit);
        assert_eq!(config.task_list.down, deserialized.task_list.down);
        assert_eq!(config.tree_view.close, deserialized.tree_view.close);
        assert_eq!(config.state_picker.select, deserialized.state_picker.select);
        assert_eq!(config.comment_viewer.close, deserialized.comment_viewer.close);
    }

    // Key parsing tests
    #[test]
    fn test_parse_all_special_keys() {
        let special_keys = vec![
            "Enter",
            "Esc",
            "Tab",
            "Backspace",
            "Delete",
            "Up",
            "Down",
            "Left",
            "Right",
            "Home",
            "End",
            "PageUp",
            "PageDown",
            "Insert",
        ];

        for key_str in special_keys {
            let result = parse_key(key_str);
            assert!(result.is_ok(), "Failed to parse: {}", key_str);
        }
    }

    #[test]
    fn test_parse_modified_keys_comprehensive() {
        let test_cases = vec![
            ("Ctrl+d", KeyModifiers::CONTROL, KeyCode::Char('d')),
            ("Alt+k", KeyModifiers::ALT, KeyCode::Char('k')),
            ("Shift+Up", KeyModifiers::SHIFT, KeyCode::Up),
            ("Control+Enter", KeyModifiers::CONTROL, KeyCode::Enter),
        ];

        for (key_str, expected_mod, expected_code) in test_cases {
            let key = parse_key(key_str).unwrap();
            assert_eq!(key.modifiers, expected_mod, "Modifier mismatch for {}", key_str);
            assert_eq!(key.code, expected_code, "Code mismatch for {}", key_str);
        }
    }

    #[test]
    fn test_parse_arrow_keys() {
        let arrows = vec![
            ("Up", KeyCode::Up),
            ("Down", KeyCode::Down),
            ("Left", KeyCode::Left),
            ("Right", KeyCode::Right),
        ];

        for (key_str, expected_code) in arrows {
            let key = parse_key(key_str).unwrap();
            assert_eq!(key.code, expected_code);
            assert_eq!(key.modifiers, KeyModifiers::NONE);
        }
    }

    // Key matching tests
    #[test]
    fn test_matches_single_key() {
        let mut config = KeyBindingsConfig::default();
        config.task_list.quit = vec!["q".to_string()];

        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(config.matches(ViewType::TaskList, Action::Quit, &key));

        let other_key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(!config.matches(ViewType::TaskList, Action::Quit, &other_key));
    }

    #[test]
    fn test_matches_multiple_keys() {
        let config = KeyBindingsConfig::default();

        // デフォルトでは quit は ["q", "Q", "Esc"]
        let q_key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        let big_q_key = KeyEvent::new(KeyCode::Char('Q'), KeyModifiers::NONE);
        let esc_key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);

        assert!(config.matches(ViewType::TaskList, Action::Quit, &q_key));
        assert!(config.matches(ViewType::TaskList, Action::Quit, &big_q_key));
        assert!(config.matches(ViewType::TaskList, Action::Quit, &esc_key));
    }

    #[test]
    fn test_matches_modified_keys() {
        let config = KeyBindingsConfig::default();

        // デフォルトでは scroll_down_fast は ["Ctrl+d"]
        let ctrl_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL);
        assert!(config.matches(ViewType::CommentViewer, Action::ScrollDownFast, &ctrl_d));

        // Ctrl なしの 'd' は一致しない
        let plain_d = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(!config.matches(ViewType::CommentViewer, Action::ScrollDownFast, &plain_d));
    }

    #[test]
    fn test_matches_wrong_view() {
        let config = KeyBindingsConfig::default();

        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);

        // TaskList の quit キーは TreeView の Close とは異なるアクション
        assert!(config.matches(ViewType::TaskList, Action::Quit, &key));
        // 同じキーでも異なるビューとアクションの組み合わせ
        assert!(config.matches(ViewType::TreeView, Action::Close, &key));
    }

    // Validation tests
    #[test]
    fn test_validate_default_config() {
        let config = KeyBindingsConfig::default();
        let result = validate_keybindings_config(&config);
        assert!(result.is_ok(), "Default config should be valid");
    }

    #[test]
    fn test_detect_key_conflict_in_same_view() {
        let mut config = KeyBindingsConfig::default();
        config.task_list.quit = vec!["j".to_string()];
        config.task_list.down = vec!["j".to_string()]; // 衝突

        let result = validate_keybindings(&config);
        assert!(result.is_err());

        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("multiple actions") || err_msg.contains("j"));
    }

    #[test]
    fn test_no_conflict_across_views() {
        let mut config = KeyBindingsConfig::default();
        config.task_list.quit = vec!["q".to_string()];
        config.tree_view.close = vec!["q".to_string()]; // 異なるビューなので OK

        let result = validate_keybindings(&config);
        assert!(result.is_ok(), "Same key in different views should not conflict");
    }

    #[test]
    fn test_empty_binding_validation() {
        let mut config = KeyBindingsConfig::default();
        config.task_list.quit = vec![];

        let result = validate_non_empty_bindings(&config);
        assert!(result.is_err());

        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("task_list.quit"));
    }

    #[test]
    fn test_invalid_key_expression() {
        let mut config = KeyBindingsConfig::default();
        config.task_list.quit = vec!["InvalidKey123".to_string()];

        let result = validate_key_expressions(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_all_keys_parseable() {
        let config = KeyBindingsConfig::default();
        let result = validate_key_expressions(&config);
        assert!(result.is_ok(), "All default keys should be parseable");
    }

    #[test]
    fn test_config_with_multiple_conflicts() {
        let mut config = KeyBindingsConfig::default();
        config.task_list.quit = vec!["x".to_string()];
        config.task_list.down = vec!["x".to_string()];
        config.task_list.up = vec!["x".to_string()];

        let result = validate_keybindings(&config);
        assert!(result.is_err(), "Multiple conflicts should be detected");
    }

    // File loading tests
    #[test]
    fn test_load_nonexistent_config() {
        // 存在しないパスからの読み込み
        let result =
            load_config(Some(std::path::Path::new("/nonexistent/path/config.toml"))).unwrap();

        // ファイルが存在しない場合は None が返される
        assert!(result.is_none());
    }

    #[test]
    fn test_load_valid_custom_config_from_file() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();

        // カスタム設定を作成（全フィールド定義）
        let custom_config = r#"
[tui.keybindings.task_list]
quit = ["x", "X"]
down = ["n"]
up = ["p"]
open_tree = ["Enter"]
jump_to_parent = ["g"]
refresh = ["r"]
add_comment = ["c"]
edit_task = ["e"]
create_task = ["t"]
create_subtask = ["s"]
copy_task_id = ["y"]
open_state_picker = ["w"]
open_comment_viewer = ["v"]
open_description_viewer = ["d"]
edit_filter = ["f"]

[tui.keybindings.tree_view]
close = ["q"]
down = ["j"]
up = ["k"]
collapse = ["h"]
expand = ["l"]
jump = ["Enter"]

[tui.keybindings.state_picker]
close = ["q"]
down = ["j"]
up = ["k"]
select = ["Enter"]

[tui.keybindings.comment_viewer]
close = ["q"]
scroll_down = ["j"]
scroll_up = ["k"]
scroll_down_fast = ["Ctrl+d"]
scroll_up_fast = ["Ctrl+u"]

[tui.keybindings.description_viewer]
close = ["q"]
scroll_down = ["j"]
scroll_up = ["k"]
scroll_down_fast = ["Ctrl+d"]
scroll_up_fast = ["Ctrl+u"]
"#;

        temp_file.write_all(custom_config.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        // 設定を読み込んで検証
        let config = load_config(Some(temp_file.path())).unwrap().unwrap();

        assert_eq!(config.tui.keybindings.task_list.quit, vec!["x", "X"]);
        assert_eq!(config.tui.keybindings.task_list.down, vec!["n"]);
        assert_eq!(config.tui.keybindings.task_list.up, vec!["p"]);
        assert_eq!(config.tui.keybindings.task_list.jump_to_parent, vec!["g"]);
    }

    #[test]
    fn test_load_invalid_toml_syntax() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();

        // 不正な TOML 構文
        let invalid_toml = r#"
[task_list
quit = ["q"]
"#;

        temp_file.write_all(invalid_toml.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        // パースエラーが返される
        let result = load_config(Some(temp_file.path()));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_missing_required_fields() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();

        // 必須フィールドが欠けている
        let incomplete_config = r#"
[task_list]
quit = ["q"]
down = ["j"]
"#;

        temp_file.write_all(incomplete_config.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        // デシリアライズエラーが返される
        let result = load_config(Some(temp_file.path()));
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_default_toml_is_valid() {
        let toml = generate_default_config_toml().unwrap();

        eprintln!("=== Generated TOML ===\n{}\n=== End ===", toml);

        // ヘッダーコメントが含まれている
        assert!(toml.contains("git-mile Configuration"), "Missing header");
        assert!(
            toml.contains("Supported key formats"),
            "Missing formats description"
        );

        // セクションが含まれている
        assert!(
            toml.contains("[tui.keybindings.task_list]"),
            "Missing task_list section"
        );
        assert!(
            toml.contains("[tui.keybindings.tree_view]"),
            "Missing tree_view section"
        );
        assert!(
            toml.contains("[tui.keybindings.state_picker]"),
            "Missing state_picker section"
        );
        assert!(
            toml.contains("[tui.keybindings.comment_viewer]"),
            "Missing comment_viewer section"
        );
        assert!(
            toml.contains("[tui.keybindings.description_viewer]"),
            "Missing description_viewer section"
        );

        // 生成された TOML がパース可能であること
        let parsed: Config = toml::from_str(&toml).unwrap_or_else(|e| {
            eprintln!("Failed to parse TOML: {:?}", e);
            panic!("TOML parse error");
        });
        assert_eq!(parsed.tui.keybindings.task_list.quit, vec!["q", "Q", "Esc"]);

        // バリデーションが通ること
        assert!(validate_config_struct(&parsed).is_ok());
    }

    #[test]
    fn test_config_file_roundtrip() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();

        // デフォルト設定を生成してファイルに書き込む
        let toml = generate_default_config_toml().unwrap();
        temp_file.write_all(toml.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        // ファイルから読み込む
        let loaded_config = load_config(Some(temp_file.path())).unwrap().unwrap();

        // デフォルト設定と一致すること
        let default_config = Config::default();
        assert_eq!(
            loaded_config.tui.keybindings.task_list.quit,
            default_config.tui.keybindings.task_list.quit
        );
        assert_eq!(
            loaded_config.tui.keybindings.task_list.down,
            default_config.tui.keybindings.task_list.down
        );
        assert_eq!(
            loaded_config.tui.keybindings.tree_view.close,
            default_config.tui.keybindings.tree_view.close
        );
        assert_eq!(
            loaded_config.tui.keybindings.state_picker.select,
            default_config.tui.keybindings.state_picker.select
        );
    }

    #[test]
    fn test_validate_config_from_file_with_conflicts() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();

        // キーの衝突がある設定
        let conflicting_config = r#"
[tui.keybindings.task_list]
quit = ["j"]
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

[tui.keybindings.tree_view]
close = ["q"]
down = ["j"]
up = ["k"]
collapse = ["h"]
expand = ["l"]
jump = ["Enter"]

[tui.keybindings.state_picker]
close = ["q"]
down = ["j"]
up = ["k"]
select = ["Enter"]

[tui.keybindings.comment_viewer]
close = ["q"]
scroll_down = ["j"]
scroll_up = ["k"]
scroll_down_fast = ["Ctrl+d"]
scroll_up_fast = ["Ctrl+u"]

[tui.keybindings.description_viewer]
close = ["q"]
scroll_down = ["j"]
scroll_up = ["k"]
scroll_down_fast = ["Ctrl+d"]
scroll_up_fast = ["Ctrl+u"]
"#;

        temp_file.write_all(conflicting_config.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        // 設定は読み込めるが、バリデーションで失敗する
        let config = load_config(Some(temp_file.path())).unwrap().unwrap();
        let result = validate_config_struct(&config);

        assert!(result.is_err());
    }

    #[test]
    fn test_load_config_from_file() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut temp_file = NamedTempFile::new().unwrap();

        // 設定ファイル
        let config_content = r#"
[tui.keybindings.task_list]
quit = ["q"]
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

[tui.keybindings.tree_view]
close = ["q"]
down = ["j"]
up = ["k"]
collapse = ["h"]
expand = ["l"]
jump = ["Enter"]

[tui.keybindings.state_picker]
close = ["q"]
down = ["j"]
up = ["k"]
select = ["Enter"]

[tui.keybindings.comment_viewer]
close = ["q"]
scroll_down = ["j"]
scroll_up = ["k"]
scroll_down_fast = ["Ctrl+d"]
scroll_up_fast = ["Ctrl+u"]

[tui.keybindings.description_viewer]
close = ["q"]
scroll_down = ["j"]
scroll_up = ["k"]
scroll_down_fast = ["Ctrl+d"]
scroll_up_fast = ["Ctrl+u"]
"#;

        temp_file.write_all(config_content.as_bytes()).unwrap();
        temp_file.flush().unwrap();

        // 設定を読み込む
        let config = load_config(Some(temp_file.path())).unwrap().unwrap();

        assert_eq!(config.tui.keybindings.task_list.quit, vec!["q"]);
        assert_eq!(config.tui.keybindings.task_list.down, vec!["j"]);

        // バリデーションが通ること
        assert!(validate_config_struct(&config).is_ok());
    }
}
