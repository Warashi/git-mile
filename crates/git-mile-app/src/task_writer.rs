//! Shared task mutation service used by CLI/TUI/MCP surfaces.

use anyhow::Error;
use git2::Oid;
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_store_git::GitStore;
use tokio::sync::MutexGuard;

use crate::config::WorkflowConfig;

pub use crate::task_patch::{diff_sets, DescriptionPatch, SetDiff, StatePatch, TaskUpdate};

/// Minimal storage abstraction required by [`TaskWriter`].
pub trait TaskStore {
    /// Error type bubbled up from the backing store.
    type Error: Into<Error>;

    /// Check if a task exists without loading its events.
    ///
    /// # Errors
    /// Returns a store-specific error when the check fails.
    fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error>;

    /// Append a single event for the target task.
    ///
    /// # Errors
    /// Returns a store-specific error when persisting the event fails.
    fn append_event(&self, event: &Event) -> Result<Oid, Self::Error>;

    /// Load every event for the given task.
    ///
    /// # Errors
    /// Returns a store-specific error when the task cannot be read.
    fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error>;

    /// Enumerate all known task identifiers.
    ///
    /// # Errors
    /// Returns a store-specific error when listing fails.
    fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error>;

    /// List task IDs that have been modified since the given timestamp.
    ///
    /// # Errors
    /// Returns a store-specific error when the query fails.
    fn list_tasks_modified_since(&self, since: time::OffsetDateTime) -> Result<Vec<TaskId>, Self::Error>;
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
        match self.store.task_exists(task) {
            Ok(true) => Ok(()),
            Ok(false) => Err(TaskWriteError::MissingTask(task)),
            Err(e) => Err(Self::store_error(e)),
        }
    }
}

impl<S> TaskWriter<S>
where
    S: TaskStore,
{
    /// Create a new task with optional parents.
    ///
    /// # Errors
    /// Returns [`TaskWriteError`] when validation fails or the store cannot persist events.
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
    ///
    /// # Errors
    /// Returns [`TaskWriteError`] when the task is missing, validation fails, or storage errors occur.
    pub fn update_task(
        &self,
        task: TaskId,
        patch: TaskUpdate,
        actor: &Actor,
    ) -> Result<TaskWriteResult, TaskWriteError> {
        self.ensure_task_exists(task)?;

        let TaskUpdate {
            title,
            state,
            description,
            labels,
            assignees,
        } = patch;

        let mut events = Vec::new();

        if let Some(title) = title {
            events.push(Event::new(task, actor, EventKind::TaskTitleSet { title }));
        }

        if let Some(state_patch) = state {
            match state_patch {
                StatePatch::Set { state } => {
                    self.validate_state(Some(&state))?;
                    let state_kind = self.workflow.resolve_state_kind(Some(&state));
                    events.push(Event::new(
                        task,
                        actor,
                        EventKind::TaskStateSet { state, state_kind },
                    ));
                }
                StatePatch::Clear => {
                    events.push(Event::new(task, actor, EventKind::TaskStateCleared));
                }
            }
        }

        if let Some(description_patch) = description {
            match description_patch {
                DescriptionPatch::Set { description } => {
                    events.push(Event::new(
                        task,
                        actor,
                        EventKind::TaskDescriptionSet {
                            description: Some(description),
                        },
                    ));
                }
                DescriptionPatch::Clear => {
                    events.push(Event::new(
                        task,
                        actor,
                        EventKind::TaskDescriptionSet { description: None },
                    ));
                }
            }
        }

        let SetDiff {
            added: label_added,
            removed: label_removed,
        } = labels;
        if !label_added.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::LabelsAdded { labels: label_added },
            ));
        }
        if !label_removed.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::LabelsRemoved {
                    labels: label_removed,
                },
            ));
        }

        let SetDiff {
            added: assignees_added,
            removed: assignees_removed,
        } = assignees;
        if !assignees_added.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::AssigneesAdded {
                    assignees: assignees_added,
                },
            ));
        }
        if !assignees_removed.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::AssigneesRemoved {
                    assignees: assignees_removed,
                },
            ));
        }

        let mut oids = Vec::new();
        for event in events {
            let oid = self.store.append_event(&event).map_err(Self::store_error)?;
            oids.push(oid);
        }

        Ok(TaskWriteResult {
            task,
            events: oids,
            comment_id: None,
        })
    }

    /// Only mutate the workflow state of a task.
    ///
    /// # Errors
    /// Returns [`TaskWriteError`] when the task is missing, the new state is invalid, or storage fails.
    pub fn set_state(
        &self,
        task: TaskId,
        state: Option<String>,
        actor: &Actor,
    ) -> Result<TaskWriteResult, TaskWriteError> {
        let state_patch = state.map_or(Some(StatePatch::Clear), |value| {
            Some(StatePatch::Set { state: value })
        });
        let patch = TaskUpdate {
            state: state_patch,
            ..TaskUpdate::default()
        };
        self.update_task(task, patch, actor)
    }

    /// Append a Markdown comment to the task.
    ///
    /// # Errors
    /// Returns [`TaskWriteError`] when the task is missing or events cannot be persisted.
    pub fn add_comment(
        &self,
        task: TaskId,
        comment: CommentRequest,
    ) -> Result<TaskWriteResult, TaskWriteError> {
        let CommentRequest { body_md, actor } = comment;
        self.ensure_task_exists(task)?;

        let comment_id = EventId::new();
        let event = Event::new(task, &actor, EventKind::CommentAdded { comment_id, body_md });
        let oid = self.store.append_event(&event).map_err(Self::store_error)?;

        Ok(TaskWriteResult {
            task,
            events: vec![oid],
            comment_id: Some(comment_id),
        })
    }

    /// Link new parents to the task.
    ///
    /// # Errors
    /// Returns [`TaskWriteError`] when either task is missing or persistence fails.
    pub fn link_parents(
        &self,
        task: TaskId,
        parents: &[TaskId],
        actor: &Actor,
    ) -> Result<TaskWriteResult, TaskWriteError> {
        self.ensure_task_exists(task)?;

        let mut events = Vec::new();
        for parent in parents {
            match self.ensure_task_exists(*parent) {
                Ok(()) => {}
                Err(TaskWriteError::MissingTask(_)) => return Err(TaskWriteError::MissingParent(*parent)),
                Err(err) => return Err(err),
            }

            let child_event = Event::new(
                task,
                actor,
                EventKind::ChildLinked {
                    parent: *parent,
                    child: task,
                },
            );
            events.push(self.store.append_event(&child_event).map_err(Self::store_error)?);

            let parent_event = Event::new(
                *parent,
                actor,
                EventKind::ChildLinked {
                    parent: *parent,
                    child: task,
                },
            );
            events.push(
                self.store
                    .append_event(&parent_event)
                    .map_err(Self::store_error)?,
            );
        }

        Ok(TaskWriteResult {
            task,
            events,
            comment_id: None,
        })
    }

    /// Remove existing parent links from the task.
    ///
    /// # Errors
    /// Returns [`TaskWriteError`] when the task is missing or persistence fails.
    pub fn unlink_parents(
        &self,
        task: TaskId,
        parents: &[TaskId],
        actor: &Actor,
    ) -> Result<TaskWriteResult, TaskWriteError> {
        self.ensure_task_exists(task)?;

        let mut events = Vec::new();
        for parent in parents {
            let child_event = Event::new(
                task,
                actor,
                EventKind::ChildUnlinked {
                    parent: *parent,
                    child: task,
                },
            );
            events.push(self.store.append_event(&child_event).map_err(Self::store_error)?);

            let parent_event = Event::new(
                *parent,
                actor,
                EventKind::ChildUnlinked {
                    parent: *parent,
                    child: task,
                },
            );
            events.push(
                self.store
                    .append_event(&parent_event)
                    .map_err(Self::store_error)?,
            );
        }

        Ok(TaskWriteResult {
            task,
            events,
            comment_id: None,
        })
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
    /// Comment identifier (when applicable).
    pub comment_id: Option<EventId>,
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

    fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error> {
        Self::task_exists(self, task)
    }

    fn append_event(&self, event: &Event) -> Result<Oid, Self::Error> {
        Self::append_event(self, event)
    }

    fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
        Self::load_events(self, task)
    }

    fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
        Self::list_tasks(self)
    }

    fn list_tasks_modified_since(&self, since: time::OffsetDateTime) -> Result<Vec<TaskId>, Self::Error> {
        Self::list_tasks_modified_since(self, since)
    }
}

