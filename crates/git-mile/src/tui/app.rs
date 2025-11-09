use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::thread;

use anyhow::{Context, Result};
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_core::{OrderedEvents, TaskFilter, TaskSnapshot};
use time::OffsetDateTime;

use crate::config::WorkflowConfig;

/// Storage abstraction so the TUI logic can be unit-tested.
pub(super) trait TaskStore {
    /// List all known task identifiers.
    fn list_tasks(&self) -> Result<Vec<TaskId>>;
    /// Load every event for the given task.
    fn load_events(&self, task: TaskId) -> Result<Vec<Event>>;
    /// Append a single event to the backing store.
    fn append_event(&self, event: &Event) -> Result<()>;
}

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
    pub(super) store: S,
    workflow: WorkflowConfig,
    /// Cached task list sorted by最終更新順。フィルタ適用前の全体集合。
    pub tasks: Vec<TaskView>,
    /// 表示対象タスクのインデックス（`tasks` への参照）。
    visible: Vec<usize>,
    /// 現在の選択位置（`visible` のインデックス）。
    pub selected: usize,
    task_index: HashMap<TaskId, usize>,
    parents_index: HashMap<TaskId, Vec<TaskId>>,
    children_index: HashMap<TaskId, Vec<TaskId>>,
    visible_index: HashMap<TaskId, usize>,
    filter: TaskFilter,
}

impl<S: TaskStore> App<S> {
    /// Create an application instance and eagerly load tasks.
    pub(super) fn new(store: S, workflow: WorkflowConfig) -> Result<Self> {
        let mut app = Self {
            store,
            workflow,
            tasks: Vec::new(),
            visible: Vec::new(),
            selected: 0,
            task_index: HashMap::new(),
            parents_index: HashMap::new(),
            children_index: HashMap::new(),
            visible_index: HashMap::new(),
            filter: TaskFilter::default(),
        };
        app.refresh_tasks()?;
        Ok(app)
    }

    pub(super) const fn workflow(&self) -> &WorkflowConfig {
        &self.workflow
    }

    pub(super) const fn filter(&self) -> &TaskFilter {
        &self.filter
    }

    pub(super) fn set_filter(&mut self, filter: TaskFilter) {
        if self.filter == filter {
            return;
        }
        let keep_id = self.selected_task_id();
        self.filter = filter;
        self.rebuild_visibility();
        self.selected = self.resolve_selection(keep_id);
    }

    pub(super) const fn has_visible_tasks(&self) -> bool {
        !self.visible.is_empty()
    }

