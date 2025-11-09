//! MCP server implementation for git-mile.

use crate::config::WorkflowConfig;
use crate::task_writer::{
    CommentRequest, CreateTaskRequest, DescriptionPatch, SetDiff, StatePatch, TaskUpdate, TaskWriteError,
    TaskWriter,
};
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::TaskId;
use git_mile_core::{OrderedEvents, StateKind, TaskFilter, TaskSnapshot};
use git_mile_store_git::GitStore;
use rmcp::handler::server::ServerHandler;
use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, Implementation, InitializeResult, ListToolsResult,
    ProtocolVersion, ServerCapabilities,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{ErrorData as McpError, tool, tool_router};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::Arc;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;
use tokio::sync::Mutex;

/// Parameters for creating a new task.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateTaskParams {
    /// Human-readable title for the task.
    pub title: String,
    /// Optional workflow state label. Falls back to `workflow.default_state` when omitted.
    #[serde(default)]
    pub state: Option<String>,
    /// Labels to attach to the task.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Initial assignees.
    #[serde(default)]
    pub assignees: Vec<String>,
    /// Optional description in Markdown.
    #[serde(default)]
    pub description: Option<String>,
    /// Parent task IDs to link this task to.
    #[serde(default)]
    pub parents: Vec<String>,
    /// Actor name (defaults from `GIT_MILE_ACTOR_NAME`, `GIT_AUTHOR_NAME`, `user.name`, or "git-mile").
    #[serde(default = "default_actor_name")]
    pub actor_name: String,
    /// Actor email (defaults from `GIT_MILE_ACTOR_EMAIL`, `GIT_AUTHOR_EMAIL`, `user.email`, or "git-mile@example.invalid").
    #[serde(default = "default_actor_email")]
    pub actor_email: String,
}

fn default_actor_name() -> String {
    std::env::var("GIT_MILE_ACTOR_NAME")
        .or_else(|_| std::env::var("GIT_AUTHOR_NAME"))
        .or_else(|_| {
            git2::Config::open_default()
                .and_then(|config| config.get_string("user.name"))
                .map_err(|_| std::env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| "git-mile".to_owned())
}

fn default_actor_email() -> String {
    std::env::var("GIT_MILE_ACTOR_EMAIL")
        .or_else(|_| std::env::var("GIT_AUTHOR_EMAIL"))
        .or_else(|_| {
            git2::Config::open_default()
                .and_then(|config| config.get_string("user.email"))
                .map_err(|_| std::env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| "git-mile@example.invalid".to_owned())
}

/// Parameters for updating an existing task.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateTaskParams {
    /// Task ID to update.
    pub task_id: String,
    /// New title (if provided, overwrites the current title).
    #[serde(default)]
    pub title: Option<String>,
    /// New description (if provided, overwrites the current description).
    #[serde(default)]
    pub description: Option<String>,
    /// New state (if provided, sets the workflow state).
    #[serde(default)]
    pub state: Option<String>,
    /// If true, clears the workflow state.
    #[serde(default)]
    pub clear_state: bool,
    /// Labels to add.
    #[serde(default)]
    pub add_labels: Vec<String>,
    /// Labels to remove.
    #[serde(default)]
    pub remove_labels: Vec<String>,
    /// Assignees to add.
    #[serde(default)]
    pub add_assignees: Vec<String>,
    /// Assignees to remove.
    #[serde(default)]
    pub remove_assignees: Vec<String>,
    /// Parent task IDs to link.
    #[serde(default)]
    pub link_parents: Vec<String>,
    /// Parent task IDs to unlink.
    #[serde(default)]
    pub unlink_parents: Vec<String>,
    /// Actor name (defaults from `GIT_MILE_ACTOR_NAME`, `GIT_AUTHOR_NAME`, `user.name`, or "git-mile").
    #[serde(default = "default_actor_name")]
    pub actor_name: String,
    /// Actor email (defaults from `GIT_MILE_ACTOR_EMAIL`, `GIT_AUTHOR_EMAIL`, `user.email`, or "git-mile@example.invalid").
    #[serde(default = "default_actor_email")]
    pub actor_email: String,
}

/// Parameters for updating a comment.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateCommentParams {
    /// Task ID containing the comment.
    pub task_id: String,
    /// Comment ID to update.
    pub comment_id: String,
    /// New comment body in Markdown.
    pub body_md: String,
    /// Actor name (defaults from `GIT_MILE_ACTOR_NAME`, `GIT_AUTHOR_NAME`, `user.name`, or "git-mile").
    #[serde(default = "default_actor_name")]
    pub actor_name: String,
    /// Actor email (defaults from `GIT_MILE_ACTOR_EMAIL`, `GIT_AUTHOR_EMAIL`, `user.email`, or "git-mile@example.invalid").
    #[serde(default = "default_actor_email")]
    pub actor_email: String,
}

/// Parameters for adding a comment.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AddCommentParams {
    /// Task ID to add comment to.
    pub task_id: String,
    /// Comment body in Markdown.
    pub body_md: String,
    /// Actor name (defaults from `GIT_MILE_ACTOR_NAME`, `GIT_AUTHOR_NAME`, `user.name`, or "git-mile").
    #[serde(default = "default_actor_name")]
    pub actor_name: String,
    /// Actor email (defaults from `GIT_MILE_ACTOR_EMAIL`, `GIT_AUTHOR_EMAIL`, `user.email`, or "git-mile@example.invalid").
    #[serde(default = "default_actor_email")]
    pub actor_email: String,
}

/// Parameters for retrieving a single task snapshot.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct GetTaskParams {
    /// Task ID to fetch.
    pub task_id: String,
}

