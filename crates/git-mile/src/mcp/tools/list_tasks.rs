//! List tasks tool implementation.

use crate::mcp::params::ListTasksParams;
use git_mile_app::AsyncTaskRepository;
use git_mile_app::{FilterBuildError, TaskFilterBuilder};
use git_mile_core::TaskFilter;
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use rmcp::ErrorData as McpError;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResult, Content};
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;

impl ListTasksParams {
    pub(crate) fn into_filter(self) -> Result<TaskFilter, McpError> {
        let Self {
            states,
            labels,
            assignees,
            include_state_kinds,
            mut exclude_state_kinds,
            parents,
            children,
            updated_since,
            updated_until,
            text,
        } = self;

        let parent_ids = parse_task_ids_for_filter(parents, "parent")?;
        let child_ids = parse_task_ids_for_filter(children, "child")?;

        let mut builder = TaskFilterBuilder::new()
            .with_states(&states)
            .with_labels(&labels)
            .with_assignees(&assignees)
            .with_parents(&parent_ids)
            .with_children(&child_ids);

        if states.is_empty() && include_state_kinds.is_empty() && exclude_state_kinds.is_empty() {
            exclude_state_kinds.push("done".to_string());
        }

        builder = builder
            .with_state_kinds(&include_state_kinds, &exclude_state_kinds)
            .map_err(|err| map_filter_error(&err))?;
        builder = builder.with_text(text);
        builder = builder
            .with_time_range(updated_since, updated_until)
            .map_err(|err| map_filter_error(&err))?;

        builder.build().map_err(|err| map_filter_error(&err))
    }
}

fn parse_task_ids_for_filter(ids: Vec<String>, context: &str) -> Result<Vec<TaskId>, McpError> {
    ids.into_iter()
        .map(|value| {
            TaskId::from_str(value.trim())
                .map_err(|err| McpError::invalid_params(format!("Invalid {context} id: {err}"), None))
        })
        .collect()
}

fn map_filter_error(err: &FilterBuildError) -> McpError {
    McpError::invalid_params(err.to_string(), None)
}

/// List tasks with optional filters.
pub async fn handle_list_tasks(
    repository: Arc<AsyncTaskRepository<Arc<Mutex<GitStore>>>>,
    Parameters(params): Parameters<ListTasksParams>,
) -> Result<CallToolResult, McpError> {
    let filter = params.into_filter()?;

    let tasks = if filter.is_empty() {
        repository
            .list_snapshots(None)
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
    } else {
        repository
            .list_snapshots(Some(&filter))
            .await
            .map_err(|e| McpError::internal_error(e.to_string(), None))?
    };

    let json_str =
        serde_json::to_string_pretty(&tasks).map_err(|e| McpError::internal_error(e.to_string(), None))?;

    Ok(CallToolResult::success(vec![Content::text(json_str)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::StateKind;

    fn build_filter(params: ListTasksParams) -> TaskFilter {
        params
            .into_filter()
            .unwrap_or_else(|err| panic!("filter should build: {err}"))
    }

    #[test]
    fn defaults_to_excluding_done_when_no_state_filters() {
        let filter = build_filter(ListTasksParams::default());
        assert!(filter.state_kinds.exclude.contains(&StateKind::Done));
    }

    #[test]
    fn applies_default_exclude_done_even_with_other_filters() {
        let params = ListTasksParams {
            labels: vec!["type/doc".to_string()],
            ..Default::default()
        };
        let filter = build_filter(params);
        assert!(filter.state_kinds.exclude.contains(&StateKind::Done));
    }

    #[test]
    fn respects_explicit_state_kinds() {
        let params = ListTasksParams {
            include_state_kinds: vec!["done".to_string()],
            ..Default::default()
        };
        let filter = build_filter(params);
        assert!(filter.state_kinds.include.contains(&StateKind::Done));
        assert!(filter.state_kinds.exclude.is_empty());
    }

    #[test]
    fn respects_explicit_states() {
        let params = ListTasksParams {
            states: vec!["state/todo".to_string()],
            ..Default::default()
        };
        let filter = build_filter(params);
        assert!(filter.states.contains("state/todo"));
        assert!(filter.state_kinds.exclude.is_empty());
    }
}
