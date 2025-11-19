//! List subtasks tool implementation.

use crate::mcp::params::ListSubtasksParams;
use git_mile_app::AsyncTaskRepository;
use git_mile_core::TaskSnapshot;
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::cmp::Ordering;
use std::sync::Arc;
use tokio::sync::Mutex;

fn map_task_error(err: &anyhow::Error, raw_id: &str) -> McpError {
    let msg = err.to_string();
    if msg.contains("Task not found") {
        McpError::invalid_params(format!("Task not found: {raw_id}"), None)
    } else {
        McpError::internal_error(msg, None)
    }
}

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
    repository: Arc<AsyncTaskRepository<Arc<Mutex<GitStore>>>>,
    Parameters(params): Parameters<ListSubtasksParams>,
) -> Result<CallToolResult, McpError> {
    let parent_id_raw = params.parent_task_id.clone();
    let parent: TaskId = parent_id_raw
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid parent task ID: {e}"), None))?;

    // Ensure parent exists
    repository
        .get_snapshot(parent)
        .await
        .map_err(|err| map_task_error(&err, &parent_id_raw))?;

    let child_ids = repository
        .list_children(parent)
        .await
        .map_err(|err| map_task_error(&err, &parent_id_raw))?;

    let mut subtasks = Vec::new();
    for child in child_ids {
        let snapshot = repository
            .get_snapshot(child)
            .await
            .map_err(|err| map_task_error(&err, &child.to_string()))?;
        subtasks.push(snapshot);
    }

    subtasks.sort_by(compare_snapshots);

    let json_str =
        serde_json::to_string_pretty(&subtasks).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
