//! Shared constants for the TUI to keep layout and timing in sync.

/// Interval in milliseconds between UI ticks/redraws.
pub const TUI_TICK_RATE_MS: u64 = 200;
/// Time-to-live in seconds for transient status messages.
pub const UI_MESSAGE_TTL_SECS: u64 = 5;
/// Width/height percentage allocated to the tree view popup.
pub const TREE_POPUP_PERCENT: u16 = 80;
/// Unit string used when indenting nested tree entries.
pub const TREE_INDENT_UNIT: &str = "  ";
/// Marker displayed for collapsed nodes in the tree view.
pub const TREE_COLLAPSED_MARKER: &str = "▶";
/// Marker displayed for expanded nodes in the tree view.
pub const TREE_EXPANDED_MARKER: &str = "▼";
/// Marker used for leaf nodes without children.
pub const TREE_LEAF_MARKER: &str = "■";
/// Highlight symbol shown beside selected list entries.
pub const TASK_LIST_HIGHLIGHT_SYMBOL: &str = "▶ ";
/// Width percentage for the state picker popup before clamping.
pub const STATE_PICKER_WIDTH_PERCENT: u16 = 40;
/// Height percentage for the state picker popup before clamping.
pub const STATE_PICKER_HEIGHT_PERCENT: u16 = 60;
/// Minimum width for the state picker popup.
pub const STATE_PICKER_MIN_WIDTH: u16 = 30;
/// Minimum height for the state picker popup.
pub const STATE_PICKER_MIN_HEIGHT: u16 = 6;
/// Width percentage for the comment viewer popup before clamping.
pub const COMMENT_VIEWER_WIDTH_PERCENT: u16 = 80;
/// Height percentage for the comment viewer popup before clamping.
pub const COMMENT_VIEWER_HEIGHT_PERCENT: u16 = 80;
/// Minimum width for the comment viewer popup.
pub const COMMENT_VIEWER_MIN_WIDTH: u16 = 40;
/// Minimum height for the comment viewer popup.
pub const COMMENT_VIEWER_MIN_HEIGHT: u16 = 10;
