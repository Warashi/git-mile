use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use anyhow::{Context, Error, Result};
use git_mile_core::TaskSnapshot;
use git_mile_core::event::Actor;
use git_mile_core::id::TaskId;

use super::task_visibility::TaskVisibility;
use crate::config::WorkflowConfig;
use crate::task_cache::TaskView;
use crate::task_patch::{TaskEditData, TaskPatch};
use crate::task_repository::TaskRepository;
use crate::task_writer::{CommentRequest, CreateTaskRequest, TaskStore, TaskWriter};

/// Application state shared between the TUI event loop and rendering.
pub(super) struct App<S: TaskStore> {
    writer: TaskWriter<S>,
    repository: Arc<TaskRepository<S>>,
    workflow: WorkflowConfig,
    /// Cached task list sorted by最終更新順。フィルタ適用前の全体集合。
    pub tasks: Vec<TaskView>,
    visibility: TaskVisibility,
    task_index: HashMap<TaskId, usize>,
    parents_index: HashMap<TaskId, Vec<TaskId>>,
    children_index: HashMap<TaskId, Vec<TaskId>>,
}

impl<S: TaskStore> App<S> {
    /// Create an application instance and eagerly load tasks.
    pub(super) fn new(store: S, repository: Arc<TaskRepository<S>>, workflow: WorkflowConfig) -> Result<Self> {
        let writer = TaskWriter::new(store, workflow.clone());
        let mut app = Self {
            writer,
            repository,
            workflow,
            tasks: Vec::new(),
            visibility: TaskVisibility::default(),
            task_index: HashMap::new(),
            parents_index: HashMap::new(),
            children_index: HashMap::new(),
        };
        app.refresh_tasks()?;
        Ok(app)
    }

    #[allow(clippy::useless_conversion)]
    fn map_store_error(err: S::Error) -> Error {
        err.into()
    }

    pub(super) const fn workflow(&self) -> &WorkflowConfig {
        &self.workflow
    }

    pub(super) const fn visibility(&self) -> &TaskVisibility {
        &self.visibility
    }

    #[allow(clippy::missing_const_for_fn)]
    pub(super) fn visibility_mut(&mut self) -> &mut TaskVisibility {
        &mut self.visibility
    }

    pub(super) fn rebuild_visibility(&mut self, preferred: Option<TaskId>) {
        self.visibility.rebuild(&self.tasks, preferred);
    }

    /// Get parent tasks of the given task.
    pub(super) fn get_parents(&self, task_id: TaskId) -> Vec<&TaskView> {
        self.parents_index
            .get(&task_id)
            .into_iter()
            .flat_map(|parents| {
                parents.iter().filter_map(|parent_id| {
                    self.task_index
                        .get(parent_id)
                        .and_then(|&idx| self.tasks.get(idx))
                })
            })
            .collect()
    }

    /// Get child tasks of the given task.
    pub(super) fn get_children(&self, task_id: TaskId) -> Vec<&TaskView> {
        self.children_index
            .get(&task_id)
            .into_iter()
            .flat_map(|children| {
                children
                    .iter()
                    .filter_map(|child_id| self.task_index.get(child_id).and_then(|&idx| self.tasks.get(idx)))
            })
            .collect()
    }

    /// Get root (topmost parent) task of the given task.
    /// Returns the task itself if it has no parents.
    pub(super) fn get_root(&self, task_id: TaskId) -> Option<&TaskView> {
        let mut queue = VecDeque::from([task_id]);
        let mut visited = HashSet::new();

        while let Some(current) = queue.pop_front() {
            if !visited.insert(current) {
                continue;
            }

            match self.parents_index.get(&current) {
                Some(parents) if !parents.is_empty() => {
                    for parent in parents {
                        queue.push_back(*parent);
                    }
                }
                _ => {
                    return self.task_index.get(&current).and_then(|&idx| self.tasks.get(idx));
                }
            }
        }
        None
    }

    /// Reload tasks from the store and keep the selection in bounds.
    pub(super) fn refresh_tasks(&mut self) -> Result<()> {
        self.refresh_tasks_with(None)
    }

