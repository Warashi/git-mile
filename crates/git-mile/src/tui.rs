//! Terminal UI for browsing and updating tasks.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::env;
use std::fs;
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::{StateKind, WorkflowConfig, WorkflowState};
use anyhow::{Context, Result, anyhow};
use arboard::Clipboard as ArboardClipboard;
use base64::{Engine as _, engine::general_purpose::STANDARD as Base64Standard};
use crossterm::{
    event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_core::{OrderedEvents, StateKindFilter, TaskFilter, TaskSnapshot, UpdatedFilter};
use git_mile_store_git::GitStore;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use tempfile::NamedTempFile;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use tracing::{subscriber::NoSubscriber, warn};
use unicode_segmentation::UnicodeSegmentation;

/// Storage abstraction so the TUI logic can be unit-tested.
pub trait TaskStore {
    /// List all known task identifiers.
    fn list_tasks(&self) -> Result<Vec<TaskId>>;
    /// Load every event for the given task.
    fn load_events(&self, task: TaskId) -> Result<Vec<Event>>;
    /// Append a single event to the backing store.
    fn append_event(&self, event: &Event) -> Result<()>;
}

/// Actor-written comment on a task.
#[derive(Debug, Clone)]
pub struct TaskComment {
    /// Actor who authored the comment.
    pub actor: Actor,
    /// Comment body in Markdown.
    pub body: String,
    /// Event timestamp in UTC.
    pub ts: OffsetDateTime,
}

/// Materialized view for TUI rendering.
#[derive(Debug, Clone)]
pub struct TaskView {
    /// Current snapshot derived from the CRDT.
    pub snapshot: TaskSnapshot,
    /// Chronological comment history.
    pub comments: Vec<TaskComment>,
    /// Timestamp of the most recent event.
    pub last_updated: Option<OffsetDateTime>,
}

impl TaskView {
    fn from_events(events: &[Event]) -> Self {
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
pub struct App<S: TaskStore> {
    store: S,
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
    pub fn new(store: S, workflow: WorkflowConfig) -> Result<Self> {
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

    pub const fn workflow(&self) -> &WorkflowConfig {
        &self.workflow
    }

    pub const fn filter(&self) -> &TaskFilter {
        &self.filter
    }

    pub fn set_filter(&mut self, filter: TaskFilter) {
        if self.filter == filter {
            return;
        }
        let keep_id = self.selected_task_id();
        self.filter = filter;
        self.rebuild_visibility();
        self.selected = self.resolve_selection(keep_id);
    }

    pub const fn has_visible_tasks(&self) -> bool {
        !self.visible.is_empty()
    }

    pub fn visible_tasks(&self) -> impl Iterator<Item = &TaskView> + '_ {
        self.visible.iter().filter_map(|&idx| self.tasks.get(idx))
    }

    fn visible_index_of(&self, task_id: TaskId) -> Option<usize> {
        self.visible_index.get(&task_id).copied()
    }

    fn is_visible(&self, task_id: TaskId) -> bool {
        self.visible_index.contains_key(&task_id)
    }

    /// Get parent tasks of the given task.
    pub fn get_parents(&self, task_id: TaskId) -> Vec<&TaskView> {
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
    pub fn get_children(&self, task_id: TaskId) -> Vec<&TaskView> {
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
    pub fn jump_to_task(&mut self, task_id: TaskId) {
        if let Some(index) = self.visible_index_of(task_id) {
            self.selected = index;
        }
    }

    /// Get root (topmost parent) task of the given task.
    /// Returns the task itself if it has no parents.
    pub fn get_root(&self, task_id: TaskId) -> Option<&TaskView> {
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
    pub fn refresh_tasks(&mut self) -> Result<()> {
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
    pub fn selected_task(&self) -> Option<&TaskView> {
        self.visible
            .get(self.selected)
            .and_then(|&idx| self.tasks.get(idx))
    }

    /// Identifier of the selected task (if any).
    pub fn selected_task_id(&self) -> Option<TaskId> {
        self.selected_task().map(|view| view.snapshot.id)
    }

    #[inline]
    fn runtime_touch() {
        let _ = thread::current().id();
    }

    /// Move selection to the next task.
    pub fn select_next(&mut self) {
        Self::runtime_touch();
        if self.visible.is_empty() {
            return;
        }
        if self.selected + 1 < self.visible.len() {
            self.selected += 1;
        }
    }

    /// Move selection to the previous task.
    pub fn select_prev(&mut self) {
        Self::runtime_touch();
        if self.visible.is_empty() {
            return;
        }
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Append a comment to the given task and refresh the view.
    pub fn add_comment(&mut self, task: TaskId, body: String, actor: &Actor) -> Result<()> {
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
    pub fn create_task(&mut self, mut data: NewTaskData, actor: &Actor) -> Result<TaskId> {
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
    pub fn update_task(&mut self, task: TaskId, data: NewTaskData, actor: &Actor) -> Result<bool> {
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
    pub fn set_task_state(&mut self, task: TaskId, state: Option<String>, actor: &Actor) -> Result<bool> {
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
pub struct NewTaskData {
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

        let desired_description = description.unwrap_or_default();
        if snapshot.description != desired_description {
            patch.description = Some(if desired_description.is_empty() {
                DescriptionPatch::Clear
            } else {
                DescriptionPatch::Set {
                    description: desired_description,
                }
            });
        }

        patch
    }

    fn into_events(self, task: TaskId, actor: &Actor, workflow: &WorkflowConfig) -> Vec<Event> {
        let mut events = Vec::new();

        if let Some(title) = self.title {
            events.push(Event::new(task, actor, EventKind::TaskTitleSet { title }));
        }

        if let Some(state) = self.state {
            events.push(match state {
                StatePatch::Set { state } => {
                    let state_kind = workflow.resolve_state_kind(Some(&state));
                    Event::new(task, actor, EventKind::TaskStateSet { state, state_kind })
                }
                StatePatch::Clear => Event::new(task, actor, EventKind::TaskStateCleared),
            });
        }

        if let Some(description) = self.description {
            let payload = match description {
                DescriptionPatch::Set { description } => Some(description),
                DescriptionPatch::Clear => None,
            };
            events.push(Event::new(
                task,
                actor,
                EventKind::TaskDescriptionSet { description: payload },
            ));
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
struct SetDiff<T> {
    added: Vec<T>,
    removed: Vec<T>,
}

fn diff_sets<T: Ord + Clone>(current: &BTreeSet<T>, desired: &BTreeSet<T>) -> SetDiff<T> {
    SetDiff {
        added: desired.difference(current).cloned().collect(),
        removed: current.difference(desired).cloned().collect(),
    }
}

/// Launch the interactive TUI.
pub fn run(store: GitStore, workflow: WorkflowConfig) -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let result = tracing::subscriber::with_default(NoSubscriber::default(), || {
        run_event_loop(&mut terminal, store, workflow)
    });

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    store: GitStore,
    workflow: WorkflowConfig,
) -> Result<()> {
    let actor = resolve_actor();
    let app = App::new(store, workflow)?;
    let mut ui = Ui::new(app, actor);

    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(200);

    loop {
        terminal.draw(|f| ui.draw(f))?;
        if ui.should_quit {
            break;
        }

        let timeout = tick_rate.checked_sub(last_tick.elapsed()).unwrap_or_default();

        if event::poll(timeout)? {
            let evt = event::read()?;
            if let CrosstermEvent::Key(key) = evt {
                if let Some(action) = ui.handle_key(key)?
                    && let Err(err) = handle_ui_action(terminal, &mut ui, action)
                {
                    ui.error(format!("エディタ処理中に失敗しました: {err}"));
                }
            } else {
                // リサイズやその他のイベントは次の描画サイクルで自然に反映される。
            }
        }

        if last_tick.elapsed() >= tick_rate {
            ui.tick();
            last_tick = Instant::now();
        }
    }

    Ok(())
}

/// Tree node for hierarchical task display.
#[derive(Debug, Clone)]
struct TreeNode {
    /// Task ID.
    task_id: TaskId,
    /// Child nodes.
    children: Vec<TreeNode>,
    /// Whether this node is expanded.
    expanded: bool,
}

/// State for tree view navigation.
#[derive(Debug, Clone)]
struct TreeViewState {
    /// Root nodes of the tree.
    roots: Vec<TreeNode>,
    /// Flattened list of visible nodes (for navigation).
    visible_nodes: Vec<(usize, TaskId)>, // (depth, task_id)
    /// Currently selected index in `visible_nodes`.
    selected: usize,
}

impl TreeViewState {
    const fn new() -> Self {
        Self {
            roots: Vec::new(),
            visible_nodes: Vec::new(),
            selected: 0,
        }
    }

    /// Rebuild visible nodes list from roots.
    fn rebuild_visible_nodes(&mut self) {
        self.visible_nodes.clear();
        for i in 0..self.roots.len() {
            Self::collect_visible_nodes_into(&self.roots[i], 0, &mut self.visible_nodes);
        }
    }

    /// Expand ancestors so the given task is visible.
    fn expand_to_task(&mut self, task_id: TaskId) {
        for root in &mut self.roots {
            if Self::expand_path_to_task(root, task_id) {
                break;
            }
        }
    }

    /// Recursively collect visible nodes into a vector.
    fn collect_visible_nodes_into(node: &TreeNode, depth: usize, visible_nodes: &mut Vec<(usize, TaskId)>) {
        visible_nodes.push((depth, node.task_id));
        if node.expanded {
            for child in &node.children {
                Self::collect_visible_nodes_into(child, depth + 1, visible_nodes);
            }
        }
    }

    /// Expand nodes along the path to the target task.
    fn expand_path_to_task(node: &mut TreeNode, task_id: TaskId) -> bool {
        if node.task_id == task_id {
            return true;
        }
        for child in &mut node.children {
            if Self::expand_path_to_task(child, task_id) {
                node.expanded = true;
                return true;
            }
        }
        false
    }

    /// Get currently selected task ID.
    #[allow(dead_code)]
    fn selected_task_id(&self) -> Option<TaskId> {
        self.visible_nodes.get(self.selected).map(|(_, id)| *id)
    }

    /// Find node by task ID (mutable).
    #[allow(dead_code)]
    fn find_node_mut(&mut self, task_id: TaskId) -> Option<&mut TreeNode> {
        for root in &mut self.roots {
            if let Some(node) = Self::find_node_in_tree_mut(root, task_id) {
                return Some(node);
            }
        }
        None
    }

    #[allow(dead_code)]
    fn find_node_in_tree_mut(node: &mut TreeNode, task_id: TaskId) -> Option<&mut TreeNode> {
        if node.task_id == task_id {
            return Some(node);
        }
        for child in &mut node.children {
            if let Some(found) = Self::find_node_in_tree_mut(child, task_id) {
                return Some(found);
            }
        }
        None
    }
}

#[derive(Debug, Clone)]
struct StatePickerOption {
    value: Option<String>,
}

impl StatePickerOption {
    const fn new(value: Option<String>) -> Self {
        Self { value }
    }

    fn matches(&self, other: Option<&str>) -> bool {
        match (&self.value, other) {
            (None, None) => true,
            (Some(left), Some(right)) => left == right,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
struct CommentViewerState {
    task_id: TaskId,
    scroll_offset: u16,
}

struct StatePickerState {
    task_id: TaskId,
    options: Vec<StatePickerOption>,
    selected: usize,
}

/// Focus state for detail view components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailFocus {
    /// No focus (browsing task list).
    None,
    /// Focus on tree view (floating window).
    TreeView,
    /// Focus on state picker popup.
    StatePicker,
    /// Focus on comment viewer popup.
    CommentViewer,
}

trait ClipboardSink {
    fn set_text(&mut self, text: &str) -> Result<()>;
}

struct SystemClipboard {
    inner: ArboardClipboard,
}

impl SystemClipboard {
    fn new() -> Result<Self> {
        let inner = ArboardClipboard::new().context("クリップボードの初期化に失敗しました")?;
        Ok(Self { inner })
    }
}

impl ClipboardSink for SystemClipboard {
    fn set_text(&mut self, text: &str) -> Result<()> {
        self.inner
            .set_text(text.to_string())
            .context("クリップボードへの書き込みに失敗しました")
    }
}

struct Osc52Clipboard;

impl ClipboardSink for Osc52Clipboard {
    fn set_text(&mut self, text: &str) -> Result<()> {
        let sequence = osc52_sequence(text);
        let mut stdout = io::stdout().lock();
        stdout
            .write_all(sequence.as_bytes())
            .context("OSC 52 シーケンスの送信に失敗しました")?;
        stdout
            .flush()
            .context("OSC 52 シーケンス送信後のフラッシュに失敗しました")?;
        Ok(())
    }
}

fn osc52_sequence(text: &str) -> String {
    let encoded = Base64Standard.encode(text);
    format!("\x1b]52;c;{encoded}\x07")
}

fn default_clipboard() -> Box<dyn ClipboardSink> {
    match SystemClipboard::new() {
        Ok(cb) => Box::new(cb),
        Err(err) => {
            warn!("システムクリップボードに接続できませんでした: {err}. OSC52へフォールバックします");
            Box::new(Osc52Clipboard)
        }
    }
}

fn truncate_with_ellipsis(input: &str, max_graphemes: usize) -> Cow<'_, str> {
    const ELLIPSIS: &str = "...";
    const ELLIPSIS_GRAPHEMES: usize = 3;

    if max_graphemes == 0 {
        return Cow::Owned(String::new());
    }

    let grapheme_count = UnicodeSegmentation::graphemes(input, true).count();
    if grapheme_count <= max_graphemes {
        return Cow::Borrowed(input);
    }

    if max_graphemes <= ELLIPSIS_GRAPHEMES {
        let truncated: String = UnicodeSegmentation::graphemes(input, true)
            .take(max_graphemes)
            .collect();
        return Cow::Owned(truncated);
    }

    let keep = max_graphemes - ELLIPSIS_GRAPHEMES;
    let mut truncated: String = UnicodeSegmentation::graphemes(input, true).take(keep).collect();
    truncated.push_str(ELLIPSIS);
    Cow::Owned(truncated)
}

struct Ui<S: TaskStore> {
    app: App<S>,
    actor: Actor,
    message: Option<Message>,
    should_quit: bool,
    /// Current focus in detail view.
    detail_focus: DetailFocus,
    /// Tree view state.
    tree_state: TreeViewState,
    /// State picker popup state.
    state_picker: Option<StatePickerState>,
    /// Comment viewer popup state.
    comment_viewer: Option<CommentViewerState>,
    clipboard: Box<dyn ClipboardSink>,
}

impl<S: TaskStore> Ui<S> {
    const MAIN_MIN_HEIGHT: u16 = 5;
    const INSTRUCTIONS_HEIGHT: u16 = 3;
    const FILTER_HEIGHT: u16 = 3;
    const STATUS_MESSAGE_MIN_HEIGHT: u16 = 3;
    const STATUS_FOOTER_MIN_HEIGHT: u16 =
        Self::INSTRUCTIONS_HEIGHT + Self::FILTER_HEIGHT + Self::STATUS_MESSAGE_MIN_HEIGHT;

    fn new(app: App<S>, actor: Actor) -> Self {
        let clipboard = default_clipboard();
        Self::with_clipboard(app, actor, clipboard)
    }

    fn with_clipboard(app: App<S>, actor: Actor, clipboard: Box<dyn ClipboardSink>) -> Self {
        let _ = thread::current().id();
        let mut ui = Self {
            app,
            actor,
            message: None,
            should_quit: false,
            detail_focus: DetailFocus::None,
            tree_state: TreeViewState::new(),
            state_picker: None,
            comment_viewer: None,
            clipboard,
        };
        ui.apply_default_filter();
        ui
    }

    fn apply_default_filter(&mut self) {
        if self.app.filter().is_empty() {
            let mut filter = TaskFilter::default();
            filter.state_kinds.exclude.insert(StateKind::Done);
            self.app.set_filter(filter);
        }
    }

    fn draw(&self, f: &mut ratatui::Frame<'_>) {
        let size = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(Self::MAIN_MIN_HEIGHT),
                Constraint::Length(Self::STATUS_FOOTER_MIN_HEIGHT),
            ])
            .split(size);

        self.draw_main(f, chunks[0]);
        self.draw_status(f, chunks[1]);

        // Draw overlays on top if active
        match self.detail_focus {
            DetailFocus::TreeView => self.draw_tree_view_popup(f),
            DetailFocus::StatePicker => self.draw_state_picker_popup(f),
            DetailFocus::CommentViewer => self.draw_comment_viewer_popup(f),
            DetailFocus::None => {}
        }
    }

    fn draw_main(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
            .split(area);

        self.draw_task_list(f, columns[0]);

        let details = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
            .split(columns[1]);

        self.draw_task_details(f, details[0]);
        self.draw_comments(f, details[1]);
    }

    fn draw_task_list(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let items = if self.app.has_visible_tasks() {
            let workflow = self.app.workflow();
            self.app
                .visible_tasks()
                .map(|view| {
                    let title = Span::styled(
                        &view.snapshot.title,
                        Style::default().add_modifier(Modifier::BOLD),
                    );
                    let state_value = view.snapshot.state.as_deref();
                    let state_label = workflow.display_label(state_value);
                    let meta = format!("{} | {}", view.snapshot.id, state_label);
                    let meta_span = Span::styled(meta, Style::default().fg(Color::DarkGray));
                    ListItem::new(vec![Line::from(vec![title]), Line::from(vec![meta_span])])
                })
                .collect()
        } else {
            let message = if self.app.filter().is_empty() {
                "タスクがありません"
            } else {
                "フィルタに一致するタスクがありません"
            };
            vec![ListItem::new(Line::from(message))]
        };

        let list = List::new(items)
            .block(Block::default().title("タスクリスト").borders(Borders::ALL))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ");
        let mut state = ListState::default();
        if self.app.has_visible_tasks() {
            state.select(Some(self.app.selected));
        }
        f.render_stateful_widget(list, area, &mut state);
    }

    fn draw_task_details(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        if let Some(task) = self.app.selected_task() {
            // Split into breadcrumb area, main details, and subtasks
            let has_parents = !task.snapshot.parents.is_empty();
            let children = self.app.get_children(task.snapshot.id);
            let has_children = !children.is_empty();

            let mut constraints = Vec::new();
            if has_parents {
                constraints.push(Constraint::Length(3)); // Breadcrumb
            }
            constraints.push(Constraint::Min(5)); // Main details
            if has_children {
                #[allow(clippy::cast_possible_truncation)]
                let height = (children.len() as u16).min(10) + 2;
                constraints.push(Constraint::Length(height)); // Subtasks
            }

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(constraints)
                .split(area);

            let mut chunk_idx = 0;

            // Draw breadcrumb if parents exist
            if has_parents {
                self.draw_breadcrumb(f, chunks[chunk_idx], task.snapshot.id);
                chunk_idx += 1;
            }

            // Draw main task details
            self.draw_main_task_details(f, chunks[chunk_idx], task);
            chunk_idx += 1;

            // Draw subtasks if children exist
            if has_children {
                self.draw_subtasks(f, chunks[chunk_idx], task.snapshot.id, &children);
            }
        } else {
            let block = Block::default().title("詳細").borders(Borders::ALL);
            let inner = block.inner(area);
            f.render_widget(block, area);
            let paragraph = Paragraph::new("タスクが選択されていません").wrap(Wrap { trim: false });
            f.render_widget(paragraph, inner);
        }
    }

    fn draw_breadcrumb(&self, f: &mut ratatui::Frame<'_>, area: Rect, task_id: TaskId) {
        let parents = self.app.get_parents(task_id);
        let mut breadcrumb_items: Vec<Span<'_>> = Vec::new();

        // Add "Home" as the first breadcrumb item
        breadcrumb_items.push(Span::raw("Home"));

        // Add parent tasks
        for parent in &parents {
            breadcrumb_items.push(Span::raw(" > "));
            let parent_title = truncate_with_ellipsis(parent.snapshot.title.as_str(), 20);
            breadcrumb_items.push(Span::raw(parent_title));
        }

        breadcrumb_items.push(Span::raw(" > "));
        breadcrumb_items.push(Span::styled("現在", Style::default().fg(Color::Cyan)));

        let line = Line::from(breadcrumb_items);
        let paragraph = Paragraph::new(line)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: false });
        f.render_widget(paragraph, area);
    }

    fn draw_main_task_details(&self, f: &mut ratatui::Frame<'_>, area: Rect, task: &TaskView) {
        let block = Block::default().title("詳細").borders(Borders::ALL);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let workflow = self.app.workflow();
        let mut lines = Vec::new();
        lines.push(Line::from(Span::styled(
            &task.snapshot.title,
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
        )));
        lines.push(Line::from(format!("ID: {}", task.snapshot.id)));
        let state_value = task.snapshot.state.as_deref();
        let state_label = workflow.display_label(state_value);
        lines.push(Line::from(format!("状態: {state_label}")));
        if !task.snapshot.labels.is_empty() {
            let labels = task
                .snapshot
                .labels
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(Line::from(format!("ラベル: {labels}")));
        }
        if !task.snapshot.assignees.is_empty() {
            let assignees = task
                .snapshot
                .assignees
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            lines.push(Line::from(format!("担当者: {assignees}")));
        }

        // Show parent info
        if !task.snapshot.parents.is_empty() {
            let parents = self.app.get_parents(task.snapshot.id);
            let parent_info = if parents.is_empty() {
                format!("親: {} 件（未読込）", task.snapshot.parents.len())
            } else {
                let parent_titles: Vec<Cow<'_, str>> = parents
                    .iter()
                    .map(|p| truncate_with_ellipsis(p.snapshot.title.as_str(), 15))
                    .collect();
                let joined = parent_titles
                    .iter()
                    .map(Cow::as_ref)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("親: {joined}")
            };
            lines.push(Line::from(parent_info));
        }

        // Show children count using parent relationships
        let child_count = self.app.get_children(task.snapshot.id).len();
        if child_count > 0 {
            lines.push(Line::from(format!("子タスク: {child_count} 件")));
        }

        if let Some(updated) = task.last_updated {
            lines.push(Line::from(format!("更新: {updated}")));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "説明:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        if task.snapshot.description.is_empty() {
            lines.push(Line::from(Span::styled(
                "説明はまだありません。",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            for line in task.snapshot.description.lines() {
                lines.push(Line::from(line.to_owned()));
            }
        }

        let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
        f.render_widget(paragraph, inner);
    }

    fn draw_subtasks(
        &self,
        f: &mut ratatui::Frame<'_>,
        area: Rect,
        _task_id: TaskId,
        children: &[&TaskView],
    ) {
        let workflow = self.app.workflow();
        let items: Vec<ListItem<'_>> = children
            .iter()
            .map(|child| {
                let state_value = child.snapshot.state.as_deref();
                let state_marker = state_kind_marker(child.snapshot.state_kind);
                let state_label = workflow.display_label(state_value);

                let text = format!("▸ {} [{}]{}", child.snapshot.title, state_label, state_marker);

                ListItem::new(text)
            })
            .collect();

        let title = format!("子タスク ({})", children.len());
        let list = List::new(items).block(Block::default().title(title).borders(Borders::ALL));

        f.render_widget(list, area);
    }

    fn draw_comments(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let block = Block::default().title("コメント").borders(Borders::ALL);
        let inner = block.inner(area);
        f.render_widget(block, area);

        if let Some(task) = self.app.selected_task() {
            if task.comments.is_empty() {
                let paragraph =
                    Paragraph::new("コメントはまだありません。").style(Style::default().fg(Color::DarkGray));
                f.render_widget(paragraph, inner);
            } else {
                let mut lines = Vec::new();
                for comment in &task.comments {
                    let header = format!(
                        "{} <{}> [{}]",
                        comment.actor.name, comment.actor.email, comment.ts
                    );
                    lines.push(Line::from(Span::styled(
                        header,
                        Style::default().fg(Color::Yellow),
                    )));
                    for body_line in comment.body.lines() {
                        lines.push(Line::from(body_line.to_owned()));
                    }
                    lines.push(Line::from(""));
                }
                let paragraph = Paragraph::new(lines).wrap(Wrap { trim: false });
                f.render_widget(paragraph, inner);
            }
        } else {
            let paragraph = Paragraph::new("コメントを表示するにはタスクを選択してください。")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(paragraph, inner);
        }
    }

    fn draw_status(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(Self::status_layout_constraints())
            .split(area);

        let instructions = Paragraph::new(self.instructions())
            .block(Block::default().title("操作").borders(Borders::ALL))
            .wrap(Wrap { trim: true });
        f.render_widget(instructions, rows[0]);

        let filter = Paragraph::new(self.filter_summary_text())
            .block(Block::default().title("フィルタ").borders(Borders::ALL))
            .wrap(Wrap { trim: true });
        f.render_widget(filter, rows[1]);

        let message = Paragraph::new(self.status_text())
            .block(Block::default().title("ステータス").borders(Borders::ALL))
            .style(self.status_style());
        f.render_widget(message, rows[2]);
    }

    const fn status_layout_constraints() -> [Constraint; 3] {
        [
            Constraint::Length(Self::INSTRUCTIONS_HEIGHT),
            Constraint::Length(Self::FILTER_HEIGHT),
            Constraint::Min(Self::STATUS_MESSAGE_MIN_HEIGHT),
        ]
    }

    fn draw_tree_view_popup(&self, f: &mut ratatui::Frame<'_>) {
        let area = f.area();

        // Create centered popup (80% of screen)
        let popup_width = (area.width * 80) / 100;
        let popup_height = (area.height * 80) / 100;
        let popup_x = (area.width - popup_width) / 2;
        let popup_y = (area.height - popup_height) / 2;

        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        // Clear background
        let block = Block::default()
            .title("タスクツリー")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(Color::Black));

        f.render_widget(Clear, popup_area);
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        // Draw tree content
        self.draw_tree_content(f, inner);
    }

    fn draw_tree_content(&self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let mut lines = Vec::new();
        let workflow = self.app.workflow();

        for (i, (depth, task_id)) in self.tree_state.visible_nodes.iter().enumerate() {
            let is_selected = i == self.tree_state.selected;

            // Find task view
            let Some(task) = self.app.tasks.iter().find(|t| t.snapshot.id == *task_id) else {
                continue;
            };

            // Build line with indentation and tree characters
            let indent = "  ".repeat(*depth);
            let children = self.app.get_children(*task_id);
            let has_children = !children.is_empty();

            // Determine tree character
            let tree_char = if has_children {
                // Check if expanded
                self.find_node_in_state(*task_id)
                    .map_or("▶", |node| if node.expanded { "▼" } else { "▶" })
            } else {
                "■"
            };

            // State marker and label
            let state_value = task.snapshot.state.as_deref();
            let state_marker = state_kind_marker(task.snapshot.state_kind);
            let state_label = workflow.display_label(state_value);

            let line_text = format!(
                "{}{} {} [{}]{}",
                indent, tree_char, task.snapshot.title, state_label, state_marker
            );

            let style = if is_selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            lines.push(Line::from(Span::styled(line_text, style)));
        }

        #[allow(clippy::cast_possible_truncation)]
        let paragraph = Paragraph::new(lines).scroll((self.tree_scroll_offset() as u16, 0));
        f.render_widget(paragraph, area);
    }

    fn draw_state_picker_popup(&self, f: &mut ratatui::Frame<'_>) {
        let Some(picker) = &self.state_picker else {
            return;
        };
        let area = f.area();

        let mut popup_width = (area.width * 2) / 5;
        popup_width = popup_width.max(30).min(area.width);
        let mut popup_height = (area.height * 3) / 5;
        popup_height = popup_height.max(6).min(area.height);
        let popup_x = area.width.saturating_sub(popup_width) / 2;
        let popup_y = area.height.saturating_sub(popup_height) / 2;
        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        let task_title = self
            .app
            .tasks
            .iter()
            .find(|view| view.snapshot.id == picker.task_id)
            .map_or("不明", |view| view.snapshot.title.as_str());

        let block = Block::default()
            .title(format!("ステータス選択: {task_title}"))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(Clear, popup_area);
        f.render_widget(block.clone(), popup_area);
        let inner = block.inner(popup_area);

        let workflow = self.app.workflow();
        let items: Vec<ListItem<'_>> = picker
            .options
            .iter()
            .map(|option| {
                let value = option.value.as_deref();
                let label = workflow.display_label(value);
                let marker = value
                    .and_then(|state_value| workflow.find_state(state_value))
                    .and_then(WorkflowState::kind)
                    .map_or("", |kind| state_kind_marker(Some(kind)));
                let text = value.map_or_else(
                    || "未設定 (stateなし)".to_string(),
                    |value| format!("{label}{marker} ({value})"),
                );
                ListItem::new(Line::from(text))
            })
            .collect();

        let mut list_state = ListState::default();
        if picker.selected < picker.options.len() {
            list_state.select(Some(picker.selected));
        }
        let list = List::new(items)
            .highlight_style(
                Style::default()
                    .bg(Color::Yellow)
                    .fg(Color::Black)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");

        f.render_stateful_widget(list, inner, &mut list_state);
    }

    fn draw_comment_viewer_popup(&self, f: &mut ratatui::Frame<'_>) {
        let Some(viewer) = &self.comment_viewer else {
            return;
        };
        let area = f.area();

        // Calculate popup size (80% width, 80% height)
        let mut popup_width = (area.width * 4) / 5;
        popup_width = popup_width.max(40).min(area.width);
        let mut popup_height = (area.height * 4) / 5;
        popup_height = popup_height.max(10).min(area.height);
        let popup_x = area.width.saturating_sub(popup_width) / 2;
        let popup_y = area.height.saturating_sub(popup_height) / 2;
        let popup_area = Rect {
            x: popup_x,
            y: popup_y,
            width: popup_width,
            height: popup_height,
        };

        let task_title = self
            .app
            .tasks
            .iter()
            .find(|view| view.snapshot.id == viewer.task_id)
            .map_or("不明", |view| view.snapshot.title.as_str());

        let block = Block::default()
            .title(format!("コメント: {task_title}"))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(Clear, popup_area);
        f.render_widget(block.clone(), popup_area);
        let inner = block.inner(popup_area);

        // Render comments
        if let Some(task) = self.app.tasks.iter().find(|view| view.snapshot.id == viewer.task_id) {
            if task.comments.is_empty() {
                let paragraph = Paragraph::new("コメントはまだありません。")
                    .style(Style::default().fg(Color::DarkGray));
                f.render_widget(paragraph, inner);
            } else {
                let mut lines = Vec::new();
                for comment in &task.comments {
                    let header = format!(
                        "{} <{}> [{}]",
                        comment.actor.name, comment.actor.email, comment.ts
                    );
                    lines.push(Line::from(Span::styled(
                        header,
                        Style::default().fg(Color::Yellow),
                    )));
                    for body_line in comment.body.lines() {
                        lines.push(Line::from(body_line.to_owned()));
                    }
                    lines.push(Line::from(""));
                }
                let paragraph = Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((viewer.scroll_offset, 0));
                f.render_widget(paragraph, inner);
            }
        }
    }

    /// Helper to find node in current tree state (read-only).
    fn find_node_in_state(&self, task_id: TaskId) -> Option<&TreeNode> {
        for root in &self.tree_state.roots {
            if let Some(node) = Self::find_node_in_tree(root, task_id) {
                return Some(node);
            }
        }
        None
    }

    fn find_node_in_tree(node: &TreeNode, task_id: TaskId) -> Option<&TreeNode> {
        if node.task_id == task_id {
            return Some(node);
        }
        for child in &node.children {
            if let Some(found) = Self::find_node_in_tree(child, task_id) {
                return Some(found);
            }
        }
        None
    }

    /// Calculate scroll offset to keep selected item visible.
    #[allow(clippy::unused_self, clippy::missing_const_for_fn)]
    fn tree_scroll_offset(&self) -> usize {
        // TODO: Implement scroll logic based on area height
        0
    }

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<UiAction>> {
        if key.kind != KeyEventKind::Press {
            return Ok(None);
        }

        self.handle_browse_key(key)
    }

    #[allow(clippy::too_many_lines)]
    fn handle_browse_key(&mut self, key: KeyEvent) -> Result<Option<UiAction>> {
        match self.detail_focus {
            DetailFocus::None => match key.code {
                KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                    self.should_quit = true;
                    Ok(None)
                }
                KeyCode::Down | KeyCode::Char('j' | 'J') => {
                    self.app.select_next();
                    Ok(None)
                }
                KeyCode::Up | KeyCode::Char('k' | 'K') => {
                    self.app.select_prev();
                    Ok(None)
                }
                KeyCode::Enter => {
                    self.open_tree_view();
                    Ok(None)
                }
                KeyCode::Char('p' | 'P') => {
                    self.jump_to_parent();
                    Ok(None)
                }
                KeyCode::Char('r' | 'R') => {
                    self.app.refresh_tasks()?;
                    self.info("タスクを再読込しました");
                    Ok(None)
                }
                KeyCode::Char('c' | 'C') => self.app.selected_task_id().map_or_else(
                    || {
                        self.error("コメント対象のタスクが選択されていません");
                        Ok(None)
                    },
                    |task| Ok(Some(UiAction::AddComment { task })),
                ),
                KeyCode::Char('e' | 'E') => self.app.selected_task_id().map_or_else(
                    || {
                        self.error("編集対象のタスクが選択されていません");
                        Ok(None)
                    },
                    |task| Ok(Some(UiAction::EditTask { task })),
                ),
                KeyCode::Char('n' | 'N') => Ok(Some(UiAction::CreateTask)),
                KeyCode::Char('s' | 'S') => self.app.selected_task_id().map_or_else(
                    || {
                        self.error("子タスクを作成する親タスクが選択されていません");
                        Ok(None)
                    },
                    |parent| Ok(Some(UiAction::CreateSubtask { parent })),
                ),
                KeyCode::Char('y' | 'Y') => {
                    self.copy_selected_task_id();
                    Ok(None)
                }
                KeyCode::Char('t' | 'T') => {
                    self.open_state_picker();
                    Ok(None)
                }
                KeyCode::Char('v' | 'V') => {
                    self.open_comment_viewer();
                    Ok(None)
                }
                KeyCode::Char('f' | 'F') => Ok(Some(UiAction::EditFilter)),
                _ => Ok(None),
            },
            DetailFocus::TreeView => match key.code {
                KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                    self.detail_focus = DetailFocus::None;
                    Ok(None)
                }
                KeyCode::Down | KeyCode::Char('j' | 'J') => {
                    self.tree_view_down();
                    Ok(None)
                }
                KeyCode::Up | KeyCode::Char('k' | 'K') => {
                    self.tree_view_up();
                    Ok(None)
                }
                KeyCode::Char('h' | 'H') => {
                    self.tree_view_collapse();
                    Ok(None)
                }
                KeyCode::Char('l' | 'L') => {
                    self.tree_view_expand();
                    Ok(None)
                }
                KeyCode::Enter => {
                    self.tree_view_jump();
                    Ok(None)
                }
                _ => Ok(None),
            },
            DetailFocus::StatePicker => match key.code {
                KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                    self.close_state_picker();
                    Ok(None)
                }
                KeyCode::Down | KeyCode::Char('j' | 'J') => {
                    self.state_picker_down();
                    Ok(None)
                }
                KeyCode::Up | KeyCode::Char('k' | 'K') => {
                    self.state_picker_up();
                    Ok(None)
                }
                KeyCode::Enter => {
                    self.apply_state_picker_selection();
                    Ok(None)
                }
                _ => Ok(None),
            },
            DetailFocus::CommentViewer => match key.code {
                KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                    self.close_comment_viewer();
                    Ok(None)
                }
                KeyCode::Char('j' | 'J') => {
                    self.comment_viewer_scroll_down(1);
                    Ok(None)
                }
                KeyCode::Char('k' | 'K') => {
                    self.comment_viewer_scroll_up(1);
                    Ok(None)
                }
                KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Half-page down (approximate with 10 lines)
                    self.comment_viewer_scroll_down(10);
                    Ok(None)
                }
                KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    // Half-page up (approximate with 10 lines)
                    self.comment_viewer_scroll_up(10);
                    Ok(None)
                }
                _ => Ok(None),
            },
        }
    }

    fn jump_to_parent(&mut self) {
        if let Some(task) = self.app.selected_task() {
            let parent_id = self
                .app
                .get_parents(task.snapshot.id)
                .first()
                .map(|parent| parent.snapshot.id);
            if let Some(parent_id) = parent_id {
                self.app.jump_to_task(parent_id);
                let parent_title = self
                    .app
                    .tasks
                    .iter()
                    .find(|view| view.snapshot.id == parent_id)
                    .map_or("不明", |view| view.snapshot.title.as_str());
                self.info(format!("親タスクへジャンプ: {parent_title}"));
            } else {
                self.error("親タスクがありません");
            }
        }
    }

    fn copy_selected_task_id(&mut self) {
        let Some(task) = self.app.selected_task() else {
            self.error("コピー対象のタスクが選択されていません");
            return;
        };

        let task_id = task.snapshot.id.to_string();
        if let Err(err) = self.clipboard.set_text(&task_id) {
            self.error(format!("タスクIDのコピーに失敗しました: {err}"));
        } else {
            self.info(format!("タスクIDをコピーしました: {task_id}"));
        }
    }

    fn open_state_picker(&mut self) {
        let Some(task) = self.app.selected_task() else {
            self.error("ステータスを変更するタスクが選択されていません");
            return;
        };

        let options = self.state_picker_options(task.snapshot.state.as_deref());
        if options.is_empty() {
            self.error("ステータス候補が見つかりません");
            return;
        }
        let selected = options
            .iter()
            .position(|option| option.matches(task.snapshot.state.as_deref()))
            .unwrap_or(0);
        self.state_picker = Some(StatePickerState {
            task_id: task.snapshot.id,
            options,
            selected,
        });
        self.detail_focus = DetailFocus::StatePicker;
    }

    fn state_picker_options(&self, current_state: Option<&str>) -> Vec<StatePickerOption> {
        let mut options = vec![StatePickerOption::new(None)];
        let workflow = self.app.workflow();
        if workflow.is_restricted() {
            options.extend(
                workflow
                    .states()
                    .iter()
                    .map(|state| StatePickerOption::new(Some(state.value().to_owned()))),
            );
        } else {
            let values: BTreeSet<String> = self
                .app
                .tasks
                .iter()
                .filter_map(|view| view.snapshot.state.clone())
                .collect();
            for value in values {
                options.push(StatePickerOption::new(Some(value)));
            }
        }

        if let Some(current) = current_state
            && !options.iter().any(|option| option.matches(Some(current)))
        {
            options.push(StatePickerOption::new(Some(current.to_owned())));
        }

        options
    }

    fn state_picker_down(&mut self) {
        if let Some(picker) = &mut self.state_picker {
            if picker.options.is_empty() {
                return;
            }
            let max_index = picker.options.len() - 1;
            picker.selected = (picker.selected + 1).min(max_index);
        }
    }

    #[allow(clippy::missing_const_for_fn)]
    fn state_picker_up(&mut self) {
        if let Some(picker) = &mut self.state_picker {
            if picker.options.is_empty() {
                return;
            }
            picker.selected = picker.selected.saturating_sub(1);
        }
    }

    fn close_state_picker(&mut self) {
        self.state_picker = None;
        self.detail_focus = DetailFocus::None;
    }

    fn open_comment_viewer(&mut self) {
        let Some(task) = self.app.selected_task() else {
            self.error("コメントを表示するタスクが選択されていません");
            return;
        };

        self.comment_viewer = Some(CommentViewerState {
            task_id: task.snapshot.id,
            scroll_offset: 0,
        });
        self.detail_focus = DetailFocus::CommentViewer;
    }

    fn close_comment_viewer(&mut self) {
        self.comment_viewer = None;
        self.detail_focus = DetailFocus::None;
    }

    fn comment_viewer_scroll_down(&mut self, lines: u16) {
        if let Some(viewer) = &mut self.comment_viewer {
            viewer.scroll_offset = viewer.scroll_offset.saturating_add(lines);
        }
    }

    fn comment_viewer_scroll_up(&mut self, lines: u16) {
        if let Some(viewer) = &mut self.comment_viewer {
            viewer.scroll_offset = viewer.scroll_offset.saturating_sub(lines);
        }
    }

    fn apply_state_picker_selection(&mut self) {
        let Some(picker) = self.state_picker.take() else {
            return;
        };
        self.detail_focus = DetailFocus::None;
        let Some(option) = picker.options.get(picker.selected) else {
            self.error("ステータス候補が見つかりません");
            return;
        };
        let desired_state = option.value.clone();
        match self
            .app
            .set_task_state(picker.task_id, desired_state, &self.actor)
        {
            Ok(true) => self.info("ステータスを更新しました"),
            Ok(false) => self.info("ステータスは変更されませんでした"),
            Err(err) => self.error(format!("ステータス更新に失敗しました: {err}")),
        }
    }

    /// Build tree starting from a root task.
    fn build_tree_from_root(&self, root_id: TaskId) -> Option<TreeNode> {
        let root_view = self.app.tasks.iter().find(|t| t.snapshot.id == root_id)?;

        if !self.app.is_visible(root_id) {
            return None;
        }

        Some(TreeNode {
            task_id: root_view.snapshot.id,
            children: self.build_children_nodes(root_id),
            expanded: true, // デフォルトで展開
        })
    }

    /// Recursively build child nodes.
    fn build_children_nodes(&self, parent_id: TaskId) -> Vec<TreeNode> {
        let children = self.app.get_children(parent_id);
        children
            .iter()
            .filter(|child| self.app.is_visible(child.snapshot.id))
            .map(|child| TreeNode {
                task_id: child.snapshot.id,
                children: self.build_children_nodes(child.snapshot.id),
                expanded: false, // デフォルトで折りたたみ
            })
            .collect()
    }

    fn open_tree_view(&mut self) {
        let Some(current_task) = self.app.selected_task() else {
            self.error("タスクが選択されていません");
            return;
        };

        let current_id = current_task.snapshot.id;

        // Find root task
        let Some(root_task) = self.app.get_root(current_id) else {
            self.error("ルートタスクが見つかりません");
            return;
        };

        let root_id = root_task.snapshot.id;

        // Build tree
        let Some(tree) = self.build_tree_from_root(root_id) else {
            self.error("ツリーの構築に失敗しました");
            return;
        };

        // Initialize tree state
        self.tree_state.roots = vec![tree];
        self.tree_state.expand_to_task(current_id);
        self.tree_state.rebuild_visible_nodes();

        // Find and select current task in tree
        if let Some(index) = self
            .tree_state
            .visible_nodes
            .iter()
            .position(|(_, id)| *id == current_id)
        {
            self.tree_state.selected = index;
        }

        self.detail_focus = DetailFocus::TreeView;
    }

    #[allow(clippy::missing_const_for_fn)]
    fn tree_view_down(&mut self) {
        if self.tree_state.selected + 1 < self.tree_state.visible_nodes.len() {
            self.tree_state.selected += 1;
        }
    }

    #[allow(clippy::missing_const_for_fn)]
    fn tree_view_up(&mut self) {
        if self.tree_state.selected > 0 {
            self.tree_state.selected -= 1;
        }
    }

    fn tree_view_collapse(&mut self) {
        let Some(task_id) = self.tree_state.selected_task_id() else {
            return;
        };

        if let Some(node) = self.tree_state.find_node_mut(task_id) {
            if node.expanded && !node.children.is_empty() {
                // Collapse current node
                node.expanded = false;
                self.tree_state.rebuild_visible_nodes();
            } else {
                // Not expanded or no children, move to parent
                self.move_to_parent_in_tree(task_id);
            }
        }
    }

    fn move_to_parent_in_tree(&mut self, task_id: TaskId) {
        let parents = self.app.get_parents(task_id);
        if let Some(parent) = parents.first()
            && let Some(index) = self
                .tree_state
                .visible_nodes
                .iter()
                .position(|(_, id)| *id == parent.snapshot.id)
        {
            self.tree_state.selected = index;
        }
    }

    fn tree_view_expand(&mut self) {
        let Some(task_id) = self.tree_state.selected_task_id() else {
            return;
        };

        let children = self.app.get_children(task_id);
        if children.is_empty() {
            // No children, try to move to first child
            return;
        }

        // Expand node
        if let Some(node) = self.tree_state.find_node_mut(task_id) {
            if node.expanded {
                // Already expanded, move to first child
                if self.tree_state.selected + 1 < self.tree_state.visible_nodes.len() {
                    self.tree_state.selected += 1;
                }
            } else {
                node.expanded = true;
                self.tree_state.rebuild_visible_nodes();
            }
        }
    }

    fn tree_view_jump(&mut self) {
        let Some(task_id) = self.tree_state.selected_task_id() else {
            return;
        };

        // Jump to selected task
        self.app.jump_to_task(task_id);

        // Close tree view
        self.detail_focus = DetailFocus::None;

        // Get task title for message
        if let Some(task) = self.app.selected_task() {
            self.info(format!("タスクへジャンプ: {}", task.snapshot.title));
        }
    }

    fn apply_comment_input(&mut self, task: TaskId, raw: &str) -> Result<()> {
        match parse_comment_editor_output(raw) {
            Some(body) => {
                self.app
                    .add_comment(task, body, &self.actor)
                    .context("コメントの保存に失敗しました")?;
                self.info("コメントを追加しました");
            }
            None => self.info("コメントをキャンセルしました"),
        }
        Ok(())
    }

    fn apply_new_task_input(&mut self, raw: &str) -> Result<()> {
        match parse_new_task_editor_output(raw) {
            Ok(Some(data)) => {
                let id = self
                    .app
                    .create_task(data, &self.actor)
                    .context("タスクの作成に失敗しました")?;
                self.info(format!("タスクを作成しました: {id}"));
            }
            Ok(None) => self.info("タスク作成をキャンセルしました"),
            Err(msg) => self.error(msg),
        }
        Ok(())
    }

    fn apply_new_subtask_input(&mut self, parent: TaskId, raw: &str) -> Result<()> {
        match parse_new_task_editor_output(raw) {
            Ok(Some(mut data)) => {
                data.parent = Some(parent);
                let id = self
                    .app
                    .create_task(data, &self.actor)
                    .context("タスクの作成に失敗しました")?;
                self.info(format!("子タスクを作成しました: {id}"));
            }
            Ok(None) => self.info("タスク作成をキャンセルしました"),
            Err(msg) => self.error(msg),
        }
        Ok(())
    }

    fn apply_edit_task_input(&mut self, task: TaskId, raw: &str) -> Result<()> {
        match parse_new_task_editor_output(raw) {
            Ok(Some(data)) => {
                let updated = self
                    .app
                    .update_task(task, data, &self.actor)
                    .context("タスクの更新に失敗しました")?;
                if updated {
                    self.info("タスクを更新しました");
                } else {
                    self.info("変更はありませんでした");
                }
            }
            Ok(None) => self.info("タスク編集をキャンセルしました"),
            Err(msg) => self.error(msg),
        }
        Ok(())
    }

    fn apply_filter_editor_output(&mut self, raw: &str) {
        match parse_filter_editor_output(raw) {
            Ok(filter) => {
                if &filter == self.app.filter() {
                    self.info("フィルタに変更はありません");
                } else {
                    self.app.set_filter(filter.clone());
                    let summary = summarize_task_filter(&filter);
                    if self.app.has_visible_tasks() {
                        self.info(format!("フィルタを更新しました: {summary}"));
                    } else {
                        self.info(format!("フィルタを更新しました（該当なし）: {summary}"));
                    }
                }
            }
            Err(err) => self.error(format!("フィルタの解析に失敗しました: {err}")),
        }
    }

    fn info(&mut self, message: impl Into<String>) {
        self.message = Some(Message::info(message));
    }

    fn error(&mut self, message: impl Into<String>) {
        self.message = Some(Message::error(message));
    }

    fn instructions(&self) -> String {
        match self.detail_focus {
            DetailFocus::None => {
                let base = "j/k:移動 ↵:ツリー n:新規 s:子タスク e:編集 c:コメント v:コメント表示 r:再読込 p:親へ y:IDコピー t:状態 f:フィルタ q:終了";
                format!("{} [{} <{}>]", base, self.actor.name, self.actor.email)
            }
            DetailFocus::TreeView => "j/k:移動 h:閉じる l:開く ↵:ジャンプ q/Esc:閉じる".to_string(),
            DetailFocus::StatePicker => "j/k:移動 ↵:決定 q/Esc:キャンセル".to_string(),
            DetailFocus::CommentViewer => "j/k:スクロール Ctrl-d/Ctrl-u:半画面スクロール q/Esc:閉じる".to_string(),
        }
    }

    fn filter_summary_text(&self) -> String {
        summarize_task_filter(self.app.filter())
    }

    fn status_text(&self) -> Cow<'_, str> {
        self.message.as_ref().map_or(
            Cow::Borrowed("ステータスメッセージはありません"),
            |msg| Cow::Borrowed(msg.text.as_str()),
        )
    }

    fn status_style(&self) -> Style {
        self.message.as_ref().map_or_else(Style::default, Message::style)
    }

    fn tick(&mut self) {
        if let Some(msg) = &self.message
            && msg.is_expired(Duration::from_secs(5))
        {
            self.message = None;
        }
    }
}

#[derive(Clone, Copy)]
enum UiAction {
    AddComment { task: TaskId },
    EditTask { task: TaskId },
    CreateTask,
    CreateSubtask { parent: TaskId },
    EditFilter,
}

fn handle_ui_action<S: TaskStore>(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ui: &mut Ui<S>,
    action: UiAction,
) -> Result<()> {
    match action {
        UiAction::AddComment { task } => {
            let template = comment_editor_template(&ui.actor, task);
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_comment_input(task, &raw)?;
        }
        UiAction::EditTask { task } => {
            let state_hint = ui.app.workflow().state_hint();
            let Some(template) = ui
                .app
                .tasks
                .iter()
                .find(|view| view.snapshot.id == task)
                .map(|view| edit_task_editor_template(view, state_hint.as_deref()))
            else {
                ui.error("編集対象のタスクが見つかりません");
                return Ok(());
            };
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_edit_task_input(task, &raw)?;
        }
        UiAction::CreateTask => {
            let hint = ui.app.workflow().state_hint();
            let default_state = ui.app.workflow().default_state();
            let template = new_task_editor_template(None, hint.as_deref(), default_state);
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_new_task_input(&raw)?;
        }
        UiAction::CreateSubtask { parent } => {
            let parent_view = ui.app.tasks.iter().find(|view| view.snapshot.id == parent);
            let hint = ui.app.workflow().state_hint();
            let default_state = ui.app.workflow().default_state();
            let template = new_task_editor_template(parent_view, hint.as_deref(), default_state);
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_new_subtask_input(parent, &raw)?;
        }
        UiAction::EditFilter => {
            let template = filter_editor_template(ui.app.filter());
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_filter_editor_output(&raw);
        }
    }
    Ok(())
}

struct Message {
    text: String,
    level: MessageLevel,
    created_at: Instant,
}

enum MessageLevel {
    Info,
    Error,
}

impl Message {
    fn info(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            level: MessageLevel::Info,
            created_at: Instant::now(),
        }
    }

    fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            level: MessageLevel::Error,
            created_at: Instant::now(),
        }
    }

    fn style(&self) -> Style {
        match self.level {
            MessageLevel::Info => Style::default().fg(Color::Green),
            MessageLevel::Error => Style::default().fg(Color::Red),
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.created_at.elapsed() >= ttl
    }
}

fn comment_editor_template(actor: &Actor, task: TaskId) -> String {
    format!(
        "# コメントを入力してください。\n# 空のまま保存するとキャンセルされます。\n# Task: {task}\n# Actor: {} <{}>\n\n",
        actor.name, actor.email
    )
}

fn parse_comment_editor_output(raw: &str) -> Option<String> {
    let body = raw
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = body.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn edit_task_editor_template(task: &TaskView, state_hint: Option<&str>) -> String {
    let snapshot = &task.snapshot;
    let state = snapshot.state.as_deref().unwrap_or_default();
    let labels = if snapshot.labels.is_empty() {
        String::new()
    } else {
        snapshot
            .labels
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let assignees = if snapshot.assignees.is_empty() {
        String::new()
    } else {
        snapshot
            .assignees
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut lines = vec![
        "# 選択中のタスクを編集します。タイトルは必須です。".to_string(),
        "# 空のフィールドは対応する値をクリアします。".to_string(),
        format!("title: {}", snapshot.title),
    ];
    if let Some(hint) = state_hint {
        lines.push(format!("# state 候補: {hint}"));
    }
    lines.extend([
        format!("state: {state}"),
        format!("labels: {labels}"),
        format!("assignees: {assignees}"),
        "---".to_string(),
        "# この下で説明を編集してください。空欄で説明を削除します。".to_string(),
    ]);
    if snapshot.description.is_empty() {
        lines.push(String::new());
    } else {
        lines.extend(snapshot.description.lines().map(str::to_owned));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn new_task_editor_template(
    parent: Option<&TaskView>,
    state_hint: Option<&str>,
    default_state: Option<&str>,
) -> String {
    let header = parent.map_or_else(
        || "# 新規タスクを作成します。".to_owned(),
        |p| {
            format!(
                "# 新規タスク（親: {} [{}...]）を作成します。",
                p.snapshot.title,
                &p.snapshot.id.to_string()[..12]
            )
        },
    );

    let mut lines = vec![
        header,
        "# タイトルは必須です。".to_string(),
        "# 空のまま保存すると作成をキャンセルしたものとして扱います。".to_string(),
        "title: ".to_string(),
    ];
    if let Some(hint) = state_hint {
        lines.push(format!("# state 候補: {hint}"));
    }
    let state_line = default_state.map_or_else(|| "state: ".to_string(), |value| format!("state: {value}"));
    lines.extend([
        state_line,
        "labels: ".to_string(),
        "assignees: ".to_string(),
        "---".to_string(),
        "# この下に説明をMarkdown形式で記入してください。不要なら空のままにしてください。".to_string(),
        String::new(),
    ]);
    lines.join("\n")
}

fn filter_editor_template(filter: &TaskFilter) -> String {
    let states = filter.states.iter().cloned().collect::<Vec<_>>().join(", ");
    let labels = filter.labels.iter().cloned().collect::<Vec<_>>().join(", ");
    let assignees = filter.assignees.iter().cloned().collect::<Vec<_>>().join(", ");
    let parents = filter
        .parents
        .iter()
        .map(TaskId::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let children = filter
        .children
        .iter()
        .map(TaskId::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let text = filter.text.clone().unwrap_or_default();
    let updated_since = filter
        .updated
        .as_ref()
        .and_then(|updated| updated.since)
        .map(format_timestamp)
        .unwrap_or_default();
    let updated_until = filter
        .updated
        .as_ref()
        .and_then(|updated| updated.until)
        .map(format_timestamp)
        .unwrap_or_default();

    let lines = vec![
        "# フィルタを編集します。空欄のフィールドは該当条件なしとして扱われます。".to_string(),
        "# states/labels/assignees/parents/children はカンマ区切りで入力してください。".to_string(),
        "# updated_since / updated_until は RFC3339 (例: 2025-01-01T09:00:00+09:00) 形式。".to_string(),
        "# state_kinds には done/in_progress などの kind を指定し、!done で除外できます。".to_string(),
        format!("# state_kinds の候補: {}", state_kind_options_hint()),
        format!("states: {states}"),
        format!(
            "state_kinds: {}",
            state_kind_filter_to_editor_value(&filter.state_kinds)
        ),
        format!("labels: {labels}"),
        format!("assignees: {assignees}"),
        format!("parents: {parents}"),
        format!("children: {children}"),
        format!("text: {text}"),
        format!("updated_since: {updated_since}"),
        format!("updated_until: {updated_until}"),
        String::new(),
    ];
    lines.join("\n")
}

fn state_kind_filter_to_editor_value(filter: &StateKindFilter) -> String {
    if filter.is_empty() {
        return String::new();
    }
    let mut tokens = Vec::new();
    for kind in &filter.include {
        tokens.push(kind.as_str().to_string());
    }
    for kind in &filter.exclude {
        tokens.push(format!("!{}", kind.as_str()));
    }
    tokens.join(", ")
}

fn state_kind_summary_tokens(filter: &StateKindFilter) -> Vec<String> {
    if filter.is_empty() {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    if !filter.include.is_empty() {
        let includes = filter
            .include
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        tokens.push(format!("state-kind={includes}"));
    }
    if !filter.exclude.is_empty() {
        let excludes = filter
            .exclude
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        tokens.push(format!("state-kind!={excludes}"));
    }
    tokens
}

fn parse_new_task_editor_output(raw: &str) -> Result<Option<NewTaskData>, String> {
    let mut title: Option<&str> = None;
    let mut state: Option<&str> = None;
    let mut labels: Option<&str> = None;
    let mut assignees: Option<&str> = None;
    let mut description_lines = Vec::new();
    let mut in_description = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if in_description {
            description_lines.push(line);
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "---" {
            in_description = true;
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            let value = value.trim();
            match key.trim() {
                "title" => title = Some(value),
                "state" => state = Some(value),
                "labels" => labels = Some(value),
                "assignees" => assignees = Some(value),
                unknown => {
                    return Err(format!("未知のフィールドです: {unknown}"));
                }
            }
        } else {
            return Err(format!("フィールドの形式が正しくありません: {trimmed}"));
        }
    }

    let title = title.unwrap_or("").trim();
    let state = state.unwrap_or("").trim();
    let labels = labels.unwrap_or("").trim();
    let assignees = assignees.unwrap_or("").trim();
    let description = description_lines.join("\n");

    let is_all_empty = title.is_empty()
        && state.is_empty()
        && labels.is_empty()
        && assignees.is_empty()
        && description.trim().is_empty();
    if is_all_empty {
        return Ok(None);
    }

    if title.is_empty() {
        return Err("タイトルを入力してください".into());
    }

    let state = if state.is_empty() {
        None
    } else {
        Some(state.to_owned())
    };
    let labels = parse_list(labels);
    let assignees = parse_list(assignees);
    let description = if description.trim().is_empty() {
        None
    } else {
        Some(description.trim_end().to_owned())
    };

    Ok(Some(NewTaskData {
        title: title.to_owned(),
        state,
        labels,
        assignees,
        description,
        parent: None,
    }))
}

fn parse_filter_editor_output(raw: &str) -> Result<TaskFilter, String> {
    let mut states = BTreeSet::new();
    let mut labels = BTreeSet::new();
    let mut assignees = BTreeSet::new();
    let mut parents = BTreeSet::new();
    let mut children = BTreeSet::new();
    let mut text: Option<String> = None;
    let mut updated_since: Option<OffsetDateTime> = None;
    let mut updated_until: Option<OffsetDateTime> = None;
    let mut state_kinds_raw: Option<&str> = None;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            return Err(format!("フィールドの形式が正しくありません: {trimmed}"));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "states" => {
                states = parse_list(value).into_iter().collect();
            }
            "labels" => {
                labels = parse_list(value).into_iter().collect();
            }
            "assignees" => {
                assignees = parse_list(value).into_iter().collect();
            }
            "parents" => {
                parents = parse_task_id_list(value)?;
            }
            "children" => {
                children = parse_task_id_list(value)?;
            }
            "text" => {
                if value.is_empty() {
                    text = None;
                } else {
                    text = Some(value.to_owned());
                }
            }
            "updated_since" => {
                updated_since = parse_optional_timestamp(value)?;
            }
            "updated_until" => {
                updated_until = parse_optional_timestamp(value)?;
            }
            "state_kinds" => state_kinds_raw = Some(value),
            unknown => return Err(format!("未知のフィールドです: {unknown}")),
        }
    }

    let updated = match (updated_since, updated_until) {
        (None, None) => None,
        _ => Some(UpdatedFilter {
            since: updated_since,
            until: updated_until,
        }),
    };

    let state_kinds = parse_state_kind_filter(state_kinds_raw.unwrap_or(""))?;

    Ok(TaskFilter {
        states,
        state_kinds,
        labels,
        assignees,
        parents,
        children,
        text,
        updated,
    })
}

fn parse_task_id_list(input: &str) -> Result<BTreeSet<TaskId>, String> {
    let mut ids = BTreeSet::new();
    for value in parse_list(input) {
        let id = TaskId::from_str(&value).map_err(|_| format!("TaskId の形式が正しくありません: {value}"))?;
        ids.insert(id);
    }
    Ok(ids)
}

fn parse_optional_timestamp(input: &str) -> Result<Option<OffsetDateTime>, String> {
    if input.is_empty() {
        return Ok(None);
    }
    OffsetDateTime::parse(input, &Rfc3339)
        .map(Some)
        .map_err(|err| format!("時刻の形式が正しくありません ({input}): {err}"))
}

fn parse_state_kind_filter(input: &str) -> Result<StateKindFilter, String> {
    if input.trim().is_empty() {
        return Ok(StateKindFilter::default());
    }
    let mut filter = StateKindFilter::default();
    for token in parse_list(input) {
        let (negated, name) = token
            .strip_prefix('!')
            .map_or((false, token.as_str()), |rest| (true, rest));
        let Some(kind) = parse_state_kind_name(name) else {
            return Err(format!("state_kind の指定が不正です: {name}"));
        };
        if negated {
            filter.exclude.insert(kind);
        } else {
            filter.include.insert(kind);
        }
    }
    Ok(filter)
}

fn parse_state_kind_name(name: &str) -> Option<StateKind> {
    let normalized = name.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    match normalized.as_str() {
        "done" => Some(StateKind::Done),
        "in_progress" => Some(StateKind::InProgress),
        "blocked" => Some(StateKind::Blocked),
        "todo" => Some(StateKind::Todo),
        "backlog" => Some(StateKind::Backlog),
        _ => None,
    }
}

const STATE_KIND_HINT: &str = "done, in_progress, blocked, todo, backlog";

const fn state_kind_options_hint() -> &'static str {
    STATE_KIND_HINT
}

const fn state_kind_marker(kind: Option<StateKind>) -> &'static str {
    match kind {
        Some(StateKind::Done) => " ✓",
        Some(StateKind::InProgress) => " →",
        Some(StateKind::Blocked) => " ⊗",
        Some(StateKind::Todo) => " □",
        Some(StateKind::Backlog) => " ◇",
        None => "",
    }
}

fn with_terminal_suspended<F, T>(terminal: &mut Terminal<CrosstermBackend<Stdout>>, f: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    suspend_terminal(terminal)?;
    let result = f();
    resume_terminal(terminal)?;
    result
}

fn suspend_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    terminal.show_cursor()?;
    terminal.flush()?;
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("failed to leave alternate screen")?;
    Ok(())
}

fn resume_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    execute!(terminal.backend_mut(), EnterAlternateScreen).context("failed to re-enter alternate screen")?;
    enable_raw_mode().context("failed to enable raw mode")?;
    terminal.clear()?;
    terminal.hide_cursor()?;
    terminal.flush()?;
    Ok(())
}

fn resolve_editor_command() -> String {
    env::var("GIT_MILE_EDITOR")
        .or_else(|_| env::var("VISUAL"))
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into())
}

fn launch_editor(initial: &str) -> Result<String> {
    let mut tempfile = NamedTempFile::new().context("一時ファイルの作成に失敗しました")?;
    tempfile
        .write_all(initial.as_bytes())
        .context("一時ファイルへの書き込みに失敗しました")?;
    tempfile
        .flush()
        .context("一時ファイルのフラッシュに失敗しました")?;

    let temp_path: PathBuf = tempfile.path().to_path_buf();

    let editor = resolve_editor_command();
    let mut parts =
        shell_words::split(&editor).map_err(|err| anyhow!("エディタコマンドを解析できません: {err}"))?;
    if parts.is_empty() {
        parts.push(editor);
    }
    let program = parts.remove(0);

    let status = Command::new(&program)
        .args(&parts)
        .arg(&temp_path)
        .status()
        .with_context(|| format!("エディタ {program} の起動に失敗しました"))?;
    if !status.success() {
        return Err(anyhow!("エディタが異常終了しました (終了コード: {status})"));
    }

    let contents =
        fs::read_to_string(&temp_path).context("エディタで編集した内容の読み込みに失敗しました")?;
    Ok(contents)
}

fn parse_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

fn summarize_task_filter(filter: &TaskFilter) -> String {
    if filter.is_empty() {
        return "未設定".to_string();
    }
    let mut parts = Vec::new();
    if !filter.states.is_empty() {
        parts.push(format!("state={}", join_string_set(&filter.states)));
    }
    if !filter.labels.is_empty() {
        parts.push(format!("label={}", join_string_set(&filter.labels)));
    }
    if !filter.assignees.is_empty() {
        parts.push(format!("assignee={}", join_string_set(&filter.assignees)));
    }
    if !filter.parents.is_empty() {
        parts.push(format!("parent={}", join_task_ids(&filter.parents)));
    }
    if !filter.children.is_empty() {
        parts.push(format!("child={}", join_task_ids(&filter.children)));
    }
    if let Some(text) = filter.text.as_deref().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    }) {
        parts.push(format!("text=\"{text}\""));
    }
    if let Some(updated) = &filter.updated {
        if let Some(since) = updated.since {
            parts.push(format!("since={}", format_timestamp(since)));
        }
        if let Some(until) = updated.until {
            parts.push(format!("until={}", format_timestamp(until)));
        }
    }
    parts.extend(state_kind_summary_tokens(&filter.state_kinds));
    if parts.is_empty() {
        "未設定".into()
    } else {
        parts.join(" / ")
    }
}

fn join_string_set(values: &BTreeSet<String>) -> String {
    values.iter().map(String::as_str).collect::<Vec<_>>().join(", ")
}

fn join_task_ids(values: &BTreeSet<TaskId>) -> String {
    values.iter().map(short_task_id).collect::<Vec<_>>().join(", ")
}

fn short_task_id(task_id: &TaskId) -> String {
    let id = task_id.to_string();
    id.chars().take(8).collect()
}

fn format_timestamp(ts: OffsetDateTime) -> String {
    ts.format(&Rfc3339).unwrap_or_else(|_| ts.to_string())
}

fn resolve_actor() -> Actor {
    let name = std::env::var("GIT_MILE_ACTOR_NAME")
        .or_else(|_| std::env::var("GIT_AUTHOR_NAME"))
        .or_else(|_| {
            git2::Config::open_default()
                .and_then(|config| config.get_string("user.name"))
                .map_err(|_| std::env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| "git-mile".to_owned());
    let email = std::env::var("GIT_MILE_ACTOR_EMAIL")
        .or_else(|_| std::env::var("GIT_AUTHOR_EMAIL"))
        .or_else(|_| {
            git2::Config::open_default()
                .and_then(|config| config.get_string("user.email"))
                .map_err(|_| std::env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| "git-mile@example.invalid".to_owned());
    Actor { name, email }
}

impl TaskStore for GitStore {
    fn list_tasks(&self) -> Result<Vec<TaskId>> {
        Self::list_tasks(self)
    }

    fn load_events(&self, task: TaskId) -> Result<Vec<Event>> {
        Self::load_events(self, task)
    }

    fn append_event(&self, event: &Event) -> Result<()> {
        Self::append_event(self, event).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;
    use crate::config::StateKind;
    use anyhow::{Result, anyhow};
    use git_mile_core::event::EventKind;
    use ratatui::layout::{Constraint, Direction, Layout, Rect};
    use std::cell::RefCell;
    use std::collections::{BTreeSet, HashMap};
    use std::rc::Rc;
    use std::str::FromStr;
    use time::{OffsetDateTime, format_description::well_known::Rfc3339};

    struct MockStore {
        tasks: RefCell<Vec<TaskId>>,
        events: RefCell<HashMap<TaskId, Vec<Event>>>,
    }

    impl MockStore {
        fn new() -> Self {
            Self {
                tasks: RefCell::new(Vec::new()),
                events: RefCell::new(HashMap::new()),
            }
        }

        fn with_task(self, id: TaskId, events: Vec<Event>) -> Self {
            self.tasks.borrow_mut().push(id);
            self.events.borrow_mut().insert(id, events);
            self
        }

        fn from_tasks(entries: Vec<(TaskId, Vec<Event>)>) -> Self {
            let store = Self::new();
            {
                let mut tasks = store.tasks.borrow_mut();
                let mut map = store.events.borrow_mut();
                for (id, events) in entries {
                    tasks.push(id);
                    map.insert(id, events);
                }
            }
            store
        }
    }

    #[test]
    fn truncate_with_ellipsis_returns_borrowed_when_short() {
        let title = "Short title";
        assert!(matches!(
            truncate_with_ellipsis(title, 20),
            Cow::Borrowed(result) if result == title
        ));
    }

    #[test]
    fn truncate_with_ellipsis_handles_multibyte_titles() {
        let title = "あいうえおかきくけこ";
        assert_eq!(truncate_with_ellipsis(title, 5), "あい...");
    }

    #[test]
    fn truncate_with_ellipsis_keeps_grapheme_clusters_intact() {
        let title = "a\u{0301}bcdef";
        assert_eq!(truncate_with_ellipsis(title, 4), "a\u{0301}...");
    }

    impl TaskStore for MockStore {
        fn list_tasks(&self) -> Result<Vec<TaskId>> {
            Ok(self.tasks.borrow().clone())
        }

        fn load_events(&self, task: TaskId) -> Result<Vec<Event>> {
            Ok(self.events.borrow().get(&task).cloned().unwrap_or_default())
        }

        fn append_event(&self, event: &Event) -> Result<()> {
            let mut events = self.events.borrow_mut();
            let entry = events.entry(event.task).or_default();
            entry.push(event.clone());
            let mut tasks = self.tasks.borrow_mut();
            if !tasks.contains(&event.task) {
                tasks.push(event.task);
            }
            Ok(())
        }
    }

    fn actor() -> Actor {
        Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        }
    }

    fn event(task: TaskId, ts: OffsetDateTime, kind: EventKind) -> Event {
        let mut ev = Event::new(task, &actor(), kind);
        ev.ts = ts;
        ev
    }

    fn ts(secs: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(secs).expect("must create timestamp from unix seconds")
    }

    fn fixed_task_id(n: u8) -> TaskId {
        TaskId::from_str(&format!("00000000-0000-0000-0000-0000000000{n:02}")).expect("must parse task id")
    }

    fn created(task: TaskId, secs: i64, title: &str) -> Event {
        event(
            task,
            ts(secs),
            EventKind::TaskCreated {
                title: title.into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: None,
                state_kind: None,
            },
        )
    }

    fn child_link(secs: i64, parent: TaskId, child: TaskId) -> Event {
        event(child, ts(secs), EventKind::ChildLinked { parent, child })
    }

    #[test]
    fn status_footer_height_matches_constraints() {
        let constraints = Ui::<MockStore>::status_layout_constraints();
        let total: u16 = constraints.iter().map(min_height_for_constraint).sum();
        assert_eq!(total, Ui::<MockStore>::STATUS_FOOTER_MIN_HEIGHT);
    }

    const fn min_height_for_constraint(constraint: &Constraint) -> u16 {
        match *constraint {
            Constraint::Length(value) | Constraint::Min(value) => value,
            _ => 0,
        }
    }

    #[derive(Default)]
    struct NoopClipboard;

    impl ClipboardSink for NoopClipboard {
        fn set_text(&mut self, _text: &str) -> Result<()> {
            Ok(())
        }
    }

    struct RecordingClipboard {
        writes: Rc<RefCell<Vec<String>>>,
    }

    impl RecordingClipboard {
        fn new(writes: Rc<RefCell<Vec<String>>>) -> Self {
            Self { writes }
        }
    }

    impl ClipboardSink for RecordingClipboard {
        fn set_text(&mut self, text: &str) -> Result<()> {
            self.writes.borrow_mut().push(text.to_string());
            Ok(())
        }
    }

    struct FailingClipboard {
        message: String,
    }

    impl FailingClipboard {
        fn new(message: impl Into<String>) -> Self {
            Self {
                message: message.into(),
            }
        }
    }

    impl ClipboardSink for FailingClipboard {
        fn set_text(&mut self, _text: &str) -> Result<()> {
            Err(anyhow!(self.message.clone()))
        }
    }

    fn ui_with_clipboard(app: App<MockStore>, clipboard: Box<dyn ClipboardSink>) -> Ui<MockStore> {
        Ui::with_clipboard(app, actor(), clipboard)
    }

    #[test]
    fn osc52_sequence_encodes_text() {
        let seq = super::osc52_sequence("Task-ID");
        assert_eq!(seq, "\x1b]52;c;VGFzay1JRA==\x07");
    }

    #[test]
    fn diff_sets_detects_added_and_removed_items() {
        let current = BTreeSet::from([String::from("a"), String::from("b")]);
        let desired = BTreeSet::from([String::from("b"), String::from("c")]);

        let diff = super::diff_sets(&current, &desired);

        assert_eq!(diff.added, vec![String::from("c")]);
        assert_eq!(diff.removed, vec![String::from("a")]);
    }

    #[test]
    fn task_view_sorts_comments_chronologically() {
        let task = TaskId::new();
        let events = vec![
            event(
                task,
                ts(2000),
                EventKind::CommentAdded {
                    comment_id: EventId::new(),
                    body_md: "Second".into(),
                },
            ),
            event(
                task,
                ts(0),
                EventKind::TaskCreated {
                    title: "Title".into(),
                    labels: Vec::new(),
                    assignees: Vec::new(),
                    description: None,
                    state: None,
                    state_kind: None,
                },
            ),
            event(
                task,
                ts(1000),
                EventKind::CommentAdded {
                    comment_id: EventId::new(),
                    body_md: "First".into(),
                },
            ),
        ];

        let view = TaskView::from_events(&events);
        assert_eq!(view.comments.len(), 2);
        assert_eq!(view.comments[0].body, "First");
        assert_eq!(view.comments[1].body, "Second");
        assert_eq!(view.last_updated, Some(ts(2000)));
    }

    #[test]
    fn app_refreshes_tasks_sorted_by_last_update() -> Result<()> {
        let task_a = TaskId::new();
        let task_b = TaskId::new();

        let events_a = vec![
            event(
                task_a,
                ts(0),
                EventKind::TaskCreated {
                    title: "A".into(),
                    labels: Vec::new(),
                    assignees: Vec::new(),
                    description: None,
                    state: None,
                    state_kind: None,
                },
            ),
            event(
                task_a,
                ts(2000),
                EventKind::CommentAdded {
                    comment_id: EventId::new(),
                    body_md: "Update".into(),
                },
            ),
        ];

        let events_b = vec![event(
            task_b,
            ts(1000),
            EventKind::TaskCreated {
                title: "B".into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: None,
                state_kind: None,
            },
        )];

        let store = MockStore::new()
            .with_task(task_a, events_a)
            .with_task(task_b, events_b);

        let app = App::new(store, WorkflowConfig::unrestricted())?;
        assert_eq!(app.tasks.len(), 2);
        let titles: Vec<_> = app
            .tasks
            .iter()
            .map(|view| view.snapshot.title.as_str())
            .collect();
        assert_eq!(titles, vec!["A", "B"]);
        Ok(())
    }

    #[test]
    fn app_get_children_uses_parent_links() -> Result<()> {
        let parent = TaskId::new();
        let child = TaskId::new();

        let parent_events = vec![created(parent, 0, "Parent")];
        let child_events = vec![created(child, 10, "Child"), child_link(20, parent, child)];

        let store = MockStore::new()
            .with_task(parent, parent_events)
            .with_task(child, child_events);
        let app = App::new(store, WorkflowConfig::unrestricted())?;

        let children = app.get_children(parent);
        assert_eq!(children.len(), 1);
        assert_eq!(children[0].snapshot.id, child);
        Ok(())
    }

    #[test]
    fn app_get_children_preserves_task_order() -> Result<()> {
        let parent = TaskId::new();
        let recent_child = TaskId::new();
        let older_child = TaskId::new();

        let store = MockStore::new()
            .with_task(parent, vec![created(parent, 0, "Parent")])
            .with_task(
                older_child,
                vec![
                    created(older_child, 10, "Older"),
                    child_link(11, parent, older_child),
                ],
            )
            .with_task(
                recent_child,
                vec![
                    created(recent_child, 20, "Recent"),
                    child_link(21, parent, recent_child),
                ],
            );
        let app = App::new(store, WorkflowConfig::unrestricted())?;

        let children = app.get_children(parent);
        let ids: Vec<TaskId> = children.iter().map(|view| view.snapshot.id).collect();
        assert_eq!(ids, vec![recent_child, older_child]);
        Ok(())
    }

    #[test]
    fn app_get_root_handles_cyclic_parent_graph() -> Result<()> {
        let root = fixed_task_id(1);
        let loop_a = fixed_task_id(2);
        let loop_b = fixed_task_id(3);
        let leaf = fixed_task_id(4);

        let store = MockStore::from_tasks(vec![
            (root, vec![created(root, 0, "Root")]),
            (
                loop_a,
                vec![
                    created(loop_a, 1, "LoopA"),
                    child_link(2, root, loop_a),
                    child_link(3, loop_b, loop_a),
                ],
            ),
            (
                loop_b,
                vec![created(loop_b, 4, "LoopB"), child_link(5, loop_a, loop_b)],
            ),
            (
                leaf,
                vec![
                    created(leaf, 6, "Leaf"),
                    child_link(7, loop_a, leaf),
                    child_link(8, loop_b, leaf),
                ],
            ),
        ]);
        let app = App::new(store, WorkflowConfig::unrestricted())?;

        let root_view = app.get_root(leaf).expect("must locate a root despite cycle");
        assert_eq!(root_view.snapshot.id, root);
        Ok(())
    }

    #[test]
    fn app_filter_only_shows_matching_tasks() -> Result<()> {
        let root = fixed_task_id(1);
        let child = fixed_task_id(2);
        let grandchild = fixed_task_id(3);

        let store = MockStore::new()
            .with_task(root, vec![created(root, 0, "Root")])
            .with_task(
                child,
                vec![created(child, 10, "Child"), child_link(11, root, child)],
            )
            .with_task(
                grandchild,
                vec![
                    created(grandchild, 20, "Grandchild"),
                    child_link(21, child, grandchild),
                ],
            );
        let mut app = App::new(store, WorkflowConfig::unrestricted())?;
        let filter = TaskFilter {
            text: Some("Grand".into()),
            ..TaskFilter::default()
        };
        app.set_filter(filter);

        let titles: Vec<String> = app
            .visible_tasks()
            .map(|view| view.snapshot.title.clone())
            .collect();
        assert_eq!(titles, vec!["Grandchild"]);
        Ok(())
    }

    #[test]
    fn app_filter_matching_parent_does_not_show_children() -> Result<()> {
        let root = fixed_task_id(1);
        let child = fixed_task_id(2);

        let store = MockStore::new()
            .with_task(root, vec![created(root, 0, "Root")])
            .with_task(
                child,
                vec![created(child, 10, "Child"), child_link(11, root, child)],
            );
        let mut app = App::new(store, WorkflowConfig::unrestricted())?;
        let filter = TaskFilter {
            text: Some("Root".into()),
            ..TaskFilter::default()
        };
        app.set_filter(filter);

        let titles: Vec<String> = app
            .visible_tasks()
            .map(|view| view.snapshot.title.clone())
            .collect();
        assert_eq!(titles, vec!["Root"]);
        Ok(())
    }

    #[test]
    fn tree_view_includes_parent_and_child() -> Result<()> {
        let parent = TaskId::new();
        let child = TaskId::new();

        let parent_events = vec![event(
            parent,
            ts(0),
            EventKind::TaskCreated {
                title: "Parent".into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: None,
                state_kind: None,
            },
        )];
        let child_events = vec![
            event(
                child,
                ts(10),
                EventKind::TaskCreated {
                    title: "Child".into(),
                    labels: Vec::new(),
                    assignees: Vec::new(),
                    description: None,
                    state: None,
                    state_kind: None,
                },
            ),
            event(child, ts(20), EventKind::ChildLinked { parent, child }),
        ];

        let store = MockStore::new()
            .with_task(parent, parent_events)
            .with_task(child, child_events);
        let app = App::new(store, WorkflowConfig::unrestricted())?;
        let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));
        ui.open_tree_view();

        assert_eq!(ui.detail_focus, DetailFocus::TreeView);
        let visible: Vec<TaskId> = ui
            .tree_state
            .visible_nodes
            .iter()
            .map(|(_, task_id)| *task_id)
            .collect();
        assert_eq!(visible, vec![parent, child]);
        Ok(())
    }

    #[test]
    fn tree_view_expands_path_to_selected_grandchild() -> Result<()> {
        let parent = TaskId::new();
        let child = TaskId::new();
        let grandchild = TaskId::new();

        let store = MockStore::new()
            .with_task(parent, vec![created(parent, 0, "Parent")])
            .with_task(
                child,
                vec![created(child, 10, "Child"), child_link(11, parent, child)],
            )
            .with_task(
                grandchild,
                vec![
                    created(grandchild, 20, "Grandchild"),
                    child_link(21, child, grandchild),
                ],
            );
        let mut app = App::new(store, WorkflowConfig::unrestricted())?;
        app.jump_to_task(grandchild);
        let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));

        ui.open_tree_view();

        let visible: Vec<TaskId> = ui
            .tree_state
            .visible_nodes
            .iter()
            .map(|(_, task_id)| *task_id)
            .collect();
        assert_eq!(visible, vec![parent, child, grandchild]);
        assert_eq!(ui.tree_state.selected_task_id(), Some(grandchild));
        Ok(())
    }

    #[test]
    fn copy_selected_task_id_writes_to_clipboard() -> Result<()> {
        let task = TaskId::new();
        let store = MockStore::new().with_task(task, vec![created(task, 0, "Task")]);
        let app = App::new(store, WorkflowConfig::unrestricted())?;
        let writes = Rc::new(RefCell::new(Vec::new()));
        let clipboard = RecordingClipboard::new(Rc::clone(&writes));
        let mut ui = ui_with_clipboard(app, Box::new(clipboard));

        ui.copy_selected_task_id();

        let recorded = writes.borrow().last().cloned();
        assert_eq!(recorded, Some(task.to_string()));
        let message = ui.message.expect("info message must be set");
        assert!(matches!(message.level, MessageLevel::Info));
        assert!(message.text.contains("コピー"));
        Ok(())
    }

    #[test]
    fn copy_selected_task_id_reports_clipboard_failure() -> Result<()> {
        let task = TaskId::new();
        let store = MockStore::new().with_task(task, vec![created(task, 0, "Task")]);
        let app = App::new(store, WorkflowConfig::unrestricted())?;
        let mut ui = ui_with_clipboard(app, Box::new(FailingClipboard::new("broken clipboard")));

        ui.copy_selected_task_id();

        let message = ui.message.expect("error message must be set");
        assert!(matches!(message.level, MessageLevel::Error));
        assert!(
            message.text.contains("broken clipboard"),
            "actual text: {}",
            message.text
        );
        Ok(())
    }

    #[test]
    fn copy_selected_task_id_without_selection_shows_error() -> Result<()> {
        let store = MockStore::new();
        let app = App::new(store, WorkflowConfig::unrestricted())?;
        let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));

        ui.copy_selected_task_id();

        let message = ui.message.expect("error message must be set");
        assert!(matches!(message.level, MessageLevel::Error));
        assert!(message.text.contains("コピー対象"));
        Ok(())
    }

    #[test]
    fn add_comment_keeps_selection_and_updates_comments() -> Result<()> {
        let task_a = TaskId::new();
        let task_b = TaskId::new();

        let store = MockStore::new()
            .with_task(
                task_a,
                vec![event(
                    task_a,
                    ts(0),
                    EventKind::TaskCreated {
                        title: "A".into(),
                        labels: Vec::new(),
                        assignees: Vec::new(),
                        description: None,
                        state: None,
                        state_kind: None,
                    },
                )],
            )
            .with_task(
                task_b,
                vec![event(
                    task_b,
                    ts(1),
                    EventKind::TaskCreated {
                        title: "B".into(),
                        labels: Vec::new(),
                        assignees: Vec::new(),
                        description: None,
                        state: None,
                        state_kind: None,
                    },
                )],
            );

        let mut app = App::new(store, WorkflowConfig::unrestricted())?;
        app.selected = 1;
        let target = app.selected_task_id().expect("selected task id");
        app.add_comment(target, "hello".into(), &actor())?;

        assert_eq!(app.selected_task_id(), Some(target));
        assert_eq!(
            app.selected_task()
                .unwrap()
                .comments
                .last()
                .map(|c| c.body.as_str()),
            Some("hello")
        );
        // Commented task should move to the top.
        assert_eq!(app.selected, 0);
        Ok(())
    }

    #[test]
    fn create_task_registers_and_selects_new_entry() -> Result<()> {
        let store = MockStore::new();
        let mut app = App::new(store, WorkflowConfig::unrestricted())?;

        let data = NewTaskData {
            title: "Title".into(),
            state: Some("todo".into()),
            labels: vec!["type/docs".into()],
            assignees: Vec::new(),
            description: Some("Write documentation".into()),
            parent: None,
        };

        let id = app.create_task(data, &actor())?;
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.selected_task_id(), Some(id));

        let snap = &app.selected_task().unwrap().snapshot;
        assert_eq!(snap.title, "Title");
        assert_eq!(snap.state.as_deref(), Some("todo"));
        assert_eq!(snap.description, "Write documentation");
        let labels: Vec<&str> = snap.labels.iter().map(String::as_str).collect();
        assert_eq!(labels, vec!["type/docs"]);
        Ok(())
    }

    #[test]
    fn create_task_rejects_unknown_state() -> Result<()> {
        let store = MockStore::new();
        let workflow = WorkflowConfig::from_states(vec![WorkflowState::new("state/todo")]);
        let mut app = App::new(store, workflow)?;

        let data = NewTaskData {
            title: "Title".into(),
            state: Some("state/done".into()),
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            parent: None,
        };

        let err = app.create_task(data, &actor()).unwrap_err();
        assert!(err.to_string().contains("state 'state/done'"));
        Ok(())
    }

    #[test]
    fn create_task_applies_default_state() -> Result<()> {
        let store = MockStore::new();
        let workflow = WorkflowConfig::from_states_with_default(
            vec![WorkflowState::new("state/todo")],
            Some("state/todo"),
        );
        let mut app = App::new(store, workflow)?;

        let data = NewTaskData {
            title: "Title".into(),
            state: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            parent: None,
        };

        app.create_task(data, &actor())?;
        let snap = &app.selected_task().unwrap().snapshot;
        assert_eq!(snap.state.as_deref(), Some("state/todo"));
        Ok(())
    }

    #[test]
    fn comment_editor_output_strips_comments_and_trims() {
        let input = "# comment\nline1\n\nline2  \n# ignored";
        let parsed = parse_comment_editor_output(input);
        assert_eq!(parsed.as_deref(), Some("line1\n\nline2"));
    }

    #[test]
    fn comment_editor_output_none_when_empty() {
        let input = "# comment\n\n   \n# another comment";
        assert!(parse_comment_editor_output(input).is_none());
    }

    #[test]
    fn new_task_editor_output_parses_fields() {
        let raw = "\
# heading
title: Sample Task
state: state/todo
labels: type/docs, area/cli
assignees: alice, bob
---
This is description.
";
        let parsed = parse_new_task_editor_output(raw).expect("parse succeeds");
        let data = parsed.expect("should create task");
        assert_eq!(data.title, "Sample Task");
        assert_eq!(data.state.as_deref(), Some("state/todo"));
        assert_eq!(data.labels, vec!["type/docs".to_string(), "area/cli".to_string()]);
        assert_eq!(data.assignees, vec!["alice".to_string(), "bob".to_string()]);
        assert_eq!(data.description.as_deref(), Some("This is description."));
    }

    #[test]
    fn new_task_editor_output_none_when_all_empty() {
        let raw = "\
# heading
title:
state:
labels:
assignees:
---
# no description
";
        let parsed = parse_new_task_editor_output(raw).expect("parse succeeds");
        assert!(parsed.is_none());
    }

    #[test]
    fn new_task_editor_output_requires_title() {
        let raw = "\
title:
state: state/todo
labels: foo
assignees:
---
";
        let err = parse_new_task_editor_output(raw).expect_err("should error");
        assert_eq!(err, "タイトルを入力してください");
    }

    #[test]
    fn new_task_editor_template_prefills_default_state() {
        let template = new_task_editor_template(None, None, Some("state/todo"));
        assert!(template.contains("state: state/todo"));
    }

    #[test]
    fn filter_editor_output_parses_all_fields() {
        let parent = fixed_task_id(1);
        let child = fixed_task_id(2);
        let raw = format!(
            "\
states: state/todo,state/done
state_kinds: !done
labels: type/bug
assignees: alice
parents: {parent}
children: {child}
text: panic
updated_since: 2025-01-01T00:00:00Z
updated_until: 2025-01-02T00:00:00Z
"
        );
        let filter = parse_filter_editor_output(&raw).expect("parse succeeds");
        assert!(filter.states.contains("state/todo"));
        assert!(filter.labels.contains("type/bug"));
        assert!(filter.assignees.contains("alice"));
        assert!(filter.parents.contains(&parent));
        assert!(filter.children.contains(&child));
        assert_eq!(filter.text.as_deref(), Some("panic"));
        assert!(filter.state_kinds.exclude.contains(&StateKind::Done));
        let updated = filter.updated.expect("updated filter");
        let expected_since = OffsetDateTime::parse("2025-01-01T00:00:00Z", &Rfc3339).expect("ts");
        let expected_until = OffsetDateTime::parse("2025-01-02T00:00:00Z", &Rfc3339).expect("ts");
        assert_eq!(updated.since, Some(expected_since));
        assert_eq!(updated.until, Some(expected_until));
    }

    #[test]
    fn filter_editor_output_rejects_invalid_timestamp() {
        let err = parse_filter_editor_output("updated_since: invalid").expect_err("should error");
        assert!(err.contains("時刻"));
    }

    #[test]
    fn summarize_task_filter_lists_active_fields() {
        let mut filter = TaskFilter::default();
        filter.states.insert("state/todo".into());
        filter.text = Some("panic".into());
        let summary = summarize_task_filter(&filter);
        assert!(summary.contains("state=state/todo"));
        assert!(summary.contains("text=\"panic\""));
    }

    #[test]
    fn summarize_task_filter_includes_state_kind_clause() {
        let mut filter = TaskFilter::default();
        filter.state_kinds.exclude.insert(StateKind::Done);
        let summary = summarize_task_filter(&filter);
        assert!(summary.contains("state-kind!=done"));
    }

    #[test]
    fn parse_state_kind_filter_handles_include_and_exclude() {
        let filter = parse_state_kind_filter("in_progress, !done").expect("parse succeeds");
        assert!(filter.include.contains(&StateKind::InProgress));
        assert!(filter.exclude.contains(&StateKind::Done));
    }

    #[test]
    fn status_layout_allocates_space_for_filter_and_status_blocks() {
        let area = Rect::new(0, 0, 80, 12);
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints(Ui::<MockStore>::status_layout_constraints())
            .split(area);
        assert_eq!(rows.len(), 3);
        assert!(rows[1].height >= 3, "フィルタ欄の高さが不足しています");
        assert!(rows[2].height >= 3, "ステータス欄の高さが不足しています");
    }

    #[test]
    fn instructions_include_filter_shortcut() -> Result<()> {
        let task = TaskId::new();
        let store = MockStore::new().with_task(task, vec![created(task, 0, "Task")]);
        let app = App::new(store, WorkflowConfig::unrestricted())?;
        let ui = ui_with_clipboard(app, Box::new(NoopClipboard));
        assert!(
            ui.instructions().contains("f:フィルタ"),
            "instructions must mention filter shortcut"
        );
        Ok(())
    }

    #[test]
    fn parse_list_trims_entries() {
        assert_eq!(
            parse_list("one, two , , three"),
            vec!["one".to_owned(), "two".to_owned(), "three".to_owned()]
        );
    }

    #[test]
    fn update_task_applies_field_changes() -> Result<()> {
        let task = TaskId::new();
        let created = Event::new(
            task,
            &actor(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: vec!["type/bug".into(), "area/cli".into()],
                assignees: vec!["alice".into(), "carol".into()],
                description: Some("old description".into()),
                state: Some("state/in-progress".into()),
                state_kind: Some(StateKind::InProgress),
            },
        );

        let store = MockStore::new().with_task(task, vec![created]);
        let mut app = App::new(store, WorkflowConfig::unrestricted())?;

        let updated = app.update_task(
            task,
            NewTaskData {
                title: "Updated".into(),
                state: None,
                labels: vec!["type/docs".into()],
                assignees: vec!["bob".into()],
                description: Some("new description".into()),
                parent: None,
            },
            &actor(),
        )?;
        assert!(updated);

        let view = app
            .tasks
            .iter()
            .find(|view| view.snapshot.id == task)
            .expect("task should exist");
        assert_eq!(view.snapshot.title, "Updated");
        assert_eq!(view.snapshot.state, None);
        let labels: Vec<&str> = view.snapshot.labels.iter().map(String::as_str).collect();
        assert_eq!(labels, vec!["type/docs"]);
        let assignees: Vec<&str> = view.snapshot.assignees.iter().map(String::as_str).collect();
        assert_eq!(assignees, vec!["bob"]);
        assert_eq!(view.snapshot.description, "new description");

        let events = app.store.events.borrow();
        let stored = events.get(&task).expect("events for task");
        assert_eq!(stored.len(), 8);
        assert!(
            stored
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::TaskTitleSet { .. }))
        );
        assert!(
            stored
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::TaskStateCleared))
        );
        assert!(
            stored
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::TaskDescriptionSet { .. }))
        );
        assert!(
            stored
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::LabelsAdded { .. }))
        );
        assert!(
            stored
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::LabelsRemoved { .. }))
        );
        assert!(
            stored
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::AssigneesAdded { .. }))
        );
        assert!(
            stored
                .iter()
                .any(|ev| matches!(ev.kind, EventKind::AssigneesRemoved { .. }))
        );
        Ok(())
    }

    #[test]
    fn update_task_returns_false_when_no_diff() -> Result<()> {
        let task = TaskId::new();
        let created = Event::new(
            task,
            &actor(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: vec!["type/bug".into()],
                assignees: vec!["alice".into()],
                description: Some("desc".into()),
                state: Some("state/todo".into()),
                state_kind: Some(StateKind::Todo),
            },
        );

        let store = MockStore::new().with_task(task, vec![created]);
        let mut app = App::new(store, WorkflowConfig::unrestricted())?;
        let snapshot = {
            let events = app.store.events.borrow();
            let stored = events.get(&task).expect("events for task");
            TaskSnapshot::replay(stored)
        };

        let updated = app.update_task(
            task,
            NewTaskData {
                title: snapshot.title.clone(),
                state: snapshot.state.clone(),
                labels: snapshot.labels.iter().cloned().collect(),
                assignees: snapshot.assignees.iter().cloned().collect(),
                description: if snapshot.description.is_empty() {
                    None
                } else {
                    Some(snapshot.description)
                },
                parent: None,
            },
            &actor(),
        )?;
        assert!(!updated);

        let events = app.store.events.borrow();
        let stored = events.get(&task).expect("events for task");
        assert_eq!(stored.len(), 1);
        Ok(())
    }

    #[test]
    fn set_task_state_applies_new_value() -> Result<()> {
        let task = TaskId::new();
        let created = Event::new(
            task,
            &actor(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: Some("state/todo".into()),
                state_kind: Some(StateKind::Todo),
            },
        );
        let workflow = WorkflowConfig::from_states(vec![
            WorkflowState::new("state/todo"),
            WorkflowState::new("state/done"),
        ]);
        let store = MockStore::new().with_task(task, vec![created]);
        let mut app = App::new(store, workflow)?;

        let changed = app.set_task_state(task, Some("state/done".into()), &actor())?;
        assert!(changed);

        let view = app
            .tasks
            .iter()
            .find(|view| view.snapshot.id == task)
            .expect("task should exist");
        assert_eq!(view.snapshot.state.as_deref(), Some("state/done"));

        let events = app.store.events.borrow();
        let stored = events.get(&task).expect("events for task");
        assert_eq!(stored.len(), 2);
        assert!(
            stored
                .iter()
                .any(|ev| matches!(&ev.kind, EventKind::TaskStateSet { state, .. } if state == "state/done"))
        );
        Ok(())
    }

    #[test]
    fn set_task_state_returns_false_when_unchanged() -> Result<()> {
        let task = TaskId::new();
        let created = Event::new(
            task,
            &actor(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: Some("state/todo".into()),
                state_kind: Some(StateKind::Todo),
            },
        );
        let workflow = WorkflowConfig::from_states(vec![WorkflowState::new("state/todo")]);
        let store = MockStore::new().with_task(task, vec![created]);
        let mut app = App::new(store, workflow)?;

        let changed = app.set_task_state(task, Some("state/todo".into()), &actor())?;
        assert!(!changed);

        let events = app.store.events.borrow();
        let stored = events.get(&task).expect("events for task");
        assert_eq!(stored.len(), 1);
        Ok(())
    }

    #[test]
    fn open_state_picker_prefills_current_state() -> Result<()> {
        let task = TaskId::new();
        let created = Event::new(
            task,
            &actor(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: Some("state/done".into()),
                state_kind: Some(StateKind::Done),
            },
        );
        let workflow = WorkflowConfig::from_states(vec![
            WorkflowState::new("state/todo"),
            WorkflowState::new("state/done"),
        ]);
        let store = MockStore::new().with_task(task, vec![created]);
        let app = App::new(store, workflow)?;
        let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));
        ui.app.set_filter(TaskFilter::default());
        ui.open_state_picker();

        assert_eq!(ui.detail_focus, DetailFocus::StatePicker);
        let picker = ui.state_picker.as_ref().expect("state picker");
        assert_eq!(
            picker.options[picker.selected].value.as_deref(),
            Some("state/done")
        );
        Ok(())
    }

    #[test]
    fn apply_state_picker_selection_updates_state() -> Result<()> {
        let task = TaskId::new();
        let created = Event::new(
            task,
            &actor(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: Some("state/todo".into()),
                state_kind: Some(StateKind::Todo),
            },
        );
        let workflow = WorkflowConfig::from_states(vec![
            WorkflowState::new("state/todo"),
            WorkflowState::new("state/done"),
        ]);
        let store = MockStore::new().with_task(task, vec![created]);
        let app = App::new(store, workflow)?;
        let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));

        ui.open_state_picker();
        ui.state_picker_down();
        ui.apply_state_picker_selection();

        let view = ui
            .app
            .tasks
            .iter()
            .find(|view| view.snapshot.id == task)
            .expect("task exists");
        assert_eq!(view.snapshot.state.as_deref(), Some("state/done"));
        assert!(ui.state_picker.is_none());
        assert_eq!(ui.detail_focus, DetailFocus::None);
        Ok(())
    }
}
