//! Add comment tool implementation.

use crate::mcp::params::AddCommentParams;
use crate::mcp::tools::common::with_store;
use git_mile_app::WorkflowConfig;
use git_mile_app::actor_from_params_or_default;
use git_mile_app::{CommentRequest, TaskWriteError, TaskWriter};
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::path::Path;
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

    let repo_hint = base_dir.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    let actor = actor_from_params_or_default(actor_name.as_deref(), actor_email.as_deref(), &repo_hint);
    let workflow_clone = workflow.clone();
    let hooks_clone = hooks_config.clone();
    let base_dir_clone = base_dir.clone();

    let comment_id = with_store(store, move |cloned_store| {
        TaskWriter::new(cloned_store, workflow_clone, hooks_clone, base_dir_clone)
            .add_comment(task, CommentRequest { body_md, actor })
            .map_err(map_task_write_error)?
            .comment_id
            .map(|id| id.to_string())
            .ok_or_else(|| McpError::internal_error("TaskWriter returned no comment ID", None))
    })
    .await?;

    let response = serde_json::json!({
        "task_id": task.to_string(),
        "comment_id": comment_id,
        "status": "added"
    });

    let json_str =
        serde_json::to_string_pretty(&response).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
