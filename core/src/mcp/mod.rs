use std::io;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

use crate::dag::EntityId;
use crate::error::{Error, Result};
use crate::issue::{IssueId, IssueStore};
use crate::mile::{MileId, MileStore};
use crate::model::{IssueDetails, MilestoneDetails};
use crate::query::{
    ComparisonExpr, ComparisonOp, Literal, LogicalExpr, PageCursor, QueryEngine, QueryError,
    QueryExpr, QueryRequest, QueryResponse, QuerySchema, issue_schema, milestone_schema,
    parse_sort_specs, prepare_filter,
};
use crate::repo::LockMode;
use crate::service::{IssueService, MilestoneService};
use rmcp::handler::server::tool::{cached_schema_for_type, parse_json_object};
use rmcp::model::{
    CallToolRequestMethod, CallToolRequestParam, CallToolResult, Content, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParam, ProtocolVersion, ServerCapabilities,
    ServerInfo, Tool, ToolAnnotations,
};
use rmcp::service::{QuitReason, RequestContext, ServerInitializeError};
use rmcp::transport::stdio;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler, ServiceExt};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Value, to_value};
use tokio::task::{self, JoinError};
use tokio::time;
use tracing::{error, info};

const LIST_TOOL_NAME: &str = "git_mile.list";
const SHOW_TOOL_NAME: &str = "git_mile.show";

/// Configuration for running the MCP server over stdio.
#[derive(Debug, Clone)]
pub struct StdioServerConfig {
    /// Repository root that backs the server.
    pub repo_path: PathBuf,
    /// Maximum duration allowed for the initial handshake.
    pub handshake_timeout: Duration,
    /// Optional idle timeout after which the server shuts down.
    pub idle_shutdown: Option<Duration>,
}

impl StdioServerConfig {
    pub fn new(repo_path: PathBuf) -> Self {
        Self {
            repo_path,
            handshake_timeout: Duration::from_secs(30),
            idle_shutdown: None,
        }
    }

    pub fn with_handshake_timeout(mut self, timeout: Duration) -> Self {
        self.handshake_timeout = timeout;
        self
    }

    pub fn with_idle_shutdown(mut self, timeout: Option<Duration>) -> Self {
        self.idle_shutdown = timeout;
        self
    }
}

#[derive(Clone)]
struct GitMileServer {
    repo_path: PathBuf,
}

impl GitMileServer {
    fn new(repo_path: PathBuf) -> Self {
        Self { repo_path }
    }

    async fn handle_list(
        &self,
        arguments: Option<JsonObject>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let args: ListToolArgs = parse_arguments(arguments)?;
        let repo = self.repo_path.clone();
        let payload = task::spawn_blocking(move || list_entities(repo, args))
            .await
            .map_err(map_join_error)?
            .map_err(map_error_to_mcp)?;
        let content = Content::json(payload)?;
        Ok(CallToolResult::success(vec![content]))
    }

    async fn handle_show(
        &self,
        arguments: Option<JsonObject>,
    ) -> std::result::Result<CallToolResult, McpError> {
        let args: ShowToolArgs = parse_arguments(arguments)?;
        let repo = self.repo_path.clone();
        let value = task::spawn_blocking(move || show_entity(repo, args))
            .await
            .map_err(map_join_error)?
            .map_err(map_error_to_mcp)?;
        let content = Content::json(value)?;
        Ok(CallToolResult::success(vec![content]))
    }
}

impl ServerHandler for GitMileServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2024_11_05,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "Provides git-mile listing (`git_mile.list`) and details (`git_mile.show`) tools."
                    .to_string(),
            ),
        }
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = std::result::Result<ListToolsResult, McpError>> + Send + '_
    {
        let tools = server_tools().to_vec();
        std::future::ready(Ok::<ListToolsResult, McpError>(ListToolsResult {
            tools,
            next_cursor: None,
        }))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = std::result::Result<CallToolResult, McpError>> + Send + '_
    {
        async fn dispatch(
            server: &GitMileServer,
            request: CallToolRequestParam,
        ) -> std::result::Result<CallToolResult, McpError> {
            match request.name.as_ref() {
                LIST_TOOL_NAME => server.handle_list(request.arguments).await,
                SHOW_TOOL_NAME => server.handle_show(request.arguments).await,
                _ => Err(McpError::method_not_found::<CallToolRequestMethod>()),
            }
        }

        dispatch(self, request)
    }
}

