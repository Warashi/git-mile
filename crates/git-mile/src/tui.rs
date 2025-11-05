//! Terminal UI for browsing and updating tasks.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::env;
use std::fs;
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::WorkflowConfig;
#[cfg(test)]
use crate::config::WorkflowState;
use anyhow::{anyhow, Context, Result};
use arboard::Clipboard as ArboardClipboard;
use base64::{engine::general_purpose::STANDARD as Base64Standard, Engine as _};
use crossterm::{
    event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_core::{OrderedEvents, TaskSnapshot};
use git_mile_store_git::GitStore;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use tempfile::NamedTempFile;
use time::OffsetDateTime;
use tracing::{subscriber::NoSubscriber, warn};

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
    /// Cached task list sorted by most recent updates.
    pub tasks: Vec<TaskView>,
    /// Currently selected task index.
    pub selected: usize,
    task_index: HashMap<TaskId, usize>,
    parents_index: HashMap<TaskId, Vec<TaskId>>,
    children_index: HashMap<TaskId, Vec<TaskId>>,
}

impl<S: TaskStore> App<S> {
    /// Create an application instance and eagerly load tasks.
    pub fn new(store: S, workflow: WorkflowConfig) -> Result<Self> {
        let mut app = Self {
            store,
            workflow,
            tasks: Vec::new(),
            selected: 0,
            task_index: HashMap::new(),
            parents_index: HashMap::new(),
            children_index: HashMap::new(),
        };
        app.refresh_tasks()?;
        Ok(app)
    }

