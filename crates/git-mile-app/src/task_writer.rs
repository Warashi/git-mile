//! Shared task mutation service used by CLI/TUI/MCP surfaces.

use anyhow::Error;
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_hooks::{HookContext, HookExecutor, HookKind, HooksConfig};
use git_mile_store_git::GitStore;
use git2::Oid;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tokio::sync::MutexGuard;

use crate::config::WorkflowConfig;

pub use crate::task_patch::{DescriptionPatch, SetDiff, StatePatch, TaskUpdate, diff_sets};

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

    /// Load events for the provided task ids in a single call.
    ///
    /// The default implementation iterates over [`load_events`](Self::load_events)
    /// for each task id and collects the results.
    ///
    /// # Errors
    /// Propagates the first error from the underlying store.
    fn load_events_for_tasks(&self, task_ids: &[TaskId]) -> Result<Vec<(TaskId, Vec<Event>)>, Self::Error> {
        let mut results = Vec::with_capacity(task_ids.len());
        for &task in task_ids {
            let events = self.load_events(task)?;
            results.push((task, events));
        }
        Ok(results)
    }

    /// Load all events for every known task.
    ///
    /// The default implementation uses [`list_tasks`](Self::list_tasks) +
    /// [`load_events`](Self::load_events). Stores can override this method to
    /// provide more efficient bulk loading.
    ///
    /// # Errors
    /// Propagates the first error from the underlying store.
    fn load_all_events(&self) -> Result<Vec<(TaskId, Vec<Event>)>, Self::Error> {
        let mut results = Vec::new();
        for task in self.list_tasks()? {
            let events = self.load_events(task)?;
            results.push((task, events));
        }
        Ok(results)
    }

    /// Invalidate cached events for the specified tasks.
    ///
    /// This is useful when external processes may have modified tasks and the cache
    /// needs to be refreshed. The default implementation does nothing, as not all
    /// stores maintain a cache.
    ///
    /// # Errors
    /// Returns a store-specific error when cache invalidation fails.
    fn invalidate_cache(&self, _task_ids: &[TaskId]) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// High-level service that validates inputs and emits task events.
pub struct TaskWriter<S> {
    store: S,
    workflow: WorkflowConfig,
    hooks_config: HooksConfig,
    base_dir: PathBuf,
}

