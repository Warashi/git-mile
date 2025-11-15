//! List subtasks tool implementation.

use crate::mcp::params::ListSubtasksParams;
use git_mile_core::TaskSnapshot;
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::cmp::Ordering;
use std::sync::Arc;
use tokio::sync::Mutex;

fn compare_snapshots(a: &TaskSnapshot, b: &TaskSnapshot) -> Ordering {
    match (a.updated_at(), b.updated_at()) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    }
}

/// List all subtasks (children) of a given parent task.
pub async fn handle_list_subtasks(
    store: Arc<Mutex<GitStore>>,
    Parameters(params): Parameters<ListSubtasksParams>,
) -> Result<CallToolResult, McpError> {
    let parent_id_raw = params.parent_task_id.clone();
    let parent: TaskId = parent_id_raw
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid parent task ID: {e}"), None))?;

    let store_guard = store.lock().await;

    // Load parent task and get its children
    let parent_events = store_guard.load_events(parent).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Task not found") {
            McpError::invalid_params(format!("Parent task not found: {parent_id_raw}"), None)
        } else {
            McpError::internal_error(msg, None)
        }
    })?;

    // Keep parent snapshot for potential future use (currently just validation).
    let _parent_snapshot = TaskSnapshot::replay(&parent_events);

    let task_ids = store_guard
        .list_tasks()
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    // Load subtasks by scanning for tasks that reference the parent.
    let mut subtasks = Vec::new();
    for candidate in task_ids {
        if candidate == parent {
            continue;
        }
        let child_events = store_guard
            .load_events(candidate)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let child_snapshot = TaskSnapshot::replay(&child_events);
        if child_snapshot.parents.contains(&parent) {
            subtasks.push(child_snapshot);
        }
    }

    subtasks.sort_by(compare_snapshots);

    drop(store_guard);

    let json_str =
        serde_json::to_string_pretty(&subtasks).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