impl<S> TaskStore for &S
where
    S: TaskStore + ?Sized,
{
    type Error = S::Error;

    fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error> {
        (*self).task_exists(task)
    }

    fn append_event(&self, event: &Event) -> Result<Oid, Self::Error> {
        (*self).append_event(event)
    }

    fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
        (*self).load_events(task)
    }

    fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
        (*self).list_tasks()
    }

    fn list_tasks_modified_since(&self, since: time::OffsetDateTime) -> Result<Vec<TaskId>, Self::Error> {
        (*self).list_tasks_modified_since(since)
    }
}

impl TaskStore for MutexGuard<'_, GitStore> {
    type Error = Error;

    fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error> {
        GitStore::task_exists(self, task)
    }

    fn append_event(&self, event: &Event) -> Result<Oid, Self::Error> {
        GitStore::append_event(self, event)
    }

    fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
        GitStore::load_events(self, task)
    }

    fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
        GitStore::list_tasks(self)
    }

    fn list_tasks_modified_since(&self, since: time::OffsetDateTime) -> Result<Vec<TaskId>, Self::Error> {
        GitStore::list_tasks_modified_since(self, since)
    }
}

impl<S> TaskStore for std::sync::Arc<S>
where
    S: TaskStore,
{
    type Error = S::Error;

    fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error> {
        (**self).task_exists(task)
    }

    fn append_event(&self, event: &Event) -> Result<Oid, Self::Error> {
        (**self).append_event(event)
    }

    fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
        (**self).load_events(task)
    }

    fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
        (**self).list_tasks()
    }

    fn list_tasks_modified_since(&self, since: time::OffsetDateTime) -> Result<Vec<TaskId>, Self::Error> {
        (**self).list_tasks_modified_since(since)
    }
}