/// Parameters for listing comments on a task.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListCommentsParams {
    /// Task ID whose comments should be returned.
    pub task_id: String,
}

/// Parameters for listing subtasks of a parent task.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListSubtasksParams {
    /// Parent task ID whose subtasks to list.
    pub parent_task_id: String,
}

/// Parameters for listing tasks with optional filters.
/// Mirrors the CLI/TUI `TaskFilter` fields that are currently exposed via the MCP surface.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ListTasksParams {
    /// Limit results to tasks in any of these workflow states.
    #[serde(default)]
    pub states: Vec<String>,
    /// Require every listed label to be present on the task.
    #[serde(default)]
    pub labels: Vec<String>,
    /// Match tasks that include any of these assignees.
    #[serde(default)]
    pub assignees: Vec<String>,
    /// Case-insensitive substring search across title/description/state/labels/assignees.
    #[serde(default)]
    pub text: Option<String>,
}

/// Workflow state entry returned by the MCP tool.
#[derive(Debug, Serialize, Deserialize)]
struct WorkflowStateEntry {
    value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kind: Option<StateKind>,
}

/// Response body for workflow state listings.
#[derive(Debug, Serialize, Deserialize)]
struct WorkflowStatesResponse {
    restricted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    default_state: Option<String>,
    states: Vec<WorkflowStateEntry>,
}

/// Comment entry returned by the MCP tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct TaskCommentEntry {
    comment_id: String,
    actor: Actor,
    body_md: String,
    created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_at: Option<String>,
}

impl ListTasksParams {
    fn into_filter(self) -> TaskFilter {
        TaskFilter {
            states: self.states.into_iter().collect(),
            labels: self.labels.into_iter().collect(),
            assignees: self.assignees.into_iter().collect(),
            text: normalize_text(self.text),
            ..TaskFilter::default()
        }
    }
}

fn normalize_text(input: Option<String>) -> Option<String> {
    input.and_then(|candidate| {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            None
        } else if trimmed.len() == candidate.len() {
            Some(candidate)
        } else {
            Some(trimmed.to_owned())
        }
    })
}

fn format_timestamp(ts: OffsetDateTime) -> Result<String, McpError> {
    ts.format(&Rfc3339)
        .map_err(|e| McpError::internal_error(e.to_string(), None))
}

