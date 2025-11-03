//! MCP server implementation for git-mile.

use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::TaskId;
use git_mile_core::TaskSnapshot;
use git_mile_store_git::GitStore;
use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Content, Implementation, InitializeResult,
    ListToolsResult, ProtocolVersion, ServerCapabilities,
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
    /// Actor name (defaults to "git-mile").
    #[serde(default = "default_actor_name")]
    pub actor_name: String,
    /// Actor email (defaults to "git-mile@example.invalid").
    #[serde(default = "default_actor_email")]
    pub actor_email: String,
}

fn default_actor_name() -> String {
    "git-mile".to_owned()
}

fn default_actor_email() -> String {
    "git-mile@example.invalid".to_owned()
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

        let json_str = serde_json::to_string_pretty(&tasks)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;

        Ok(CallToolResult::success(vec![Content::text(json_str)]))
    }

    /// Create a new task.
    #[tool(description = "Create a new task with title, labels, assignees, description, and state")]
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

        // Load the newly created task to return its snapshot
        let events = store
            .load_events(task)
            .map_err(|e| McpError::internal_error(e.to_string(), None))?;
        let snapshot = TaskSnapshot::replay(&events);

        let json_str = serde_json::to_string_pretty(&snapshot)
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
