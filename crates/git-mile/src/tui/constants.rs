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
/// Width percentage for the log viewer popup before clamping.
pub const LOG_VIEWER_WIDTH_PERCENT: u16 = 80;
/// Height percentage for the log viewer popup before clamping.
pub const LOG_VIEWER_HEIGHT_PERCENT: u16 = 80;
/// Minimum width for the log viewer popup.
pub const LOG_VIEWER_MIN_WIDTH: u16 = 50;
/// Minimum height for the log viewer popup.
pub const LOG_VIEWER_MIN_HEIGHT: u16 = 12;
/// Height reserved for the breadcrumb row showing ancestors.
pub const DETAIL_BREADCRUMB_HEIGHT: u16 = 3;
/// Minimum height dedicated to the primary detail section.
pub const DETAIL_SECTION_MIN_HEIGHT: u16 = 5;
/// Maximum number of child rows rendered before scrolling is required.
pub const DETAIL_CHILD_LIST_MAX_ROWS: u16 = 10;
/// Additional padding rows to give the child list breathing room.
pub const DETAIL_CHILD_LIST_PADDING_ROWS: u16 = 2;
/// Maximum character width for ancestor titles rendered in the breadcrumb.
pub const DETAIL_BREADCRUMB_TITLE_MAX_CHARS: usize = 20;
/// Maximum character width for parent titles listed in the metadata section.
pub const DETAIL_PARENT_TITLE_MAX_CHARS: usize = 15;
/// Marker displayed at the start of each child entry in the detail pane.
pub const DETAIL_CHILD_ENTRY_MARKER: &str = "▸";