fn compare_snapshots(a: &TaskSnapshot, b: &TaskSnapshot) -> Ordering {
    match (a.updated_at(), b.updated_at()) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    }
}

/// MCP server for git-mile.
#[derive(Clone)]
pub struct GitMileServer {
    tool_router: ToolRouter<Self>,
    store: Arc<Mutex<GitStore>>,
    workflow: WorkflowConfig,
}

#[tool_router]
impl GitMileServer {
    /// Create a new MCP server instance.
    pub fn new(store: GitStore, workflow: WorkflowConfig) -> Self {
        Self {
            tool_router: Self::tool_router(),
            store: Arc::new(Mutex::new(store)),
            workflow,
        }
    }

    async fn load_snapshot(&self, task: TaskId) -> Result<TaskSnapshot, McpError> {
        let store = self.store.lock().await;
        let events = store
            .load_events(task)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        drop(store);
        Ok(TaskSnapshot::replay(&events))
    }

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

    /// List tasks with optional filters.
    #[tool(description = "List tasks in the repository, optionally filtered by state/label/assignee/text")]
    async fn list_tasks(
        &self,
        Parameters(params): Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        let filter = params.into_filter();
        let store = self.store.lock().await;
        let task_ids = store
            .list_tasks()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        let mut tasks = Vec::new();
        for task_id in task_ids {
            let events = store
                .load_events(task_id)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            let snapshot = TaskSnapshot::replay(&events);
            tasks.push(snapshot);
        }

        drop(store);

        let tasks = if filter.is_empty() {
            tasks
        } else {
            tasks
                .into_iter()
                .filter(|snapshot| filter.matches(snapshot))
                .collect()
        };

        let json_str = serde_json::to_string_pretty(&tasks)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json_str)]))
    }

    /// List workflow states configured for this repository.
    #[tool(description = "List workflow states and default selection configured for this repository")]
    async fn list_workflow_states(&self) -> Result<CallToolResult, McpError> {
        let response = WorkflowStatesResponse {
            restricted: self.workflow.is_restricted(),
            default_state: self.workflow.default_state().map(str::to_owned),
            states: self
                .workflow
                .states()
                .iter()
                .map(|state| WorkflowStateEntry {
                    value: state.value().to_owned(),
                    label: state.label().map(str::to_owned),
                    kind: state.kind(),
                })
                .collect(),
        };

        let json_str = serde_json::to_string_pretty(&response)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json_str)]))
    }

    /// Fetch a single task snapshot by ID.
    #[tool(description = "Fetch a single task snapshot by ID")]
    async fn get_task(
        &self,
        Parameters(params): Parameters<GetTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        let task_id_raw = params.task_id.clone();
        let task: TaskId = task_id_raw
            .parse()
            .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

        let store = self.store.lock().await;
        let events = store.load_events(task).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("Task not found") {
                McpError::invalid_params(format!("Task not found: {task_id_raw}"), None)
            } else {
                McpError::internal_error(msg, None)
            }
        })?;

        drop(store);

        let snapshot = TaskSnapshot::replay(&events);
        let json_str = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json_str)]))
    }

    /// List comments recorded on a task.
    #[tool(description = "List all comments on a task in chronological order")]
    async fn list_comments(
        &self,
        Parameters(params): Parameters<ListCommentsParams>,
    ) -> Result<CallToolResult, McpError> {
        use git_mile_core::id::EventId;

        let task_id_raw = params.task_id.clone();
        let task: TaskId = task_id_raw
            .parse()
            .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

        let store = self.store.lock().await;
        let events = store.load_events(task).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("Task not found") {
                McpError::invalid_params(format!("Task not found: {task_id_raw}"), None)
            } else {
                McpError::internal_error(msg, None)
            }
        })?;
        drop(store);

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

        let json_str = serde_json::to_string_pretty(&comments)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json_str)]))
    }

    /// List all subtasks of a parent task.
    #[tool(description = "List all subtasks (children) of a given parent task")]
    async fn list_subtasks(
        &self,
        Parameters(params): Parameters<ListSubtasksParams>,
    ) -> Result<CallToolResult, McpError> {
        let parent_id_raw = params.parent_task_id.clone();
        let parent: TaskId = parent_id_raw
            .parse()
            .map_err(|e| McpError::invalid_params(format!("Invalid parent task ID: {e}"), None))?;

        let store = self.store.lock().await;

        // Load parent task and get its children
        let parent_events = store.load_events(parent).map_err(|e| {
            let msg = e.to_string();
            if msg.contains("Task not found") {
                McpError::invalid_params(format!("Parent task not found: {parent_id_raw}"), None)
            } else {
                McpError::internal_error(msg, None)
            }
        })?;

        // Keep parent snapshot for potential future use (currently just validation).
        let _parent_snapshot = TaskSnapshot::replay(&parent_events);

        let task_ids = store
            .list_tasks()
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // Load subtasks by scanning for tasks that reference the parent.
        let mut subtasks = Vec::new();
        for candidate in task_ids {
            if candidate == parent {
                continue;
            }
            let child_events = store
                .load_events(candidate)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            let child_snapshot = TaskSnapshot::replay(&child_events);
            if child_snapshot.parents.contains(&parent) {
                subtasks.push(child_snapshot);
            }
        }

        subtasks.sort_by(compare_snapshots);

        drop(store);

        let json_str = serde_json::to_string_pretty(&subtasks)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json_str)]))
    }

    /// Create a new task.
    #[tool(
        description = "Create a new task with title, labels, assignees, description, state, and parent tasks"
    )]
    async fn create_task(
        &self,
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

        let parents = Self::parse_task_ids(parents, "parent task ID")?;
        let task = {
            let store = self.store.lock().await;
            let writer = TaskWriter::new(&*store, self.workflow.clone());
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

            writer
                .create_task(request)
                .map_err(Self::map_task_write_error)?
                .task
        };
        let snapshot = self.load_snapshot(task).await?;

        let json_str = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json_str)]))
    }

    /// Update an existing task.
    #[tool(
        description = "Update an existing task's title, description, state, labels, assignees, or parent tasks"
    )]
    async fn update_task(
        &self,
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
        let mut update = TaskUpdate::default();
        update.title = title;
        update.description = description.map(|body| DescriptionPatch::Set { description: body });
        update.state = if let Some(value) = state {
            Some(StatePatch::Set { state: value })
        } else if clear_state {
            Some(StatePatch::Clear)
        } else {
            None
        };
        update.labels = SetDiff {
            added: add_labels,
            removed: remove_labels,
        };
        update.assignees = SetDiff {
            added: add_assignees,
            removed: remove_assignees,
        };

        let actor = Actor {
            name: actor_name,
            email: actor_email,
        };

        let link_parent_ids = Self::parse_task_ids(link_parents, "parent task ID")?;
        let unlink_parent_ids = Self::parse_task_ids(unlink_parents, "parent task ID")?;

        {
            let store = self.store.lock().await;
            let writer = TaskWriter::new(&*store, self.workflow.clone());

            writer
                .update_task(task, update, &actor)
                .map_err(Self::map_task_write_error)?;

            if !link_parent_ids.is_empty() {
                writer
                    .link_parents(task, &link_parent_ids, &actor)
                    .map_err(Self::map_task_write_error)?;
            }

            if !unlink_parent_ids.is_empty() {
                writer
                    .unlink_parents(task, &unlink_parent_ids, &actor)
                    .map_err(Self::map_task_write_error)?;
            }
        }

        let snapshot = self.load_snapshot(task).await?;
        let json_str = serde_json::to_string_pretty(&snapshot)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json_str)]))
    }

    /// Update a comment.
    #[tool(description = "Update an existing comment's body")]
    async fn update_comment(
        &self,
        Parameters(params): Parameters<UpdateCommentParams>,
    ) -> Result<CallToolResult, McpError> {
        use git_mile_core::id::EventId;

        let store = self.store.lock().await;

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
        let events = store
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

        store
            .append_event(&event)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        drop(store);

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

    /// Add a comment to a task.
    #[tool(description = "Add a comment to a task")]
    async fn add_comment(
        &self,
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

        let comment_id = {
            let store = self.store.lock().await;
            let writer = TaskWriter::new(&*store, self.workflow.clone());
            let result = writer
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
                .map_err(Self::map_task_write_error)?;

            result
                .comment_id
                .map(|id| id.to_string())
                .ok_or_else(|| McpError::internal_error("TaskWriter returned no comment ID", None))
        }?;

        let response = serde_json::json!({
            "task_id": task.to_string(),
            "comment_id": comment_id,
            "status": "added"
        });

        let json_str = serde_json::to_string_pretty(&response)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json_str)]))
    }
}

