//! Shared task mutation service skeleton.
//!
//! NOTE: This module currently contains type definitions only. The actual logic
//! will be implemented while migrating CLI/TUI/MCP flows to this layer.

use anyhow::Error;
use git_mile_core::event::{Actor, Event};
use git_mile_core::id::TaskId;
use git2::Oid;

use crate::config::WorkflowConfig;

/// Minimal storage abstraction required by [`TaskWriter`].
pub trait TaskStore {
    /// Error type bubbled up from the backing store.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Append a single event for the target task.
    fn append_event(&self, event: &Event) -> Result<Oid, Self::Error>;

    /// Load every event for the given task.
    fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error>;

    /// Enumerate all known task identifiers.
    fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error>;
}

/// High-level service that validates inputs and emits task events.
pub struct TaskWriter<S> {
    store: S,
    workflow: WorkflowConfig,
}

impl<S> TaskWriter<S> {
    /// Construct a new writer.
    pub const fn new(store: S, workflow: WorkflowConfig) -> Self {
        Self { store, workflow }
    }

    /// Borrow the workflow configuration.
    pub const fn workflow(&self) -> &WorkflowConfig {
        &self.workflow
    }
}

impl<S> TaskWriter<S> {
    fn store_error(err: S::Error) -> TaskWriteError
    where
        S: TaskStore,
    {
        TaskWriteError::Store(Error::new(err))
    }
}

impl<S> TaskWriter<S>
where
    S: TaskStore,
{
    /// Create a new task with optional parents.
    pub fn create_task(&self, request: CreateTaskRequest) -> Result<CreateTaskResult, TaskWriteError> {
        let _ = request;
        Err(TaskWriteError::NotImplemented("create_task"))
    }

    /// Apply a patch to an existing task.
    pub fn update_task(&self, task: TaskId, patch: TaskUpdate) -> Result<TaskWriteResult, TaskWriteError> {
        let _ = (task, patch);
        Err(TaskWriteError::NotImplemented("update_task"))
    }

    /// Only mutate the workflow state of a task.
    pub fn set_state(
        &self,
        task: TaskId,
        state: Option<String>,
        actor: &Actor,
    ) -> Result<TaskWriteResult, TaskWriteError> {
        let _ = (task, state, actor);
        Err(TaskWriteError::NotImplemented("set_state"))
    }

    /// Append a Markdown comment to the task.
    pub fn add_comment(
        &self,
        task: TaskId,
        comment: CommentRequest,
    ) -> Result<TaskWriteResult, TaskWriteError> {
        let _ = (task, comment);
        Err(TaskWriteError::NotImplemented("add_comment"))
    }

    /// Link new parents to the task.
    pub fn link_parents(
        &self,
        task: TaskId,
        parents: &[TaskId],
        actor: &Actor,
    ) -> Result<TaskWriteResult, TaskWriteError> {
        let _ = (task, parents, actor);
        Err(TaskWriteError::NotImplemented("link_parents"))
    }

    /// Remove existing parent links from the task.
    pub fn unlink_parents(
        &self,
        task: TaskId,
        parents: &[TaskId],
        actor: &Actor,
    ) -> Result<TaskWriteResult, TaskWriteError> {
        let _ = (task, parents, actor);
        Err(TaskWriteError::NotImplemented("unlink_parents"))
    }
}

/// Payload used when creating a task.
#[derive(Debug, Clone)]
pub struct CreateTaskRequest {
    /// Human-readable title.
    pub title: String,
    /// Optional workflow state label.
    pub state: Option<String>,
    /// Labels to attach.
    pub labels: Vec<String>,
    /// Initial assignees.
    pub assignees: Vec<String>,
    /// Optional Markdown description.
    pub description: Option<String>,
    /// Parent tasks to link.
    pub parents: Vec<TaskId>,
    /// Actor who authored the request.
    pub actor: Actor,
}

/// Comment body payload.
#[derive(Debug, Clone)]
pub struct CommentRequest {
    /// Comment body encoded as Markdown.
    pub body_md: String,
    /// Actor who authored the comment.
    pub actor: Actor,
}

/// Aggregate task update payload.
#[derive(Debug, Clone)]
pub struct TaskUpdate {
    /// Overwrite the task title.
    pub title: Option<String>,
    /// Overwrite the description body.
    pub description: Option<String>,
    /// Set the workflow state to this value.
    pub state: Option<String>,
    /// When true, clears the workflow state.
    pub clear_state: bool,
    /// Labels to add.
    pub add_labels: Vec<String>,
    /// Labels to remove.
    pub remove_labels: Vec<String>,
    /// Assignees to add.
    pub add_assignees: Vec<String>,
    /// Assignees to remove.
    pub remove_assignees: Vec<String>,
    /// Parent tasks to link.
    pub link_parents: Vec<TaskId>,
    /// Parent tasks to unlink.
    pub unlink_parents: Vec<TaskId>,
    /// Actor who authored this update.
    pub actor: Actor,
}

/// Result returned when a task is created.
#[derive(Debug, Clone)]
pub struct CreateTaskResult {
    /// Identifier of the new task.
    pub task: TaskId,
    /// Event object IDs created during the operation.
    pub events: Vec<Oid>,
}

/// Result returned for update/comment/link operations.
#[derive(Debug, Clone)]
pub struct TaskWriteResult {
    /// Identifier of the mutated task.
    pub task: TaskId,
    /// Event object IDs created during the operation.
    pub events: Vec<Oid>,
}

/// Errors surfaced by [`TaskWriter`].
#[derive(thiserror::Error, Debug)]
pub enum TaskWriteError {
    /// Workflow state validation failed.
    #[error("workflow state '{0}' is not allowed")]
    InvalidState(String),
    /// Parent task could not be found.
    #[error("parent task {0} not found")]
    MissingParent(TaskId),
    /// Target task could not be found.
    #[error("task {0} not found")]
    MissingTask(TaskId),
    /// Backing store returned an error.
    #[error("store error: {0}")]
    Store(#[from] Error),
    /// Placeholder for unimplemented operations.
    #[error("{0} not implemented yet")]
    NotImplemented(&'static str),
}
