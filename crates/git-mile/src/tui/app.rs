use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::thread;

use anyhow::{Context, Error, Result};
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::TaskId;
use git_mile_core::{OrderedEvents, TaskFilter, TaskSnapshot};
use time::OffsetDateTime;

use super::task_cache::TaskCache;
use super::task_visibility::TaskVisibility;
use crate::config::WorkflowConfig;
use crate::task_writer::{
    CommentRequest, CreateTaskRequest, DescriptionPatch, SetDiff, StatePatch, TaskStore as CoreTaskStore,
    TaskUpdate, TaskWriter, diff_sets,
};

/// Storage abstraction marker so the TUI logic can be unit-tested.
pub(super) trait TaskStore: CoreTaskStore<Error = anyhow::Error> {}

impl<T> TaskStore for T where T: CoreTaskStore<Error = anyhow::Error> {}

/// Actor-written comment on a task.
#[derive(Debug, Clone)]
pub(super) struct TaskComment {
    /// Actor who authored the comment.
    pub actor: Actor,
    /// Comment body in Markdown.
    pub body: String,
    /// Event timestamp in UTC.
    pub ts: OffsetDateTime,
}

/// Materialized view for TUI rendering.
#[derive(Debug, Clone)]
pub(super) struct TaskView {
    /// Current snapshot derived from the CRDT.
    pub snapshot: TaskSnapshot,
    /// Chronological comment history.
    pub comments: Vec<TaskComment>,
    /// Timestamp of the most recent event.
    pub last_updated: Option<OffsetDateTime>,
}

impl TaskView {
    pub(super) fn from_events(events: &[Event]) -> Self {
        let ordered = OrderedEvents::from(events);
        let snapshot = TaskSnapshot::replay_ordered(&ordered);

        let comments = ordered
            .iter()
            .filter_map(|ev| {
                if let EventKind::CommentAdded { body_md, .. } = &ev.kind {
                    Some(TaskComment {
                        actor: ev.actor.clone(),
                        body: body_md.clone(),
                        ts: ev.ts,
                    })
                } else {
                    None
                }
            })
            .collect();

        let last_updated = ordered.latest().map(|ev| ev.ts);

        Self {
            snapshot,
            comments,
            last_updated,
        }
    }
}

/// Application state shared between the TUI event loop and rendering.
pub(super) struct App<S: TaskStore> {
    writer: TaskWriter<S>,
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
    pub(super) fn new(store: S, workflow: WorkflowConfig) -> Result<Self> {
        let writer = TaskWriter::new(store, workflow.clone());
        let mut app = Self {
            writer,
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

    pub(super) const fn store(&self) -> &S {
        self.writer.store()
    }

    pub(super) fn filter(&self) -> &TaskFilter {
        self.visibility.filter()
    }

    pub(super) fn set_filter(&mut self, filter: TaskFilter) {
        if self.visibility.filter() == &filter {
            return;
        }
        let keep_id = self.selected_task_id();
        self.visibility.set_filter(filter);
        self.visibility.rebuild(&self.tasks, keep_id);
    }

    pub(super) fn has_visible_tasks(&self) -> bool {
        self.visibility.has_visible_tasks()
    }

    pub(super) fn visible_tasks(&self) -> impl Iterator<Item = &TaskView> + '_ {
        self.visibility
            .visible_indexes()
            .iter()
            .filter_map(|&idx| self.tasks.get(idx))
    }

    pub(super) fn is_visible(&self, task_id: TaskId) -> bool {
        self.visibility.contains(task_id)
    }

    pub(super) fn selection_index(&self) -> usize {
        self.visibility.selected_index()
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

    /// Jump to a specific task by ID.
    pub(super) fn jump_to_task(&mut self, task_id: TaskId) {
        self.visibility.jump_to_task(task_id);
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
        let keep_id = preferred.or_else(|| self.selected_task().map(|view| view.snapshot.id));

        let cache = TaskCache::load(self.store()).map_err(Self::map_store_error)?;
        self.tasks = cache.tasks;
        self.task_index = cache.task_index;
        self.parents_index = cache.parents_index;
        self.children_index = cache.children_index;
        self.visibility.rebuild(&self.tasks, keep_id);
        Ok(())
    }

    /// Selected task (if any).
    pub(super) fn selected_task(&self) -> Option<&TaskView> {
        self.visibility.selected_task(&self.tasks)
    }

    /// Identifier of the selected task (if any).
    pub(super) fn selected_task_id(&self) -> Option<TaskId> {
        self.visibility.selected_task_id(&self.tasks)
    }

    #[inline]
    fn runtime_touch() {
        let _ = thread::current().id();
    }

    /// Move selection to the next task.
    pub(super) fn select_next(&mut self) {
        Self::runtime_touch();
        self.visibility.select_next();
    }

    /// Move selection to the previous task.
    pub(super) fn select_prev(&mut self) {
        Self::runtime_touch();
        self.visibility.select_prev();
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

        let patch = TaskPatch::from_snapshot(snapshot, data);
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

#[derive(Debug, Default)]
struct TaskPatch {
    title: Option<String>,
    state: Option<StatePatch>,
    description: Option<DescriptionPatch>,
    labels: SetDiff<String>,
    assignees: SetDiff<String>,
}

impl TaskPatch {
    fn from_snapshot(snapshot: &TaskSnapshot, data: NewTaskData) -> Self {
        let NewTaskData {
            title,
            state,
            labels,
            assignees,
            description,
            parent: _,
        } = data;

        let mut patch = Self::default();

        if title != snapshot.title {
            patch.title = Some(title);
        }

        patch.state = match (snapshot.state.as_ref(), state) {
            (Some(old), Some(new)) if *old != new => Some(StatePatch::Set { state: new }),
            (None, Some(new)) => Some(StatePatch::Set { state: new }),
            (Some(_), None) => Some(StatePatch::Clear),
            _ => None,
        };

        let desired_labels: BTreeSet<String> = labels.into_iter().collect();
        patch.labels = diff_sets(&snapshot.labels, &desired_labels);

        let desired_assignees: BTreeSet<String> = assignees.into_iter().collect();
        patch.assignees = diff_sets(&snapshot.assignees, &desired_assignees);

        patch.description = description.map_or_else(
            || (!snapshot.description.is_empty()).then_some(DescriptionPatch::Clear),
            |text| {
                if text.is_empty() {
                    if snapshot.description.is_empty() {
                        None
                    } else {
                        Some(DescriptionPatch::Clear)
                    }
                } else if text == snapshot.description {
                    None
                } else {
                    Some(DescriptionPatch::Set { description: text })
                }
            },
        );

        patch
    }

    const fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.state.is_none()
            && self.description.is_none()
            && self.labels.is_empty()
            && self.assignees.is_empty()
    }

    fn into_task_update(self) -> TaskUpdate {
        TaskUpdate {
            title: self.title,
            state: self.state,
            description: self.description,
            labels: self.labels,
            assignees: self.assignees,
        }
    }
}
