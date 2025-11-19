//! Get task tool implementation.

use crate::mcp::params::GetTaskParams;
use git_mile_app::AsyncTaskRepository;
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
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

/// Fetch a single task snapshot by ID.
pub async fn handle_get_task(
    repository: Arc<AsyncTaskRepository<Arc<Mutex<GitStore>>>>,
    Parameters(params): Parameters<GetTaskParams>,
) -> Result<CallToolResult, McpError> {
    let task_id_raw = params.task_id.clone();
    let task: TaskId = task_id_raw
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

    let snapshot = repository
        .get_snapshot(task)
        .await
        .map_err(|err| map_task_error(&err, &task_id_raw))?;
    let json_str =
        serde_json::to_string_pretty(&snapshot).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