    fn refresh_tasks_with(&mut self, preferred: Option<TaskId>) -> Result<()> {
        let keep_id = preferred.or_else(|| self.visibility.selected_task_id(&self.tasks));

        let cache = self.repository.get_cache()?;
        self.tasks = cache.tasks;
        self.task_index = cache.task_index;
        self.parents_index = cache.parents_index;
        self.children_index = cache.children_index;
        self.visibility.rebuild(&self.tasks, keep_id);
        Ok(())
    }

    /// Append a comment to the given task and refresh the view.
    pub(super) fn add_comment(&mut self, task: TaskId, body: String, actor: &Actor) -> Result<()> {
        self.writer
            .add_comment(
                task,
                CommentRequest {
                    body_md: body,
                    actor: actor.clone(),
                },
            )
            .context("コメントの作成に失敗しました")?;
        self.refresh_tasks_with(Some(task))
    }

    /// Create a fresh task and refresh the view, returning the new identifier.
    pub(super) fn create_task(&mut self, data: NewTaskData, actor: &Actor) -> Result<TaskId> {
        let parents = data.parent.into_iter().collect();
        let result = self
            .writer
            .create_task(CreateTaskRequest {
                title: data.title,
                state: data.state,
                labels: data.labels,
                assignees: data.assignees,
                description: data.description,
                parents,
                actor: actor.clone(),
            })
            .context("タスクの作成に失敗しました")?;

        self.refresh_tasks_with(Some(result.task))?;
        Ok(result.task)
    }

    /// Update an existing task and refresh the view. Returns `true` when any changes were applied.
    pub(super) fn update_task(&mut self, task: TaskId, data: NewTaskData, actor: &Actor) -> Result<bool> {
        let mut loaded_snapshot = None;
        let snapshot = if let Some(view) = self.tasks.iter().find(|view| view.snapshot.id == task) {
            &view.snapshot
        } else {
            let events = self
                .writer
                .store()
                .load_events(task)
                .map_err(Self::map_store_error)
                .context("タスクの読み込みに失敗しました")?;
            let snapshot = TaskSnapshot::replay(&events);
            let snapshot_ref: &TaskSnapshot = loaded_snapshot.insert(snapshot);
            snapshot_ref
        };

        let patch = TaskPatch::from_snapshot(snapshot, data.into());
        if patch.is_empty() {
            return Ok(false);
        }
        let update = patch.into_task_update();

        self.writer
            .update_task(task, update, actor)
            .context("タスク更新イベントの書き込みに失敗しました")?;
        self.refresh_tasks_with(Some(task))?;
        Ok(true)
    }

    /// Update only the workflow state of an existing task.
    pub(super) fn set_task_state(
        &mut self,
        task: TaskId,
        state: Option<String>,
        actor: &Actor,
    ) -> Result<bool> {
        self.workflow.validate_state(state.as_deref())?;

        let mut loaded_snapshot = None;
        let snapshot = if let Some(view) = self.tasks.iter().find(|view| view.snapshot.id == task) {
            &view.snapshot
        } else {
            let events = self
                .writer
                .store()
                .load_events(task)
                .map_err(Self::map_store_error)
                .context("タスクの読み込みに失敗しました")?;
            let snapshot = TaskSnapshot::replay(&events);
            let snapshot_ref: &TaskSnapshot = loaded_snapshot.insert(snapshot);
            snapshot_ref
        };

        if snapshot.state.as_deref() == state.as_deref() {
            return Ok(false);
        }

        self.writer
            .set_state(task, state, actor)
            .context("タスクのステータス更新イベントの書き込みに失敗しました")?;
        self.refresh_tasks_with(Some(task))?;
        Ok(true)
    }
}

/// Input collected from the new task form.
#[derive(Debug)]
pub(super) struct NewTaskData {
    pub title: String,
    pub state: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub description: Option<String>,
    pub parent: Option<TaskId>,
}

impl From<NewTaskData> for TaskEditData {
    fn from(data: NewTaskData) -> Self {
        Self::new(
            data.title,
            data.state,
            data.labels,
            data.assignees,
            data.description,
        )
    }
}
