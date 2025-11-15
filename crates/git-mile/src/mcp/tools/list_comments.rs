//! List comments tool implementation.

use crate::mcp::params::{ListCommentsParams, TaskCommentEntry};
use git_mile_core::OrderedEvents;
use git_mile_core::event::EventKind;
use git_mile_core::id::{EventId, TaskId};
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::Mutex;

fn format_timestamp(ts: OffsetDateTime) -> Result<String, McpError> {
    ts.format(&Rfc3339)
        .map_err(|e| McpError::internal_error(e.to_string(), None))
}

/// List all comments on a task in chronological order.
pub async fn handle_list_comments(
    store: Arc<Mutex<GitStore>>,
    Parameters(params): Parameters<ListCommentsParams>,
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

    let ordered = OrderedEvents::from(events.as_slice());
    let mut comments = Vec::new();
    let mut index: HashMap<EventId, usize> = HashMap::new();

    for ev in ordered.iter() {
        match &ev.kind {
            EventKind::CommentAdded { comment_id, body_md } => {
                let entry = TaskCommentEntry {
                    comment_id: comment_id.to_string(),
                    actor: ev.actor.clone(),
                    body_md: body_md.clone(),
                    created_at: format_timestamp(ev.ts)?,
                    updated_at: None,
                };
                index.insert(*comment_id, comments.len());
                comments.push(entry);
            }
            EventKind::CommentUpdated { comment_id, body_md } => {
                let Some(&position) = index.get(comment_id) else {
                    return Err(McpError::internal_error(
                        format!("Comment {comment_id} was updated before it was added"),
                        None,
                    ));
                };
                let Some(entry) = comments.get_mut(position) else {
                    return Err(McpError::internal_error(
                        format!("Comment map out of sync for {comment_id}"),
                        None,
                    ));
                };
                entry.body_md.clone_from(body_md);
                entry.updated_at = Some(format_timestamp(ev.ts)?);
            }
            _ => {}
        }
    }

    let json_str =
        serde_json::to_string_pretty(&comments).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
