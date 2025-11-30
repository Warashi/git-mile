//! List task events tool implementation.

use crate::mcp::params::ListTaskEventsParams;
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

/// List ordered events for a single task.
pub async fn handle_list_task_events(
    repository: Arc<AsyncTaskRepository<Arc<Mutex<GitStore>>>>,
    Parameters(params): Parameters<ListTaskEventsParams>,
) -> Result<CallToolResult, McpError> {
    let task_id_raw = params.task_id.clone();
    let task: TaskId = task_id_raw
        .parse()
        .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

    let events = repository
        .get_log(task)
        .await
        .map_err(|err| map_task_error(&err, &task_id_raw))?;

    let json =
        serde_json::to_string_pretty(&events).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json)]))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use git_mile_core::event::{Actor, Event, EventKind};
    use git_mile_core::id::TaskId;
    use tempfile::tempdir;
    use time::Duration;

    #[tokio::test]
    async fn returns_events_in_order() {
        let dir = tempdir().expect("create temp dir");
        let repo_path = dir.path();
        git2::Repository::init(repo_path).expect("init repo");
        let store = GitStore::open(repo_path).expect("open store");

        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let mut later = Event::new(
            task,
            &actor,
            EventKind::TaskTitleSet {
                title: "later".into(),
            },
        );
        later.lamport = 2;
        later.ts = later.ts + Duration::seconds(10);

        let mut earlier = Event::new(task, &actor, EventKind::TaskStateCleared);
        earlier.lamport = 1;
        earlier.ts = earlier.ts + Duration::seconds(20);

        store.append_event(&later).expect("append later");
        store.append_event(&earlier).expect("append earlier");

        let shared = Arc::new(Mutex::new(store));
        let repository = Arc::new(AsyncTaskRepository::new(shared));

        let result = handle_list_task_events(
            repository,
            Parameters(ListTaskEventsParams {
                task_id: task.to_string(),
            }),
        )
        .await
        .expect("tool should succeed");

        let content = result
            .content
            .first()
            .and_then(|item| item.as_text().map(|text| text.text.clone()))
            .expect("text content");
        let events: Vec<Event> = serde_json::from_str(&content).expect("parse events");
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].id, earlier.id);
        assert_eq!(events[1].id, later.id);
    }
}
