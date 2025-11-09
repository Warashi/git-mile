//! Shared task mutation service used by CLI/TUI/MCP surfaces.

use anyhow::Error;
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_store_git::GitStore;
use git2::Oid;

use crate::config::WorkflowConfig;

/// Minimal storage abstraction required by [`TaskWriter`].
pub trait TaskStore {
    /// Error type bubbled up from the backing store.
    type Error: Into<Error>;

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

    /// Expose a reference to the underlying store (read-only operations).
    pub const fn store(&self) -> &S {
        &self.store
    }
}

impl<S> TaskWriter<S> {
    fn store_error(err: S::Error) -> TaskWriteError
    where
        S: TaskStore,
    {
        TaskWriteError::Store(err.into())
    }

    fn validate_state(&self, state: Option<&str>) -> Result<(), TaskWriteError>
    where
        S: TaskStore,
    {
        self.workflow
            .validate_state(state)
            .map_err(|_| TaskWriteError::InvalidState(state.unwrap_or("<none>").to_owned()))
    }

    fn ensure_task_exists(&self, task: TaskId) -> Result<(), TaskWriteError>
    where
        S: TaskStore,
    {
        self.store
            .load_events(task)
            .map(|_| ())
            .map_err(|_| TaskWriteError::MissingTask(task))
    }
}

impl<S> TaskWriter<S>
where
    S: TaskStore,
{
    /// Create a new task with optional parents.
    pub fn create_task(&self, request: CreateTaskRequest) -> Result<CreateTaskResult, TaskWriteError> {
        let CreateTaskRequest {
            title,
            mut state,
            labels,
            assignees,
            description,
            parents,
            actor,
        } = request;

        if state.is_none() {
            state = self.workflow.default_state().map(str::to_owned);
        }
        self.validate_state(state.as_deref())?;
        let state_kind = self.workflow.resolve_state_kind(state.as_deref());

        let task = TaskId::new();
        let mut events = Vec::new();
        let mut parent_links = Vec::new();

        let created_event = Event::new(
            task,
            &actor,
            EventKind::TaskCreated {
                title,
                labels,
                assignees,
                description,
                state,
                state_kind,
            },
        );
        let created_oid = self
            .store
            .append_event(&created_event)
            .map_err(Self::store_error)?;
        events.push(created_oid);

        for parent in parents {
            self.ensure_task_exists(parent)
                .map_err(|_| TaskWriteError::MissingParent(parent))?;

            let child_event = Event::new(task, &actor, EventKind::ChildLinked { parent, child: task });
            let child_oid = self.store.append_event(&child_event).map_err(Self::store_error)?;
            events.push(child_oid);

            let parent_event = Event::new(parent, &actor, EventKind::ChildLinked { parent, child: task });
            let parent_oid = self
                .store
                .append_event(&parent_event)
                .map_err(Self::store_error)?;
            events.push(parent_oid);

            parent_links.push(ParentLinkResult {
                parent,
                oid: parent_oid,
            });
        }

        Ok(CreateTaskResult {
            task,
            events,
            parent_links,
        })
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
        let CommentRequest { body_md, actor } = comment;
        self.ensure_task_exists(task)?;

        let event = Event::new(
            task,
            &actor,
            EventKind::CommentAdded {
                comment_id: EventId::new(),
                body_md,
            },
        );
        let oid = self.store.append_event(&event).map_err(Self::store_error)?;

        Ok(TaskWriteResult {
            task,
            events: vec![oid],
        })
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
    /// Parent link events appended on the parent tasks.
    pub parent_links: Vec<ParentLinkResult>,
}

/// Result returned for update/comment/link operations.
#[derive(Debug, Clone)]
pub struct TaskWriteResult {
    /// Identifier of the mutated task.
    pub task: TaskId,
    /// Event object IDs created during the operation.
    pub events: Vec<Oid>,
}

/// Parent link metadata emitted during task creation/linking.
#[derive(Debug, Clone)]
pub struct ParentLinkResult {
    /// Parent task identifier.
    pub parent: TaskId,
    /// Event ID recorded on the parent ref.
    pub oid: Oid,
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

impl TaskStore for GitStore {
    type Error = Error;

    fn append_event(&self, event: &Event) -> Result<Oid, Self::Error> {
        GitStore::append_event(self, event)
    }

    fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
        GitStore::load_events(self, task)
    }

    fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
        GitStore::list_tasks(self)
    }
}