    pub(super) fn visible_tasks(&self) -> impl Iterator<Item = &TaskView> + '_ {
        self.visible.iter().filter_map(|&idx| self.tasks.get(idx))
    }

    fn visible_index_of(&self, task_id: TaskId) -> Option<usize> {
        self.visible_index.get(&task_id).copied()
    }

    pub(super) fn is_visible(&self, task_id: TaskId) -> bool {
        self.visible_index.contains_key(&task_id)
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
        if let Some(index) = self.visible_index_of(task_id) {
            self.selected = index;
        }
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

        let mut views = Vec::new();
        for tid in self.store.list_tasks()? {
            let events = self.store.load_events(tid)?;
            views.push(TaskView::from_events(&events));
        }
        views.sort_by(|a, b| match (a.last_updated, b.last_updated) {
            (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.snapshot.id.cmp(&b.snapshot.id),
        });

        let mut task_index = HashMap::new();
        let mut parents_index = HashMap::new();
        let mut children_index: HashMap<TaskId, Vec<TaskId>> = HashMap::new();

        for (idx, view) in views.iter().enumerate() {
            task_index.insert(view.snapshot.id, idx);
        }

        for view in &views {
            let parents: Vec<TaskId> = view.snapshot.parents.iter().copied().collect();
            for parent in &parents {
                children_index.entry(*parent).or_default().push(view.snapshot.id);
            }
            children_index.entry(view.snapshot.id).or_default();
            parents_index.insert(view.snapshot.id, parents);
        }

        self.tasks = views;
        self.task_index = task_index;
        self.parents_index = parents_index;
        self.children_index = children_index;
        self.rebuild_visibility();
        self.selected = self.resolve_selection(keep_id);
        Ok(())
    }

    fn resolve_selection(&self, preferred: Option<TaskId>) -> usize {
        if self.visible.is_empty() {
            return 0;
        }
        if let Some(id) = preferred
            && let Some(index) = self.visible_index_of(id)
        {
            return index;
        }
        self.selected.min(self.visible.len() - 1)
    }

    fn rebuild_visibility(&mut self) {
        self.visible.clear();
        self.visible_index.clear();

        if self.tasks.is_empty() {
            return;
        }

        if self.filter.is_empty() {
            for (idx, view) in self.tasks.iter().enumerate() {
                let pos = self.visible.len();
                self.visible.push(idx);
                self.visible_index.insert(view.snapshot.id, pos);
            }
            return;
        }

        for (idx, view) in self.tasks.iter().enumerate() {
            if self.filter.matches(&view.snapshot) {
                let pos = self.visible.len();
                self.visible.push(idx);
                self.visible_index.insert(view.snapshot.id, pos);
            }
        }
    }

    /// Selected task (if any).
    pub(super) fn selected_task(&self) -> Option<&TaskView> {
        self.visible
            .get(self.selected)
            .and_then(|&idx| self.tasks.get(idx))
    }

    /// Identifier of the selected task (if any).
    pub(super) fn selected_task_id(&self) -> Option<TaskId> {
        self.selected_task().map(|view| view.snapshot.id)
    }

    #[inline]
    fn runtime_touch() {
        let _ = thread::current().id();
    }

    /// Move selection to the next task.
    pub(super) fn select_next(&mut self) {
        Self::runtime_touch();
        if self.visible.is_empty() {
            return;
        }
        if self.selected + 1 < self.visible.len() {
            self.selected += 1;
        }
    }

    /// Move selection to the previous task.
    pub(super) fn select_prev(&mut self) {
        Self::runtime_touch();
        if self.visible.is_empty() {
            return;
        }
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Append a comment to the given task and refresh the view.
    pub(super) fn add_comment(&mut self, task: TaskId, body: String, actor: &Actor) -> Result<()> {
        let event = Event::new(
            task,
            actor,
            EventKind::CommentAdded {
                comment_id: EventId::new(),
                body_md: body,
            },
        );
        self.store.append_event(&event)?;
        self.refresh_tasks_with(Some(task))
    }

    /// Create a fresh task and refresh the view, returning the new identifier.
    pub(super) fn create_task(&mut self, mut data: NewTaskData, actor: &Actor) -> Result<TaskId> {
        if data.state.is_none() {
            data.state = self.workflow.default_state().map(str::to_owned);
        }
        self.workflow.validate_state(data.state.as_deref())?;
        let state_kind = self.workflow.resolve_state_kind(data.state.as_deref());

        let task = TaskId::new();
        let event = Event::new(
            task,
            actor,
            EventKind::TaskCreated {
                title: data.title,
                labels: data.labels,
                assignees: data.assignees,
                description: data.description,
                state: data.state,
                state_kind,
            },
        );
        self.store.append_event(&event)?;

        // Create ChildLinked event if parent is specified
        if let Some(parent) = data.parent {
            let link_event = Event::new(task, actor, EventKind::ChildLinked { parent, child: task });
            self.store.append_event(&link_event)?;
            let parent_event = Event::new(parent, actor, EventKind::ChildLinked { parent, child: task });
            self.store.append_event(&parent_event)?;
        }

        self.refresh_tasks_with(Some(task))?;
        Ok(task)
    }

    /// Update an existing task and refresh the view. Returns `true` when any changes were applied.
    pub(super) fn update_task(&mut self, task: TaskId, data: NewTaskData, actor: &Actor) -> Result<bool> {
        let mut loaded_snapshot = None;
        let snapshot = if let Some(view) = self.tasks.iter().find(|view| view.snapshot.id == task) {
            &view.snapshot
        } else {
            let snapshot = self
                .store
                .load_events(task)
                .map(|events| TaskSnapshot::replay(&events))
                .context("タスクの読み込みに失敗しました")?;
            let snapshot_ref: &TaskSnapshot = loaded_snapshot.insert(snapshot);
            snapshot_ref
        };

        let patch = TaskPatch::from_snapshot(snapshot, data);
        if patch.is_empty() {
            return Ok(false);
        }
        if let Some(StatePatch::Set { state }) = &patch.state {
            self.workflow.validate_state(Some(state))?;
        }

        for event in patch.into_events(task, actor, &self.workflow) {
            self.store
                .append_event(&event)
                .context("タスク更新イベントの書き込みに失敗しました")?;
        }
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
            let snapshot = self
                .store
                .load_events(task)
                .map(|events| TaskSnapshot::replay(&events))
                .context("タスクの読み込みに失敗しました")?;
            let snapshot_ref: &TaskSnapshot = loaded_snapshot.insert(snapshot);
            snapshot_ref
        };

        if snapshot.state.as_deref() == state.as_deref() {
            return Ok(false);
        }

        let event = match state {
            Some(state_value) => {
                let state_kind = self.workflow.resolve_state_kind(Some(&state_value));
                Event::new(
                    task,
                    actor,
                    EventKind::TaskStateSet {
                        state: state_value,
                        state_kind,
                    },
                )
            }
            None => Event::new(task, actor, EventKind::TaskStateCleared),
        };
        self.store
            .append_event(&event)
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

    fn into_events(self, task: TaskId, actor: &Actor, workflow: &WorkflowConfig) -> Vec<Event> {
        let mut events = Vec::new();

        if let Some(title) = self.title {
            events.push(Event::new(task, actor, EventKind::TaskTitleSet { title }));
        }

        if let Some(state_patch) = self.state {
            match state_patch {
                StatePatch::Set { state } => {
                    let state_kind = workflow.resolve_state_kind(Some(&state));
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

        if let Some(description_patch) = self.description {
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

        if !self.labels.added.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::LabelsAdded {
                    labels: self.labels.added,
                },
            ));
        }

        if !self.labels.removed.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::LabelsRemoved {
                    labels: self.labels.removed,
                },
            ));
        }

        if !self.assignees.added.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::AssigneesAdded {
                    assignees: self.assignees.added,
                },
            ));
        }

        if !self.assignees.removed.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::AssigneesRemoved {
                    assignees: self.assignees.removed,
                },
            ));
        }

        events
    }

    const fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.state.is_none()
            && self.description.is_none()
            && self.labels.added.is_empty()
            && self.labels.removed.is_empty()
            && self.assignees.added.is_empty()
            && self.assignees.removed.is_empty()
    }
}

#[derive(Debug)]
enum StatePatch {
    Set { state: String },
    Clear,
}

#[derive(Debug)]
enum DescriptionPatch {
    Set { description: String },
    Clear,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct SetDiff<T> {
    pub added: Vec<T>,
    pub removed: Vec<T>,
}

pub(super) fn diff_sets<T: Ord + Clone>(current: &BTreeSet<T>, desired: &BTreeSet<T>) -> SetDiff<T> {
    SetDiff {
        added: desired.difference(current).cloned().collect(),
        removed: current.difference(desired).cloned().collect(),
    }
}
