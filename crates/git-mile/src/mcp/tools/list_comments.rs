//! List comments tool implementation.

use crate::mcp::params::{ListCommentsParams, TaskCommentEntry};
use git_mile_app::AsyncTaskRepository;
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::sync::Arc;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::Mutex;

fn map_task_error(err: &anyhow::Error, raw_id: &str) -> McpError {
    let msg = err.to_string();
    if msg.contains("Task not found") {
        McpError::invalid_params(format!("Task not found: {raw_id}"), None)
    } else {
        McpError::internal_error(msg, None)
    }
}

fn format_timestamp(ts: OffsetDateTime) -> Result<String, McpError> {
    ts.format(&Rfc3339)
        .map_err(|e| McpError::internal_error(e.to_string(), None))
}

/// List all comments on a task in chronological order.
pub async fn handle_list_comments(
    repository: Arc<AsyncTaskRepository<Arc<Mutex<GitStore>>>>,
    Parameters(params): Parameters<ListCommentsParams>,
) -> Result<CallToolResult, McpError> {
    let task_id_raw = params.task_id.clone();
    let task: TaskId = task_id_raw
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

    let view = repository
        .get_view(task)
        .await
        .map_err(|err| map_task_error(&err, &task_id_raw))?;

    let comments = view
        .comments
        .iter()
        .map(|comment| {
            Ok(TaskCommentEntry {
                comment_id: comment.id.to_string(),
                actor: comment.actor.clone(),
                body_md: comment.body.clone(),
                created_at: format_timestamp(comment.created_at)?,
                updated_at: comment.updated_at.map(format_timestamp).transpose()?,
            })
        })
        .collect::<Result<Vec<_>, McpError>>()?;

    let json_str =
        serde_json::to_string_pretty(&comments).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