impl ServerHandler for GitMileServer {
    fn get_info(&self) -> InitializeResult {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build();

        InitializeResult {
            protocol_version: ProtocolVersion::LATEST,
            capabilities,
            server_info: Implementation {
                name: "git-mile".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                icons: None,
                title: None,
                website_url: None,
            },
            instructions: None,
        }
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult {
            tools: self.tool_router.list_all(),
            next_cursor: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, McpError> {
        let tool_context = ToolCallContext::new(self, request, context);
        self.tool_router.call(tool_context).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::WorkflowState;
    use anyhow::{Context, Result, anyhow};
    use git_mile_core::{
        StateKind, TaskSnapshot,
        event::{Actor, Event, EventKind},
        id::TaskId,
    };
    use git2::Repository;
    use rmcp::model::{ErrorCode, RawContent};
    use serde::Deserialize;
    use tempfile::tempdir;

    #[tokio::test]
    async fn get_task_returns_snapshot() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;

        let task_id = seed_task(repo.path())?;
        let server = GitMileServer::new(GitStore::open(repo.path())?, WorkflowConfig::default());

        let result = server
            .get_task(Parameters(GetTaskParams {
                task_id: task_id.to_string(),
            }))
            .await?;

        let Some(content) = result.content.first() else {
            panic!("tool response should include content");
        };
        let text = match &content.raw {
            RawContent::Text(block) => block.text.clone(),
            _ => panic!("expected text content"),
        };
        let snapshot: TaskSnapshot = serde_json::from_str(&text)?;

        assert_eq!(snapshot.id, task_id);
        assert_eq!(snapshot.title, "MCP test");
        assert_eq!(snapshot.state.as_deref(), Some("state/todo"));
        assert!(snapshot.labels.contains("label/docs"));
        assert_eq!(snapshot.description, "hello");

        Ok(())
    }

    #[tokio::test]
    async fn get_task_with_missing_id_returns_invalid_params() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;

        let server = GitMileServer::new(GitStore::open(repo.path())?, WorkflowConfig::default());

        let Err(err) = server
            .get_task(Parameters(GetTaskParams {
                task_id: TaskId::new().to_string(),
            }))
            .await
        else {
            panic!("missing task should return error");
        };

        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("Task not found"));

        Ok(())
    }

