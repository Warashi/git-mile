//! MCP server implementation for git-mile.

mod params;
mod tools;

pub use params::*;

use crate::async_task_store::AsyncTaskRepository;
use crate::config::WorkflowConfig;
use git_mile_store_git::GitStore;
use rmcp::handler::server::ServerHandler;
use rmcp::handler::server::tool::{ToolCallContext, ToolRouter};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, Implementation, InitializeResult, ListToolsResult,
    ProtocolVersion, ServerCapabilities,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::{tool, tool_router, ErrorData as McpError};
use std::sync::Arc;
use tokio::sync::Mutex;

/// MCP server for git-mile.
#[derive(Clone)]
pub struct GitMileServer {
    tool_router: ToolRouter<Self>,
    store: Arc<Mutex<GitStore>>,
    repository: Arc<AsyncTaskRepository<Arc<Mutex<GitStore>>>>,
    workflow: WorkflowConfig,
}

#[tool_router]
impl GitMileServer {
    /// Create a new MCP server instance.
    pub fn new(store: GitStore, workflow: WorkflowConfig) -> Self {
        let store_arc = Arc::new(Mutex::new(store));
        let repository = Arc::new(AsyncTaskRepository::new(Arc::clone(&store_arc)));

        Self {
            tool_router: Self::tool_router(),
            store: store_arc,
            repository,
            workflow,
        }
    }

    /// List tasks with optional filters.
    #[tool(description = "List tasks in the repository, optionally filtered by state/label/assignee/text")]
    async fn list_tasks(
        &self,
        params: Parameters<ListTasksParams>,
    ) -> Result<CallToolResult, McpError> {
        tools::list_tasks::handle_list_tasks(self.repository.clone(), params).await
    }

    /// List workflow states configured for this repository.
    #[tool(description = "List workflow states and default selection configured for this repository")]
    async fn list_workflow_states(&self) -> Result<CallToolResult, McpError> {
        tools::list_workflow_states::handle_list_workflow_states(&self.workflow).await
    }

    /// Fetch a single task snapshot by ID.
    #[tool(description = "Fetch a single task snapshot by ID")]
    async fn get_task(
        &self,
        params: Parameters<GetTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        tools::get_task::handle_get_task(self.store.clone(), params).await
    }

    /// List comments recorded on a task.
    #[tool(description = "List all comments on a task in chronological order")]
    async fn list_comments(
        &self,
        params: Parameters<ListCommentsParams>,
    ) -> Result<CallToolResult, McpError> {
        tools::list_comments::handle_list_comments(self.store.clone(), params).await
    }

    /// List all subtasks of a parent task.
    #[tool(description = "List all subtasks (children) of a given parent task")]
    async fn list_subtasks(
        &self,
        params: Parameters<ListSubtasksParams>,
    ) -> Result<CallToolResult, McpError> {
        tools::list_subtasks::handle_list_subtasks(self.store.clone(), params).await
    }

    /// Create a new task.
    #[tool(
        description = "Create a new task with title, labels, assignees, description, state, and parent tasks"
    )]
    async fn create_task(
        &self,
        params: Parameters<CreateTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        tools::create_task::handle_create_task(self.store.clone(), self.workflow.clone(), params).await
    }

    /// Update an existing task.
    #[tool(
        description = "Update an existing task's title, description, state, labels, assignees, or parent tasks"
    )]
    async fn update_task(
        &self,
        params: Parameters<UpdateTaskParams>,
    ) -> Result<CallToolResult, McpError> {
        tools::update_task::handle_update_task(self.store.clone(), self.workflow.clone(), params).await
    }

    /// Update a comment.
    #[tool(description = "Update an existing comment's body")]
    async fn update_comment(
        &self,
        params: Parameters<UpdateCommentParams>,
    ) -> Result<CallToolResult, McpError> {
        tools::update_comment::handle_update_comment(self.store.clone(), params).await
    }

    /// Add a comment to a task.
    #[tool(description = "Add a comment to a task")]
    async fn add_comment(
        &self,
        params: Parameters<AddCommentParams>,
    ) -> Result<CallToolResult, McpError> {
        tools::add_comment::handle_add_comment(self.store.clone(), self.workflow.clone(), params).await
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
