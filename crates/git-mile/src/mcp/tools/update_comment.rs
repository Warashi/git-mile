//! Update comment tool implementation.

use crate::mcp::params::UpdateCommentParams;
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_store_git::GitStore;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use rmcp::ErrorData as McpError;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Update an existing comment's body.
pub async fn handle_update_comment(
    store: Arc<Mutex<GitStore>>,
    Parameters(params): Parameters<UpdateCommentParams>,
) -> Result<CallToolResult, McpError> {
    let store_guard = store.lock().await;

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

    // Load events and verify comment exists
    let events = store_guard
        .load_events(task)
        .map_err(|e| McpError::invalid_params(format!("Task not found: {e}"), None))?;

    let comment_exists = events.iter().any(
        |ev| matches!(&ev.kind, EventKind::CommentAdded { comment_id: cid, .. } if *cid == comment_id),
    );

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

    // Create CommentUpdated event
    let event = Event::new(
        task,
        &actor,
        EventKind::CommentUpdated {
            comment_id,
            body_md: params.body_md,
        },
    );

    store_guard
        .append_event(&event)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    drop(store_guard);

    // Return success with the updated comment info
    let result = serde_json::json!({
        "task_id": task.to_string(),
        "comment_id": comment_id.to_string(),
        "status": "updated"
    });

    let json_str = serde_json::to_string_pretty(&result)
        .map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