    pub const fn workflow(&self) -> &WorkflowConfig {
        &self.workflow
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
        if let Some(index) = self.task_index.get(&task_id).copied() {
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

        if let Some(id) = keep_id {
            self.selected = if self.tasks.is_empty() {
                0
            } else {
                self.task_index.get(&id).copied().unwrap_or(0)
            };
        } else if self.tasks.is_empty() {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(self.tasks.len() - 1);
        }
        Ok(())
    }

    /// Selected task (if any).
    pub fn selected_task(&self) -> Option<&TaskView> {
        self.tasks.get(self.selected)
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
        if self.tasks.is_empty() {
            return;
        }
        if self.selected + 1 < self.tasks.len() {
            self.selected += 1;
        }
    }

    /// Move selection to the previous task.
    pub fn select_prev(&mut self) {
        Self::runtime_touch();
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
    pub fn create_task(&mut self, data: NewTaskData, actor: &Actor) -> Result<TaskId> {
        self.workflow.validate_state(data.state.as_deref())?;

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
            },
        );
        self.store.append_event(&event)?;

        // Create ChildLinked event if parent is specified
        if let Some(parent) = data.parent {
            let link_event = Event::new(task, actor, EventKind::ChildLinked { parent, child: task });
            self.store.append_event(&link_event)?;
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

        for event in patch.into_events(task, actor) {
            self.store
                .append_event(&event)
                .context("タスク更新イベントの書き込みに失敗しました")?;
        }
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

    fn into_events(self, task: TaskId, actor: &Actor) -> Vec<Event> {
        let mut events = Vec::new();

        if let Some(title) = self.title {
            events.push(Event::new(task, actor, EventKind::TaskTitleSet { title }));
        }

        if let Some(state) = self.state {
            events.push(match state {
                StatePatch::Set { state } => Event::new(task, actor, EventKind::TaskStateSet { state }),
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
                if let Some(action) = ui.handle_key(key)? {
                    if let Err(err) = handle_ui_action(terminal, &mut ui, action) {
                        ui.error(format!("エディタ処理中に失敗しました: {err}"));
                    }
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

    /// Recursively collect visible nodes into a vector.
    fn collect_visible_nodes_into(node: &TreeNode, depth: usize, visible_nodes: &mut Vec<(usize, TaskId)>) {
        visible_nodes.push((depth, node.task_id));
        if node.expanded {
            for child in &node.children {
                Self::collect_visible_nodes_into(child, depth + 1, visible_nodes);
            }
        }
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

/// Focus state for detail view components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DetailFocus {
    /// No focus (browsing task list).
    None,
    /// Focus on tree view (floating window).
    TreeView,
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

struct Ui<S: TaskStore> {
    app: App<S>,
    actor: Actor,
    message: Option<Message>,
    should_quit: bool,
    /// Current focus in detail view.
    detail_focus: DetailFocus,
    /// Tree view state.
    tree_state: TreeViewState,
    clipboard: Box<dyn ClipboardSink>,
}

impl<S: TaskStore> Ui<S> {
    fn new(app: App<S>, actor: Actor) -> Self {
        let clipboard = default_clipboard();
        Self::with_clipboard(app, actor, clipboard)
    }

    fn with_clipboard(app: App<S>, actor: Actor, clipboard: Box<dyn ClipboardSink>) -> Self {
        let _ = thread::current().id();
        Self {
            app,
            actor,
            message: None,
            should_quit: false,
            detail_focus: DetailFocus::None,
            tree_state: TreeViewState::new(),
            clipboard,
        }
    }

    fn draw(&self, f: &mut ratatui::Frame<'_>) {
        let size = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(7)])
            .split(size);

        self.draw_main(f, chunks[0]);
        self.draw_status(f, chunks[1]);

        // Draw tree view on top if active
        if self.detail_focus == DetailFocus::TreeView {
            self.draw_tree_view_popup(f);
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
        let items = if self.app.tasks.is_empty() {
            vec![ListItem::new(Line::from("タスクがありません"))]
        } else {
            let workflow = self.app.workflow();
            self.app
                .tasks
                .iter()
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
        };

        let list = List::new(items)
            .block(Block::default().title("タスクリスト").borders(Borders::ALL))
            .highlight_style(Style::default().add_modifier(Modifier::REVERSED))
            .highlight_symbol("▶ ");
        let mut state = ListState::default();
        if !self.app.tasks.is_empty() {
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
            let parent_title: Cow<'_, str> = if parent.snapshot.title.len() > 20 {
                Cow::Owned(format!("{}...", &parent.snapshot.title[..17]))
            } else {
                Cow::Borrowed(parent.snapshot.title.as_str())
            };
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
                    .map(|p| {
                        if p.snapshot.title.len() > 15 {
                            Cow::Owned(format!("{}...", &p.snapshot.title[..12]))
                        } else {
                            Cow::Borrowed(p.snapshot.title.as_str())
                        }
                    })
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
                let state_marker = workflow.state_marker(state_value);
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
            .constraints([Constraint::Length(4), Constraint::Length(3)])
            .split(area);

        let instructions = Paragraph::new(self.instructions())
            .block(Block::default().title("操作").borders(Borders::ALL))
            .wrap(Wrap { trim: true });
        f.render_widget(instructions, rows[0]);

        let message = Paragraph::new(self.status_text())
            .block(Block::default().title("ステータス").borders(Borders::ALL))
            .style(self.status_style());
        f.render_widget(message, rows[1]);
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
            let state_marker = workflow.state_marker(state_value);
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

    /// Build tree starting from a root task.
    fn build_tree_from_root(&self, root_id: TaskId) -> Option<TreeNode> {
        let root_view = self.app.tasks.iter().find(|t| t.snapshot.id == root_id)?;

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
        if let Some(parent) = parents.first() {
            if let Some(index) = self
                .tree_state
                .visible_nodes
                .iter()
                .position(|(_, id)| *id == parent.snapshot.id)
            {
                self.tree_state.selected = index;
            }
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

    fn info(&mut self, message: impl Into<String>) {
        self.message = Some(Message::info(message));
    }

    fn error(&mut self, message: impl Into<String>) {
        self.message = Some(Message::error(message));
    }

    fn instructions(&self) -> String {
        match self.detail_focus {
            DetailFocus::None => {
                let base =
                    "j/k:移動 ↵:ツリー n:新規 s:子タスク e:編集 c:コメント r:再読込 p:親へ y:IDコピー q:終了";
                format!("{} [{} <{}>]", base, self.actor.name, self.actor.email)
            }
            DetailFocus::TreeView => "j/k:移動 h:閉じる l:開く ↵:ジャンプ q/Esc:閉じる".to_string(),
        }
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
        if let Some(msg) = &self.message {
            if msg.is_expired(Duration::from_secs(5)) {
                self.message = None;
            }
        }
    }
}

#[derive(Clone, Copy)]
enum UiAction {
    AddComment { task: TaskId },
    EditTask { task: TaskId },
    CreateTask,
    CreateSubtask { parent: TaskId },
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
            let template = new_task_editor_template(None, hint.as_deref());
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_new_task_input(&raw)?;
        }
        UiAction::CreateSubtask { parent } => {
            let parent_view = ui.app.tasks.iter().find(|view| view.snapshot.id == parent);
            let hint = ui.app.workflow().state_hint();
            let template = new_task_editor_template(parent_view, hint.as_deref());
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_new_subtask_input(parent, &raw)?;
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

fn new_task_editor_template(parent: Option<&TaskView>, state_hint: Option<&str>) -> String {
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
    lines.extend([
        "state: ".to_string(),
        "labels: ".to_string(),
        "assignees: ".to_string(),
        "---".to_string(),
        "# この下に説明をMarkdown形式で記入してください。不要なら空のままにしてください。".to_string(),
        String::new(),
    ]);
    lines.join("\n")
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
    use anyhow::{anyhow, Result};
    use git_mile_core::event::EventKind;
    use std::cell::RefCell;
    use std::collections::{BTreeSet, HashMap};
    use std::rc::Rc;
    use std::str::FromStr;
    use time::OffsetDateTime;

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
            },
        )
    }

    fn child_link(secs: i64, parent: TaskId, child: TaskId) -> Event {
        event(child, ts(secs), EventKind::ChildLinked { parent, child })
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
            },
        )];

        let store = MockStore::new()
            .with_task(task_a, events_a)
            .with_task(task_b, events_b);

        let app = App::new(store, WorkflowConfig::default())?;
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
        let app = App::new(store, WorkflowConfig::default())?;

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
        let app = App::new(store, WorkflowConfig::default())?;

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
        let app = App::new(store, WorkflowConfig::default())?;

        let root_view = app.get_root(leaf).expect("must locate a root despite cycle");
        assert_eq!(root_view.snapshot.id, root);
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
                },
            ),
            event(child, ts(20), EventKind::ChildLinked { parent, child }),
        ];

        let store = MockStore::new()
            .with_task(parent, parent_events)
            .with_task(child, child_events);
        let app = App::new(store, WorkflowConfig::default())?;
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
    fn copy_selected_task_id_writes_to_clipboard() -> Result<()> {
        let task = TaskId::new();
        let store = MockStore::new().with_task(task, vec![created(task, 0, "Task")]);
        let app = App::new(store, WorkflowConfig::default())?;
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
        let app = App::new(store, WorkflowConfig::default())?;
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
        let app = App::new(store, WorkflowConfig::default())?;
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
                    },
                )],
            );

        let mut app = App::new(store, WorkflowConfig::default())?;
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
        let mut app = App::new(store, WorkflowConfig::default())?;

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
            },
        );

        let store = MockStore::new().with_task(task, vec![created]);
        let mut app = App::new(store, WorkflowConfig::default())?;

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
        assert!(stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::TaskTitleSet { .. })));
        assert!(stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::TaskStateCleared)));
        assert!(stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::TaskDescriptionSet { .. })));
        assert!(stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::LabelsAdded { .. })));
        assert!(stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::LabelsRemoved { .. })));
        assert!(stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::AssigneesAdded { .. })));
        assert!(stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::AssigneesRemoved { .. })));
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
            },
        );

        let store = MockStore::new().with_task(task, vec![created]);
        let mut app = App::new(store, WorkflowConfig::default())?;
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
}
