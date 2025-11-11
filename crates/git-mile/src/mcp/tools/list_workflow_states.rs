//! List workflow states tool implementation.

use crate::config::WorkflowConfig;
use crate::mcp::params::{WorkflowStateEntry, WorkflowStatesResponse};
use rmcp::model::{CallToolResult, Content};
use rmcp::ErrorData as McpError;

/// List workflow states configured for this repository.
#[allow(clippy::unused_async)]
pub async fn handle_list_workflow_states(workflow: &WorkflowConfig) -> Result<CallToolResult, McpError> {
    let response = WorkflowStatesResponse {
        restricted: workflow.is_restricted(),
        default_state: workflow.default_state().map(str::to_owned),
        states: workflow
            .states()
            .iter()
            .map(|state| WorkflowStateEntry {
                value: state.value().to_owned(),
                label: state.label().map(str::to_owned),
                kind: state.kind(),
            })
            .collect(),
    };

    let json_str =
        serde_json::to_string_pretty(&response).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}
