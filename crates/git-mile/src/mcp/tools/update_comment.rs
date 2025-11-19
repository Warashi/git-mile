//! Update comment tool implementation.

use crate::mcp::params::UpdateCommentParams;
use crate::mcp::tools::common::with_store;
use git_mile_app::AsyncTaskRepository;
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Update an existing comment's body.
pub async fn handle_update_comment(
    store: Arc<Mutex<GitStore>>,
    repository: Arc<AsyncTaskRepository<Arc<Mutex<GitStore>>>>,
    Parameters(params): Parameters<UpdateCommentParams>,
) -> Result<CallToolResult, McpError> {
    // Parse task ID
    let task: TaskId = params
        .task_id
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

    // Parse comment ID
    let comment_id: EventId = params
        .comment_id
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid comment ID: {e}"), None))?;

    // Ensure target comment exists
    let view = repository
        .get_view(task)
        .await
        .map_err(|e| McpError::invalid_params(e.to_string(), None))?;

    let comment_exists = view.comments.iter().any(|comment| comment.id == comment_id);

    if !comment_exists {
        return Err(McpError::invalid_params(
            format!("Comment {comment_id} not found in task {task}"),
            None,
        ));
    }

    let actor = Actor {
        name: params.actor_name,
        email: params.actor_email,
    };
    let body_md = params.body_md;

    with_store(store, move |cloned_store| {
        let mut event = Event::new(task, &actor, EventKind::CommentUpdated { comment_id, body_md });
        let lamport = next_lamport(&cloned_store, task)?;
        event.lamport = lamport;

        cloned_store
            .append_event(&event)
            .map(|_| ())
            .map_err(|e| McpError::internal_error(e.to_string(), None))
    })
    .await?;

    // Return success with the updated comment info
    let result = serde_json::json!({
        "task_id": task.to_string(),
        "comment_id": comment_id.to_string(),
        "status": "updated"
    });

    let json_str =
        serde_json::to_string_pretty(&result).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}

fn next_lamport(store: &GitStore, task: TaskId) -> Result<u64, McpError> {
    let events = store
        .load_events(task)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;
    Ok(events.iter().map(|event| event.lamport).max().unwrap_or(0) + 1)
}