impl<S> TaskWriter<S> {
    /// Construct a new writer.
    pub const fn new(
        store: S,
        workflow: WorkflowConfig,
        hooks_config: HooksConfig,
        base_dir: PathBuf,
    ) -> Self {
        Self {
            store,
            workflow,
            hooks_config,
            base_dir,
        }
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

struct LamportTracker<'a, S> {
    store: &'a S,
    cache: BTreeMap<TaskId, u64>,
}

impl<'a, S> LamportTracker<'a, S>
where
    S: TaskStore,
{
    #[allow(clippy::missing_const_for_fn)]
    fn new(store: &'a S) -> Self {
        Self {
            store,
            cache: BTreeMap::new(),
        }
    }

    fn assign(&mut self, event: &mut Event) -> Result<(), TaskWriteError> {
        event.lamport = self.next(event.task)?;
        Ok(())
    }

    fn next(&mut self, task: TaskId) -> Result<u64, TaskWriteError> {
        if let Some(value) = self.cache.get_mut(&task) {
            *value += 1;
            return Ok(*value);
        }

        let mut previous = 0;
        match self.store.task_exists(task) {
            Ok(true) => {
                let events = self
                    .store
                    .load_events(task)
                    .map_err(TaskWriter::<S>::store_error)?;
                previous = events.iter().map(|event| event.lamport).max().unwrap_or(0);
            }
            Ok(false) => {}
            Err(err) => return Err(TaskWriter::<S>::store_error(err)),
        }

        let next = previous + 1;
        self.cache.insert(task, next);
        Ok(next)
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

    /// Execute a pre-hook and reject the operation if it fails
    fn execute_pre_hook(&self, kind: HookKind, event: &Event) -> Result<(), TaskWriteError>
    where
        S: TaskStore,
    {
        let executor = HookExecutor::new(self.hooks_config.clone(), self.base_dir.clone());
        let context = HookContext {
            event: event.clone(),
            data: None,
        };

        match executor.execute(kind, &context) {
            Ok(result) => {
                if result.is_success() {
                    Ok(())
                } else {
                    Err(TaskWriteError::HookRejected {
                        hook: kind.script_name().to_owned(),
                        exit_code: result.exit_code,
                        stderr: result.stderr,
                    })
                }
            }
            Err(git_mile_hooks::HookError::NotFound(_)) => {
                // Hook script not found - this is not an error, just skip
                Ok(())
            }
            Err(e) => Err(TaskWriteError::HookFailed {
                hook: kind.script_name().to_owned(),
                error: e.to_string(),
            }),
        }
    }

    /// Execute a post-hook (errors are logged but don't fail the operation)
    fn execute_post_hook(&self, kind: HookKind, event: &Event)
    where
        S: TaskStore,
    {
        let executor = HookExecutor::new(self.hooks_config.clone(), self.base_dir.clone());
        let context = HookContext {
            event: event.clone(),
            data: None,
        };

        // Post-hooks should not fail the operation, so we ignore errors
        let _ = executor.execute(kind, &context);
    }

    /// Append an event to the store with hook execution
    ///
    /// This method wraps the store's `append_event` and ensures that `PreEvent` and `PostEvent`
    /// hooks are executed for all events, along with any specific hooks.
    ///
    /// # Hook Execution Order
    /// 1. `PreEvent` (global)
    /// 2. Specific pre-hook (if provided)
    /// 3. Store `append_event`
    /// 4. Specific post-hook (if provided)
    /// 5. `PostEvent` (global)
    ///
    /// # Errors
    /// Returns [`TaskWriteError`] if `PreEvent`, specific pre-hook, or store operation fails.
    fn append_event_with_hooks(
        &self,
        event: &Event,
        specific_pre_hook: Option<HookKind>,
        specific_post_hook: Option<HookKind>,
    ) -> Result<Oid, TaskWriteError>
    where
        S: TaskStore,
    {
        // 1. PreEvent (global)
        self.execute_pre_hook(HookKind::PreEvent, event)?;

        // 2. Specific pre-hook (e.g., PreTaskUpdate)
        if let Some(hook_kind) = specific_pre_hook {
            self.execute_pre_hook(hook_kind, event)?;
        }

        // 3. Persist to store
        let oid = self.store.append_event(event).map_err(Self::store_error)?;

        // 4. Specific post-hook
        if let Some(hook_kind) = specific_post_hook {
            self.execute_post_hook(hook_kind, event);
        }

        // 5. PostEvent (global)
        self.execute_post_hook(HookKind::PostEvent, event);

        Ok(oid)
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
        let mut lamports = LamportTracker::new(&self.store);
        let mut parent_links = Vec::new();

        let mut created_event = Event::new(
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
        lamports.assign(&mut created_event)?;

        let created_oid = self.append_event_with_hooks(
            &created_event,
            Some(HookKind::PreTaskCreate),
            Some(HookKind::PostTaskCreate),
        )?;
        events.push(created_oid);

        for parent in parents {
            self.ensure_task_exists(parent)
                .map_err(|_| TaskWriteError::MissingParent(parent))?;

            let mut child_event = Event::new(task, &actor, EventKind::ChildLinked { parent, child: task });
            lamports.assign(&mut child_event)?;
            let child_oid = self.append_event_with_hooks(
                &child_event,
                Some(HookKind::PreRelationChange),
                Some(HookKind::PostRelationChange),
            )?;
            events.push(child_oid);

            let mut parent_event = Event::new(parent, &actor, EventKind::ChildLinked { parent, child: task });
            lamports.assign(&mut parent_event)?;
            let parent_oid = self.append_event_with_hooks(
                &parent_event,
                Some(HookKind::PreRelationChange),
                Some(HookKind::PostRelationChange),
            )?;
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
    #[allow(clippy::too_many_lines)]
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
        let mut lamports = LamportTracker::new(&self.store);

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
        for mut event in events {
            lamports.assign(&mut event)?;

            // Determine specific hooks based on event kind
            let (pre_hook, post_hook) = match &event.kind {
                EventKind::TaskStateSet { .. } | EventKind::TaskStateCleared => {
                    // State change events get state-specific hooks
                    (Some(HookKind::PreStateChange), Some(HookKind::PostStateChange))
                }
                EventKind::TaskTitleSet { .. }
                | EventKind::TaskDescriptionSet { .. }
                | EventKind::LabelsAdded { .. }
                | EventKind::LabelsRemoved { .. }
                | EventKind::AssigneesAdded { .. }
                | EventKind::AssigneesRemoved { .. } => {
                    // Other task update events get task-update hooks
                    (Some(HookKind::PreTaskUpdate), Some(HookKind::PostTaskUpdate))
                }
                _ => (None, None),
            };

            let oid = self.append_event_with_hooks(&event, pre_hook, post_hook)?;
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
        let mut lamports = LamportTracker::new(&self.store);
        let mut event = Event::new(task, &actor, EventKind::CommentAdded { comment_id, body_md });
        lamports.assign(&mut event)?;
        let oid = self.append_event_with_hooks(
            &event,
            Some(HookKind::PreCommentAdd),
            Some(HookKind::PostCommentAdd),
        )?;

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
        let mut lamports = LamportTracker::new(&self.store);
        for parent in parents {
            match self.ensure_task_exists(*parent) {
                Ok(()) => {}
                Err(TaskWriteError::MissingTask(_)) => return Err(TaskWriteError::MissingParent(*parent)),
                Err(err) => return Err(err),
            }

            let mut child_event = Event::new(
                task,
                actor,
                EventKind::ChildLinked {
                    parent: *parent,
                    child: task,
                },
            );
            lamports.assign(&mut child_event)?;
            events.push(self.append_event_with_hooks(
                &child_event,
                Some(HookKind::PreRelationChange),
                Some(HookKind::PostRelationChange),
            )?);

            let mut parent_event = Event::new(
                *parent,
                actor,
                EventKind::ChildLinked {
                    parent: *parent,
                    child: task,
                },
            );
            lamports.assign(&mut parent_event)?;
            events.push(self.append_event_with_hooks(
                &parent_event,
                Some(HookKind::PreRelationChange),
                Some(HookKind::PostRelationChange),
            )?);
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
        let mut lamports = LamportTracker::new(&self.store);
        for parent in parents {
            let mut child_event = Event::new(
                task,
                actor,
                EventKind::ChildUnlinked {
                    parent: *parent,
                    child: task,
                },
            );
            lamports.assign(&mut child_event)?;
            events.push(self.append_event_with_hooks(
                &child_event,
                Some(HookKind::PreRelationChange),
                Some(HookKind::PostRelationChange),
            )?);

            let mut parent_event = Event::new(
                *parent,
                actor,
                EventKind::ChildUnlinked {
                    parent: *parent,
                    child: task,
                },
            );
            lamports.assign(&mut parent_event)?;
            events.push(self.append_event_with_hooks(
                &parent_event,
                Some(HookKind::PreRelationChange),
                Some(HookKind::PostRelationChange),
            )?);
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
    /// Hook rejected the operation.
    #[error("hook '{hook}' rejected operation (exit code {exit_code}): {stderr}")]
    HookRejected {
        /// Hook name
        hook: String,
        /// Exit code
        exit_code: i32,
        /// Standard error output
        stderr: String,
    },
    /// Hook execution failed.
    #[error("hook '{hook}' failed: {error}")]
    HookFailed {
        /// Hook name
        hook: String,
        /// Error message
        error: String,
    },
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

    fn load_all_events(&self) -> Result<Vec<(TaskId, Vec<Event>)>, Self::Error> {
        Self::load_all_task_events(self)
    }

    fn invalidate_cache(&self, task_ids: &[TaskId]) -> Result<(), Self::Error> {
        self.invalidate_tasks_cache(task_ids);
        Ok(())
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

    fn load_all_events(&self) -> Result<Vec<(TaskId, Vec<Event>)>, Self::Error> {
        (*self).load_all_events()
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

    fn load_all_events(&self) -> Result<Vec<(TaskId, Vec<Event>)>, Self::Error> {
        GitStore::load_all_events(self)
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

    fn load_all_events(&self) -> Result<Vec<(TaskId, Vec<Event>)>, Self::Error> {
        (**self).load_all_events()
    }
}