    #[tokio::test]
    async fn create_task_uses_default_state() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;
        let workflow = WorkflowConfig::from_states_with_default(
            vec![WorkflowState::new("state/todo")],
            Some("state/todo"),
        );
        let server = GitMileServer::new(GitStore::open(repo.path())?, workflow);

        let result = server
            .create_task(Parameters(CreateTaskParams {
                title: "Demo".into(),
                state: None,
                labels: vec![],
                assignees: vec![],
                description: None,
                parents: vec![],
                actor_name: "tester".into(),
                actor_email: "tester@example.invalid".into(),
            }))
            .await?;

        let Some(content) = result.content.first() else {
            panic!("tool response should include content");
        };
        let text = match &content.raw {
            RawContent::Text(block) => block.text.clone(),
            _ => panic!("expected text content"),
        };
        let snapshot: TaskSnapshot = serde_json::from_str(&text)?;
        assert_eq!(snapshot.state.as_deref(), Some("state/todo"));
        Ok(())
    }

    #[tokio::test]
    async fn list_tasks_applies_filters() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;
        let store = GitStore::open(repo.path())?;

        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let done_task = TaskId::new();
        let done_event = Event::new(
            done_task,
            &actor,
            EventKind::TaskCreated {
                title: "Ship docs".into(),
                labels: vec!["label/docs".into()],
                assignees: vec!["alice".into()],
                description: Some("Document the release".into()),
                state: Some("state/done".into()),
                state_kind: None,
            },
        );
        store.append_event(&done_event)?;

        let todo_task = TaskId::new();
        let todo_event = Event::new(
            todo_task,
            &actor,
            EventKind::TaskCreated {
                title: "Implement feature".into(),
                labels: vec!["label/feature".into()],
                assignees: vec!["bob".into()],
                description: Some("Feature work is pending".into()),
                state: Some("state/todo".into()),
                state_kind: None,
            },
        );
        store.append_event(&todo_event)?;

        let server = GitMileServer::new(store, WorkflowConfig::default());

        let snapshots = decode_task_list(
            server
                .list_tasks(Parameters(ListTasksParams {
                    states: vec!["state/done".into()],
                    ..Default::default()
                }))
                .await?,
        )?;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, done_task);

        let snapshots = decode_task_list(
            server
                .list_tasks(Parameters(ListTasksParams {
                    labels: vec!["label/feature".into()],
                    ..Default::default()
                }))
                .await?,
        )?;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, todo_task);

        let snapshots = decode_task_list(
            server
                .list_tasks(Parameters(ListTasksParams {
                    assignees: vec!["alice".into()],
                    ..Default::default()
                }))
                .await?,
        )?;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, done_task);

        let snapshots = decode_task_list(
            server
                .list_tasks(Parameters(ListTasksParams {
                    text: Some("FEATURE".into()),
                    ..Default::default()
                }))
                .await?,
        )?;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, todo_task);

        Ok(())
    }

    #[tokio::test]
    async fn list_workflow_states_reflects_workflow_config() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;
        let workflow = WorkflowConfig::from_states_with_default(
            vec![WorkflowState::new("state/todo"), WorkflowState::new("state/done")],
            Some("state/todo"),
        );
        let server = GitMileServer::new(GitStore::open(repo.path())?, workflow);

        let result = server.list_workflow_states().await?;
        let Some(content) = result.content.first() else {
            panic!("tool response should include content");
        };
        let text = match &content.raw {
            RawContent::Text(block) => block.text.clone(),
            _ => panic!("expected text content"),
        };
        let response: WorkflowStatesResponse = serde_json::from_str(&text)?;

        assert!(response.restricted);
        assert_eq!(response.default_state.as_deref(), Some("state/todo"));
        assert_eq!(response.states.len(), 2);
        assert_eq!(response.states[0].value, "state/todo");

        Ok(())
    }

    #[tokio::test]
    async fn list_subtasks_returns_children() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;
        let server = GitMileServer::new(GitStore::open(repo.path())?, WorkflowConfig::default());

        let parent_snapshot = decode_task_result(
            server
                .create_task(Parameters(create_task_params("Parent task", vec![])))
                .await?,
        )?;
        let parent = parent_snapshot.id;

        let _child1 = server
            .create_task(Parameters(create_task_params(
                "Child 1",
                vec![parent.to_string()],
            )))
            .await?;
        let _child2 = server
            .create_task(Parameters(create_task_params(
                "Child 2",
                vec![parent.to_string()],
            )))
            .await?;

        // Test list_subtasks
        let result = server
            .list_subtasks(Parameters(ListSubtasksParams {
                parent_task_id: parent.to_string(),
            }))
            .await?;

        let Some(content) = result.content.first() else {
            panic!("tool response should include content");
        };
        let text = match &content.raw {
            RawContent::Text(block) => block.text.clone(),
            _ => panic!("expected text content"),
        };
        let subtasks: Vec<TaskSnapshot> = serde_json::from_str(&text)?;

        assert_eq!(subtasks.len(), 2);
        let titles: Vec<_> = subtasks.iter().map(|s| s.title.as_str()).collect();
        assert!(titles.contains(&"Child 1"));
        assert!(titles.contains(&"Child 2"));

        Ok(())
    }

    #[tokio::test]
    async fn list_subtasks_supports_legacy_links() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;
        let store = GitStore::open(repo.path())?;
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let parent = TaskId::new();
        let parent_event = Event::new(
            parent,
            &actor,
            EventKind::TaskCreated {
                title: "Parent".into(),
                labels: vec![],
                assignees: vec![],
                description: None,
                state: None,
                state_kind: None,
            },
        );
        store.append_event(&parent_event)?;

        let child = TaskId::new();
        let child_event = Event::new(
            child,
            &actor,
            EventKind::TaskCreated {
                title: "Child".into(),
                labels: vec![],
                assignees: vec![],
                description: None,
                state: None,
                state_kind: None,
            },
        );
        store.append_event(&child_event)?;

        // Legacy link emits only the child-side event.
        let child_link = Event::new(child, &actor, EventKind::ChildLinked { parent, child });
        store.append_event(&child_link)?;

        let server = GitMileServer::new(store, WorkflowConfig::default());

        let result = server
            .list_subtasks(Parameters(ListSubtasksParams {
                parent_task_id: parent.to_string(),
            }))
            .await?;
        let subtasks = decode_task_list(result)?;
        assert_eq!(subtasks.len(), 1);
        assert_eq!(subtasks[0].id, child);
        Ok(())
    }

    #[tokio::test]
    async fn list_subtasks_with_invalid_parent_returns_error() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;
        let server = GitMileServer::new(GitStore::open(repo.path())?, WorkflowConfig::default());

        let Err(err) = server
            .list_subtasks(Parameters(ListSubtasksParams {
                parent_task_id: TaskId::new().to_string(),
            }))
            .await
        else {
            panic!("missing parent should return error");
        };

        assert_eq!(err.code, ErrorCode::INVALID_PARAMS);
        assert!(err.message.contains("Parent task not found"));

        Ok(())
    }

    #[tokio::test]
    async fn list_comments_returns_comment_history() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;
        let server = GitMileServer::new(GitStore::open(repo.path())?, WorkflowConfig::default());

        let task_snapshot = decode_task_result(
            server
                .create_task(Parameters(create_task_params("Commented task", vec![])))
                .await?,
        )?;
        let task_id = task_snapshot.id.to_string();

        server
            .add_comment(Parameters(AddCommentParams {
                task_id: task_id.clone(),
                body_md: "first comment".into(),
                actor_name: "alice".into(),
                actor_email: "alice@example.invalid".into(),
            }))
            .await?;
        server
            .add_comment(Parameters(AddCommentParams {
                task_id: task_id.clone(),
                body_md: "second comment".into(),
                actor_name: "bob".into(),
                actor_email: "bob@example.invalid".into(),
            }))
            .await?;

        let comments = decode_comment_list(
            server
                .list_comments(Parameters(ListCommentsParams {
                    task_id: task_id.clone(),
                }))
                .await?,
        )?;

        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].body_md, "first comment");
        assert_eq!(comments[0].actor.name, "alice");
        assert_eq!(comments[1].body_md, "second comment");
        assert_eq!(comments[1].actor.name, "bob");
        assert!(comments.iter().all(|c| !c.comment_id.is_empty()));
        assert!(comments.iter().all(|c| !c.created_at.is_empty()));
        assert!(comments.iter().all(|c| c.updated_at.is_none()));

        Ok(())
    }

    #[tokio::test]
    async fn list_comments_reflects_updates() -> Result<()> {
        let repo = tempdir()?;
        Repository::init(repo.path())?;
        let server = GitMileServer::new(GitStore::open(repo.path())?, WorkflowConfig::default());

        let task_snapshot = decode_task_result(
            server
                .create_task(Parameters(create_task_params("Task with edits", vec![])))
                .await?,
        )?;
        let task_id = task_snapshot.id.to_string();

        let comment_id = decode_comment_operation_id(
            server
                .add_comment(Parameters(AddCommentParams {
                    task_id: task_id.clone(),
                    body_md: "initial".into(),
                    actor_name: "alice".into(),
                    actor_email: "alice@example.invalid".into(),
                }))
                .await?,
        )?;

        server
            .update_comment(Parameters(UpdateCommentParams {
                task_id: task_id.clone(),
                comment_id,
                body_md: "edited body".into(),
                actor_name: "carol".into(),
                actor_email: "carol@example.invalid".into(),
            }))
            .await?;

        let comments = decode_comment_list(
            server
                .list_comments(Parameters(ListCommentsParams { task_id }))
                .await?,
        )?;

        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body_md, "edited body");
        assert!(
            comments[0].updated_at.is_some(),
            "updated comments must include updated_at"
        );

        Ok(())
    }

    #[derive(Deserialize)]
    struct CommentResponse {
        comment_id: String,
        actor: Actor,
        body_md: String,
        created_at: String,
        updated_at: Option<String>,
    }

    #[derive(Deserialize)]
    struct CommentOperation {
        comment_id: String,
    }

    fn decode_comment_list(result: CallToolResult) -> Result<Vec<CommentResponse>> {
        let content = result
            .content
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("tool response should include content"))?;
        let text = match content.raw {
            RawContent::Text(block) => block.text,
            _ => return Err(anyhow!("expected text content")),
        };
        serde_json::from_str(&text).context("must decode comment entries")
    }

    fn decode_comment_operation_id(result: CallToolResult) -> Result<String> {
        let content = result
            .content
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("tool response should include content"))?;
        let text = match content.raw {
            RawContent::Text(block) => block.text,
            _ => return Err(anyhow!("expected text content")),
        };
        let parsed: CommentOperation = serde_json::from_str(&text).context("must decode comment op")?;
        Ok(parsed.comment_id)
    }

    fn decode_task_list(result: CallToolResult) -> Result<Vec<TaskSnapshot>> {
        let content = result
            .content
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("tool response should include content"))?;
        let text = match content.raw {
            RawContent::Text(block) => block.text,
            _ => return Err(anyhow!("expected text content")),
        };
        serde_json::from_str(&text).context("must decode task snapshots")
    }

    fn decode_task_result(result: CallToolResult) -> Result<TaskSnapshot> {
        let content = result
            .content
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("tool response should include content"))?;
        let text = match content.raw {
            RawContent::Text(block) => block.text,
            _ => return Err(anyhow!("expected text content")),
        };
        serde_json::from_str(&text).context("must decode task snapshot")
    }

    fn create_task_params(title: &str, parents: Vec<String>) -> CreateTaskParams {
        CreateTaskParams {
            title: title.to_owned(),
            state: None,
            labels: vec![],
            assignees: vec![],
            description: None,
            parents,
            actor_name: "tester".into(),
            actor_email: "tester@example.invalid".into(),
        }
    }

    fn seed_task(repo_path: &std::path::Path) -> Result<TaskId> {
        let store = GitStore::open(repo_path)?;
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let event = Event::new(
            task,
            &actor,
            EventKind::TaskCreated {
                title: "MCP test".into(),
                labels: vec!["label/docs".into()],
                assignees: vec![],
                description: Some("hello".into()),
                state: Some("state/todo".into()),
                state_kind: Some(StateKind::Todo),
            },
        );

        store.append_event(&event)?;
        Ok(task)
    }
}
