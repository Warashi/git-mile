//! Parameter definitions for MCP tools.

use git_mile_core::StateKind;
use git_mile_core::event::Actor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Parameters for creating a new task.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateTaskParams {
    /// Human-readable title for the task.
    pub title: String,
    /// Optional workflow state label. Falls back to `workflow.default_state` when omitted.
    #[serde(default)]
    pub state: Option<String>,
    /// Labels to attach to the task.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Initial assignees.
    #[serde(default)]
    pub assignees: Vec<String>,
    /// Optional description in Markdown.
    #[serde(default)]
    pub description: Option<String>,
    /// Parent task IDs to link this task to.
    #[serde(default)]
    pub parents: Vec<String>,
    /// Optional actor display name provided via MCP params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_name: Option<String>,
    /// Optional actor email provided via MCP params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_email: Option<String>,
}

/// Parameters for updating an existing task.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateTaskParams {
    /// Task ID to update.
    pub task_id: String,
    /// New title (if provided, overwrites the current title).
    #[serde(default)]
    pub title: Option<String>,
    /// New description (if provided, overwrites the current description).
    #[serde(default)]
    pub description: Option<String>,
    /// New state (if provided, sets the workflow state).
    #[serde(default)]
    pub state: Option<String>,
    /// If true, clears the workflow state.
    #[serde(default)]
    pub clear_state: bool,
    /// Labels to add.
    #[serde(default)]
    pub add_labels: Vec<String>,
    /// Labels to remove.
    #[serde(default)]
    pub remove_labels: Vec<String>,
    /// Assignees to add.
    #[serde(default)]
    pub add_assignees: Vec<String>,
    /// Assignees to remove.
    #[serde(default)]
    pub remove_assignees: Vec<String>,
    /// Parent task IDs to link.
    #[serde(default)]
    pub link_parents: Vec<String>,
    /// Parent task IDs to unlink.
    #[serde(default)]
    pub unlink_parents: Vec<String>,
    /// Optional actor display name provided via MCP params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_name: Option<String>,
    /// Optional actor email provided via MCP params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_email: Option<String>,
}

/// Parameters for updating a comment.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateCommentParams {
    /// Task ID containing the comment.
    pub task_id: String,
    /// Comment ID to update.
    pub comment_id: String,
    /// New comment body in Markdown.
    pub body_md: String,
    /// Optional actor display name provided via MCP params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_name: Option<String>,
    /// Optional actor email provided via MCP params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_email: Option<String>,
}

/// Parameters for adding a comment.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AddCommentParams {
    /// Task ID to add comment to.
    pub task_id: String,
    /// Comment body in Markdown.
    pub body_md: String,
    /// Optional actor display name provided via MCP params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_name: Option<String>,
    /// Optional actor email provided via MCP params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor_email: Option<String>,
}

/// Parameters for retrieving a single task snapshot.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetTaskParams {
    /// Task ID to fetch.
    pub task_id: String,
}

/// Parameters for listing comments on a task.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListCommentsParams {
    /// Task ID whose comments should be returned.
    pub task_id: String,
}

/// Parameters for listing subtasks of a parent task.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListSubtasksParams {
    /// Parent task ID whose subtasks to list.
    pub parent_task_id: String,
}

/// Parameters for listing tasks with optional filters.
/// Mirrors the CLI/TUI `TaskFilter` fields that are currently exposed via the MCP surface.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListTasksParams {
    /// Limit results to tasks in any of these workflow states.
    #[serde(default)]
    pub states: Vec<String>,
    /// Require every listed label to be present on the task.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Match tasks that include any of these assignees.
    #[serde(default)]
    pub assignees: Vec<String>,
    /// Include only these workflow state kinds.
    #[serde(default)]
    pub include_state_kinds: Vec<String>,
    /// Exclude these workflow state kinds.
    #[serde(default)]
    pub exclude_state_kinds: Vec<String>,
    /// Require tasks to include any of these parents.
    #[serde(default)]
    pub parents: Vec<String>,
    /// Require tasks to include any of these children.
    #[serde(default)]
    pub children: Vec<String>,
    /// Match tasks updated at or after this timestamp (RFC3339).
    #[serde(default)]
    pub updated_since: Option<String>,
    /// Match tasks updated at or before this timestamp (RFC3339).
    #[serde(default)]
    pub updated_until: Option<String>,
    /// Case-insensitive substring search across title/description/state/labels/assignees.
    #[serde(default)]
    pub text: Option<String>,
}

/// Workflow state entry returned by the MCP tool.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkflowStateEntry {
    pub value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<StateKind>,
}

/// Response body for workflow state listings.
#[derive(Debug, Serialize, Deserialize)]
pub struct WorkflowStatesResponse {
    pub restricted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_state: Option<String>,
    pub states: Vec<WorkflowStateEntry>,
}

/// Comment entry returned by the MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCommentEntry {
    pub comment_id: String,
    pub actor: Actor,
    pub body_md: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}
