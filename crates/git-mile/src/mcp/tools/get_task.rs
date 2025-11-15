//! Get task tool implementation.

use crate::mcp::params::GetTaskParams;
use git_mile_core::TaskSnapshot;
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Fetch a single task snapshot by ID.
pub async fn handle_get_task(
    store: Arc<Mutex<GitStore>>,
    Parameters(params): Parameters<GetTaskParams>,
) -> Result<CallToolResult, McpError> {
    let task_id_raw = params.task_id.clone();
    let task: TaskId = task_id_raw
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

    let store_guard = store.lock().await;
    let events = store_guard.load_events(task).map_err(|e| {
        let msg = e.to_string();
        if msg.contains("Task not found") {
            McpError::invalid_params(format!("Task not found: {task_id_raw}"), None)
        } else {
            McpError::internal_error(msg, None)
        }
    })?;

    drop(store_guard);

    let snapshot = TaskSnapshot::replay(&events);
    let json_str =
        serde_json::to_string_pretty(&snapshot).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
