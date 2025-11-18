//! Add comment tool implementation.

use crate::mcp::params::AddCommentParams;
use git_mile_app::WorkflowConfig;
use git_mile_app::{CommentRequest, TaskWriteError, TaskWriter};
use git_mile_core::event::Actor;
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::sync::Arc;
use tokio::sync::Mutex;

fn map_task_write_error(err: TaskWriteError) -> McpError {
    match err {
        TaskWriteError::InvalidState(state) => {
            McpError::invalid_params(format!("Invalid workflow state: {state}"), None)
        }
        TaskWriteError::MissingParent(parent) => {
            McpError::invalid_params(format!("Parent task not found: {parent}"), None)
        }
        TaskWriteError::MissingTask(task) => {
            McpError::invalid_params(format!("Task not found: {task}"), None)
        }
        TaskWriteError::Store(error) => McpError::internal_error(error.to_string(), None),
        TaskWriteError::NotImplemented(name) => {
            McpError::internal_error(format!("{name} not implemented"), None)
        }
        TaskWriteError::HookRejected {
            hook,
            exit_code,
            stderr,
        } => McpError::invalid_params(
            format!("Hook '{hook}' rejected operation (exit code {exit_code}): {stderr}"),
            None,
        ),
        TaskWriteError::HookFailed { hook, error } => {
            McpError::internal_error(format!("Hook '{hook}' failed: {error}"), None)
        }
    }
}

/// Add a comment to a task.
pub async fn handle_add_comment(
    store: Arc<Mutex<GitStore>>,
    workflow: WorkflowConfig,
    hooks_config: git_mile_app::HooksConfig,
    base_dir: std::path::PathBuf,
    Parameters(params): Parameters<AddCommentParams>,
) -> Result<CallToolResult, McpError> {
    let AddCommentParams {
        task_id,
        body_md,
        actor_name,
        actor_email,
    } = params;

    let task: TaskId = task_id
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

    let comment_id = TaskWriter::new(store.lock().await, workflow, hooks_config, base_dir)
        .add_comment(
            task,
            CommentRequest {
                body_md,
                actor: Actor {
                    name: actor_name,
                    email: actor_email,
                },
            },
        )
        .map_err(map_task_write_error)?
        .comment_id
        .map(|id| id.to_string())
        .ok_or_else(|| McpError::internal_error("TaskWriter returned no comment ID", None))?;

    let response = serde_json::json!({
        "task_id": task.to_string(),
        "comment_id": comment_id,
        "status": "added"
    });

    let json_str =
        serde_json::to_string_pretty(&response).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