static TOOL_REGISTRY: OnceLock<Vec<Tool>> = OnceLock::new();

fn server_tools() -> &'static [Tool] {
    TOOL_REGISTRY
        .get_or_init(|| vec![build_list_tool(), build_show_tool()])
        .as_slice()
}

fn build_list_tool() -> Tool {
    Tool::new(
        LIST_TOOL_NAME,
        "List git-mile issues or milestones",
        cached_schema_for_type::<ListToolArgs>(),
    )
    .annotate(
        ToolAnnotations::with_title("List git-mile entities")
            .read_only(true)
            .idempotent(true),
    )
}

fn build_show_tool() -> Tool {
    Tool::new(
        SHOW_TOOL_NAME,
        "Show git-mile issue or milestone details",
        cached_schema_for_type::<ShowToolArgs>(),
    )
    .annotate(
        ToolAnnotations::with_title("Show git-mile entity details")
            .read_only(true)
            .idempotent(true),
    )
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ListToolArgs {
    entity: EntityKind,
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    sort: Vec<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    include_closed: bool,
}

impl Default for ListToolArgs {
    fn default() -> Self {
        Self {
            entity: EntityKind::Milestone,
            filter: None,
            sort: Vec::new(),
            limit: None,
            cursor: None,
            include_closed: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum EntityKind {
    Issue,
    Milestone,
}

#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
struct ShowToolArgs {
    entity: EntityKind,
    id: String,
}

#[derive(Serialize)]
struct ListPayload {
    items: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    next_cursor: Option<String>,
}

fn list_entities(repo_path: PathBuf, args: ListToolArgs) -> Result<ListPayload> {
    let ListToolArgs {
        entity,
        filter,
        sort,
        limit,
        cursor,
        include_closed,
    } = args;
    let schema = match entity {
        EntityKind::Milestone => milestone_schema(),
        EntityKind::Issue => issue_schema(),
    };
    let mut request =
        build_query_request(&schema, filter.as_deref(), &sort, limit, cursor.as_deref())?;
    if !include_closed {
        request = augment_with_open_filter(request, "status");
    }
    match entity {
        EntityKind::Milestone => {
            let response = execute_milestone_query(&repo_path, &schema, &request)?;
            to_list_payload(response)
        }
        EntityKind::Issue => {
            let response = execute_issue_query(&repo_path, &schema, &request)?;
            to_list_payload(response)
        }
    }
}

fn show_entity(repo_path: PathBuf, args: ShowToolArgs) -> Result<Value> {
    let ShowToolArgs { entity, id } = args;
    let entity_id = EntityId::from_str(&id)
        .map_err(|err| Error::validation(format!("invalid entity id '{id}': {err}")))?;
    match entity {
        EntityKind::Milestone => {
            let service = MilestoneService::open_with_mode(&repo_path, LockMode::Read)?;
            let mile_id: MileId = entity_id;
            let details = service.get_with_comments(&mile_id)?;
            Ok(to_value(details)?)
        }
        EntityKind::Issue => {
            let service = IssueService::open_with_mode(&repo_path, LockMode::Read)?;
            let issue_id: IssueId = entity_id;
            let details = service.get_with_comments(&issue_id)?;
            Ok(to_value(details)?)
        }
    }
}

fn execute_milestone_query(
    repo: &Path,
    schema: &QuerySchema,
    request: &QueryRequest,
) -> Result<QueryResponse<MilestoneDetails>> {
    let records = load_milestone_details(repo)?;
    let engine = QueryEngine::new(schema.clone());
    engine
        .execute(records, request, None)
        .map_err(map_query_error)
}

fn execute_issue_query(
    repo: &Path,
    schema: &QuerySchema,
    request: &QueryRequest,
) -> Result<QueryResponse<IssueDetails>> {
    let records = load_issue_details(repo)?;
    let engine = QueryEngine::new(schema.clone());
    engine
        .execute(records, request, None)
        .map_err(map_query_error)
}

fn load_milestone_details(repo: &Path) -> Result<Vec<MilestoneDetails>> {
    let store = MileStore::open_with_mode(repo, LockMode::Read)?;
    let service = MilestoneService::open_with_mode(repo, LockMode::Read)?;
    let mut details = Vec::new();
    for summary in store.list_miles()? {
        details.push(service.get_with_comments(&summary.id)?);
    }
    Ok(details)
}

fn load_issue_details(repo: &Path) -> Result<Vec<IssueDetails>> {
    let store = IssueStore::open_with_mode(repo, LockMode::Read)?;
    let service = IssueService::open_with_mode(repo, LockMode::Read)?;
    let mut details = Vec::new();
    for summary in store.list_issues()? {
        details.push(service.get_with_comments(&summary.id)?);
    }
    Ok(details)
}

fn build_query_request(
    schema: &QuerySchema,
    filter: Option<&str>,
    sort_tokens: &[String],
    limit: Option<usize>,
    cursor: Option<&str>,
) -> Result<QueryRequest> {
    let filter_expr =
        prepare_filter(schema, filter).map_err(|err| Error::validation(err.to_string()))?;
    let sort_specs =
        parse_sort_specs(sort_tokens).map_err(|err| Error::validation(err.to_string()))?;
    let cursor_value = if let Some(raw) = cursor {
        Some(PageCursor::parse(raw).map_err(|err| Error::validation(err.to_string()))?)
    } else {
        None
    };

    Ok(QueryRequest {
        filter: filter_expr,
        sort: sort_specs,
        limit,
        cursor: cursor_value,
    })
}

fn augment_with_open_filter(mut request: QueryRequest, status_field: &str) -> QueryRequest {
    let open_expr = QueryExpr::Comparison(ComparisonExpr {
        operator: ComparisonOp::NotEq,
        field: status_field.to_string(),
        values: vec![Literal::String("closed".to_string())],
    });

    request.filter = match request.filter.take() {
        Some(existing) => Some(QueryExpr::Logical(LogicalExpr::And(vec![
            existing, open_expr,
        ]))),
        None => Some(open_expr),
    };
    request
}

fn to_list_payload<T: Serialize>(response: QueryResponse<T>) -> Result<ListPayload> {
    let items = response
        .items
        .into_iter()
        .map(to_value)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(ListPayload {
        items,
        next_cursor: response.next_cursor,
    })
}

fn parse_arguments<T: DeserializeOwned>(
    arguments: Option<JsonObject>,
) -> std::result::Result<T, McpError> {
    let object = arguments.ok_or_else(|| {
        McpError::invalid_params("arguments must be provided for this tool", None)
    })?;
    parse_json_object(object)
}

fn map_error_to_mcp(err: Error) -> McpError {
    match err {
        Error::Validation(message) | Error::Conflict(message) => {
            McpError::invalid_params(message, None)
        }
        other => {
            error!(error = %other, "git-mile MCP server internal error");
            McpError::internal_error(other.to_string(), None)
        }
    }
}

fn map_query_error(err: QueryError) -> Error {
    Error::validation(err.to_string())
}

fn map_initialize_error(err: ServerInitializeError) -> Error {
    Error::Io(io::Error::other(format!(
        "failed to initialize MCP server: {err}"
    )))
}

fn handle_wait_result(result: std::result::Result<QuitReason, JoinError>) -> Result<()> {
    match result {
        Ok(QuitReason::Closed | QuitReason::Cancelled) => Ok(()),
        Ok(QuitReason::JoinError(err)) => Err(join_error_to_core(err)),
        Err(err) => Err(join_error_to_core(err)),
    }
}

fn join_error_to_core(err: JoinError) -> Error {
    Error::Io(io::Error::other(format!("MCP server task failed: {err}")))
}

fn map_join_error(err: JoinError) -> McpError {
    if err.is_panic() {
        error!("blocking task panicked: {err}");
    } else {
        error!("blocking task cancelled: {err}");
    }
    McpError::internal_error("internal server task failure", None)
}

/// Run the MCP server over stdio until the client disconnects or the idle timeout elapses.
///
/// # Errors
///
/// Returns an error when the handshake times out, initialization fails, or the server loop
/// encounters an unrecoverable error.
pub async fn run_stdio_server(config: StdioServerConfig) -> Result<()> {
    info!(
        "starting git-mile MCP stdio server (handshake_timeout={:?}, idle_timeout={:?})",
        config.handshake_timeout, config.idle_shutdown
    );

    let server = GitMileServer::new(config.repo_path.clone());
    let transport = stdio();

    let running = time::timeout(config.handshake_timeout, server.serve(transport))
        .await
        .map_err(|_| Error::validation("MCP client handshake timed out"))?
        .map_err(map_initialize_error)?;

    info!("MCP client connected; entering serving loop");

    if let Some(idle) = config.idle_shutdown {
        let cancel = running.cancellation_token();
        let waiting = running.waiting();
        tokio::pin!(waiting);
        tokio::select! {
            result = &mut waiting => handle_wait_result(result)?,
            _ = time::sleep(idle) => {
                info!("idle timeout reached; shutting down MCP server");
                cancel.cancel();
                let result = waiting.await;
                handle_wait_result(result)?;
            }
        }
    } else {
        let result = running.waiting().await;
        handle_wait_result(result)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::ReplicaId;
    use crate::issue::{CreateIssueInput, IssueStatus};
    use crate::mile::{CreateMileInput, MileStatus};
    use git2::Repository;
    use tempfile::TempDir;

    fn init_repo() -> Result<(TempDir, ReplicaId)> {
        let temp = tempfile::tempdir()?;
        Repository::init_bare(temp.path())?;
        Ok((temp, ReplicaId::new("mcp-tests")))
    }

    fn create_milestone(repo: &Path, replica: &ReplicaId) -> Result<MileId> {
        let mile_store = MileStore::open_with_mode(repo, LockMode::Write)?;
        let snapshot = mile_store.create_mile(CreateMileInput {
            replica_id: replica.clone(),
            author: "Tester <tester@example.com>".into(),
            message: Some("create milestone".into()),
            title: "Milestone Alpha".into(),
            description: Some("First milestone".into()),
            initial_status: MileStatus::Open,
            initial_comment: Some("kickoff".into()),
            labels: vec!["alpha".into()],
        })?;
        Ok(snapshot.id)
    }

    fn create_issue(repo: &Path, replica: &ReplicaId) -> Result<IssueId> {
        let issue_store = IssueStore::open_with_mode(repo, LockMode::Write)?;
        let snapshot = issue_store.create_issue(CreateIssueInput {
            replica_id: replica.clone(),
            author: "Tester <tester@example.com>".into(),
            message: Some("create issue".into()),
            title: "Issue Alpha".into(),
            description: Some("Issue details".into()),
            initial_status: IssueStatus::Open,
            initial_comment: Some("Initial comment".into()),
            labels: vec!["alpha".into()],
        })?;
        Ok(snapshot.id)
    }

    #[test]
    fn list_entities_returns_created_items() -> Result<()> {
        let (temp, replica) = init_repo()?;
        create_milestone(temp.path(), &replica)?;
        let payload = list_entities(
            temp.path().to_path_buf(),
            ListToolArgs {
                entity: EntityKind::Milestone,
                include_closed: true,
                ..ListToolArgs::default()
            },
        )?;
        assert_eq!(payload.items.len(), 1);
        assert!(payload.next_cursor.is_none());
        Ok(())
    }

    #[test]
    fn show_entity_returns_issue_details() -> Result<()> {
        let (temp, replica) = init_repo()?;
        create_milestone(temp.path(), &replica)?;
        let issue_id = create_issue(temp.path(), &replica)?;
        let value = show_entity(
            temp.path().to_path_buf(),
            ShowToolArgs {
                entity: EntityKind::Issue,
                id: issue_id.to_string(),
            },
        )?;
        let title = value
            .get("title")
            .and_then(Value::as_str)
            .expect("title present");
        assert_eq!(title, "Issue Alpha");
        Ok(())
    }

    #[test]
    fn show_entity_invalid_id_produces_error() {
        let (temp, _) = init_repo().expect("init repo");
        let result = show_entity(
            temp.path().to_path_buf(),
            ShowToolArgs {
                entity: EntityKind::Issue,
                id: "invalid-id".into(),
            },
        );
        assert!(matches!(result, Err(Error::Validation(_))));
    }
}
