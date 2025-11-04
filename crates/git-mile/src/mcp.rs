//! MCP server implementation for git-mile.

use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::TaskId;
use git_mile_core::TaskSnapshot;
use git_mile_store_git::GitStore;
use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, Implementation, InitializeResult, ListToolsResult,
    ProtocolVersion, ServerCapabilities,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{tool, tool_router, ErrorData as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Parameters for creating a new task.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CreateTaskParams {
    /// Human-readable title for the task.
    pub title: String,
    /// Optional workflow state label.
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

/// MCP server for git-mile.
#[derive(Clone)]
pub struct GitMileServer {
    tool_router: ToolRouter<Self>,
    store: Arc<Mutex<GitStore>>,
}

#[tool_router]
impl GitMileServer {
    /// Create a new MCP server instance.
    pub fn new(store: GitStore) -> Self {
        Self {
            tool_router: Self::tool_router(),
            store: Arc::new(Mutex::new(store)),
        }
    }

    /// List all tasks in the repository.
    #[tool(description = "List all tasks in the repository with their current state")]
    async fn list_tasks(&self) -> Result<CallToolResult, McpError> {
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

        let json_str = serde_json::to_string_pretty(&tasks)
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
        let store = self.store.lock().await;
        let task = TaskId::new();
        let actor = Actor {
            name: params.actor_name,
            email: params.actor_email,
        };

        let event = Event::new(
            task,
            &actor,
            EventKind::TaskCreated {
                title: params.title,
                labels: params.labels,
                assignees: params.assignees,
                description: params.description,
                state: params.state,
            },
        );

        store
            .append_event(&event)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        // Create ChildLinked events for each parent
        for parent_str in params.parents {
            let parent: TaskId = parent_str
                .parse()
                .map_err(|e| McpError::invalid_params(format!("Invalid parent task ID: {e}"), None))?;

            // Verify parent task exists
            let _ = store
                .load_events(parent)
                .map_err(|e| McpError::invalid_params(format!("Parent task not found: {e}"), None))?;

            let link_event = Event::new(task, &actor, EventKind::ChildLinked { parent, child: task });
            store
                .append_event(&link_event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Load the newly created task to return its snapshot
        let events = store
            .load_events(task)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let snapshot = TaskSnapshot::replay(&events);

        drop(store);

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
        let store = self.store.lock().await;

        // Parse task ID
        let task: TaskId = params
            .task_id
            .parse()
            .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

        // Verify task exists
        let _events = store
            .load_events(task)
            .map_err(|e| McpError::invalid_params(format!("Task not found: {e}"), None))?;

        let actor = Actor {
            name: params.actor_name,
            email: params.actor_email,
        };

        // Process updates in order: title, description, state, labels, assignees

        // Update title
        if let Some(title) = params.title {
            let event = Event::new(task, &actor, EventKind::TaskTitleSet { title });
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Update description
        if let Some(description) = params.description {
            let event = Event::new(
                task,
                &actor,
                EventKind::TaskDescriptionSet {
                    description: Some(description),
                },
            );
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Clear state if requested
        if params.clear_state {
            let event = Event::new(task, &actor, EventKind::TaskStateCleared);
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Set state
        if let Some(state) = params.state {
            let event = Event::new(task, &actor, EventKind::TaskStateSet { state });
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Add labels
        if !params.add_labels.is_empty() {
            let event = Event::new(
                task,
                &actor,
                EventKind::LabelsAdded {
                    labels: params.add_labels,
                },
            );
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Remove labels
        if !params.remove_labels.is_empty() {
            let event = Event::new(
                task,
                &actor,
                EventKind::LabelsRemoved {
                    labels: params.remove_labels,
                },
            );
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Add assignees
        if !params.add_assignees.is_empty() {
            let event = Event::new(
                task,
                &actor,
                EventKind::AssigneesAdded {
                    assignees: params.add_assignees,
                },
            );
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Remove assignees
        if !params.remove_assignees.is_empty() {
            let event = Event::new(
                task,
                &actor,
                EventKind::AssigneesRemoved {
                    assignees: params.remove_assignees,
                },
            );
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Link parents
        for parent_str in params.link_parents {
            let parent: TaskId = parent_str
                .parse()
                .map_err(|e| McpError::invalid_params(format!("Invalid parent task ID: {e}"), None))?;

            // Verify parent task exists
            let _ = store
                .load_events(parent)
                .map_err(|e| McpError::invalid_params(format!("Parent task not found: {e}"), None))?;

            let event = Event::new(task, &actor, EventKind::ChildLinked { parent, child: task });
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Unlink parents
        for parent_str in params.unlink_parents {
            let parent: TaskId = parent_str
                .parse()
                .map_err(|e| McpError::invalid_params(format!("Invalid parent task ID: {e}"), None))?;

            let event = Event::new(task, &actor, EventKind::ChildUnlinked { parent, child: task });
            store
                .append_event(&event)
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        }

        // Load the updated task to return its snapshot
        let events = store
            .load_events(task)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let snapshot = TaskSnapshot::replay(&events);

        drop(store);

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
        use git_mile_core::id::EventId;

        let store = self.store.lock().await;

        // Parse task ID
        let task: TaskId = params
            .task_id
            .parse()
            .map_err(|e| McpError::invalid_params(format!("Invalid task ID: {e}"), None))?;

        // Verify task exists
        let _events = store
            .load_events(task)
            .map_err(|e| McpError::invalid_params(format!("Task not found: {e}"), None))?;

        let actor = Actor {
            name: params.actor_name,
            email: params.actor_email,
        };

        let comment_id = EventId::new();

        // Create CommentAdded event
        let event = Event::new(
            task,
            &actor,
            EventKind::CommentAdded {
                comment_id,
                body_md: params.body_md,
            },
        );

        store
            .append_event(&event)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        drop(store);

        // Return success with the new comment info
        let result = serde_json::json!({
            "task_id": task.to_string(),
            "comment_id": comment_id.to_string(),
            "status": "added"
        });

        let json_str = serde_json::to_string_pretty(&result)
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
