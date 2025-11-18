//! Update task tool implementation.

use crate::mcp::params::UpdateTaskParams;
use git_mile_app::WorkflowConfig;
use git_mile_app::{DescriptionPatch, SetDiff, StatePatch, TaskUpdate, TaskWriteError, TaskWriter};
use git_mile_core::TaskSnapshot;
use git_mile_core::event::Actor;
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::sync::Arc;
use tokio::sync::Mutex;

fn parse_task_ids(ids: Vec<String>, context: &str) -> Result<Vec<TaskId>, McpError> {
    ids.into_iter()
        .map(|value| {
            value
                .parse()
                .map_err(|e| McpError::invalid_params(format!("Invalid {context}: {e}"), None))
        })
        .collect()
}

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

async fn load_snapshot(store: Arc<Mutex<GitStore>>, task: TaskId) -> Result<TaskSnapshot, McpError> {
    let store_guard = store.lock().await;
    let events = store_guard
        .load_events(task)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    drop(store_guard);
    Ok(TaskSnapshot::replay(&events))
}

/// Update an existing task's title, description, state, labels, assignees, or parent tasks.
pub async fn handle_update_task(
    store: Arc<Mutex<GitStore>>,
    workflow: WorkflowConfig,
    hooks_config: git_mile_app::HooksConfig,
    base_dir: std::path::PathBuf,
    Parameters(params): Parameters<UpdateTaskParams>,
) -> Result<CallToolResult, McpError> {
    let UpdateTaskParams {
        task_id,
        title,
        description,
        state,
        clear_state,
        add_labels,
        remove_labels,
        add_assignees,
        remove_assignees,
        link_parents,
        unlink_parents,
        actor_name,
        actor_email,
    } = params;

    let task: TaskId = task_id
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;
    let update = TaskUpdate {
        title,
        description: description.map(|body| DescriptionPatch::Set { description: body }),
        state: state.map_or_else(
            || {
                if clear_state {
                    Some(StatePatch::Clear)
                } else {
                    None
                }
            },
            |value| Some(StatePatch::Set { state: value }),
        ),
        labels: SetDiff {
            added: add_labels,
            removed: remove_labels,
        },
        assignees: SetDiff {
            added: add_assignees,
            removed: remove_assignees,
        },
    };

    let actor = Actor {
        name: actor_name,
        email: actor_email,
    };

    let link_parent_ids = parse_task_ids(link_parents, "parent task ID")?;
    let unlink_parent_ids = parse_task_ids(unlink_parents, "parent task ID")?;

    {
        let writer = TaskWriter::new(store.lock().await, workflow, hooks_config, base_dir);

        writer
            .update_task(task, update, &actor)
            .map_err(map_task_write_error)?;

        if !link_parent_ids.is_empty() {
            writer
                .link_parents(task, &link_parent_ids, &actor)
                .map_err(map_task_write_error)?;
        }

        if !unlink_parent_ids.is_empty() {
            writer
                .unlink_parents(task, &unlink_parent_ids, &actor)
                .map_err(map_task_write_error)?;
        }
    }

    let snapshot = load_snapshot(store, task).await?;
    let json_str =
        serde_json::to_string_pretty(&snapshot).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
