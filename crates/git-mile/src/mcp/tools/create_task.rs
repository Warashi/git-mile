//! Create task tool implementation.

use crate::mcp::params::CreateTaskParams;
use git_mile_app::WorkflowConfig;
use git_mile_app::{CreateTaskRequest, TaskWriteError, TaskWriter};
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

/// Create a new task with title, labels, assignees, description, state, and parent tasks.
pub async fn handle_create_task(
    store: Arc<Mutex<GitStore>>,
    workflow: WorkflowConfig,
    Parameters(params): Parameters<CreateTaskParams>,
) -> Result<CallToolResult, McpError> {
    let CreateTaskParams {
        title,
        state,
        labels,
        assignees,
        description,
        parents,
        actor_name,
        actor_email,
    } = params;

    let parents = parse_task_ids(parents, "parent task ID")?;
    let task = {
        let writer = TaskWriter::new(store.lock().await, workflow);
        let request = CreateTaskRequest {
            title,
            state,
            labels,
            assignees,
            description,
            parents,
            actor: Actor {
                name: actor_name,
                email: actor_email,
            },
        };

        writer.create_task(request).map_err(map_task_write_error)?.task
    };
    let snapshot = load_snapshot(store, task).await?;

    let json_str =
        serde_json::to_string_pretty(&snapshot).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
