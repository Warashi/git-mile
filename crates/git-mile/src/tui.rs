//! Terminal UI for browsing and updating tasks.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::{self, Stdout, Write};
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossterm::{
    event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_core::TaskSnapshot;
use git_mile_store_git::GitStore;
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use tempfile::NamedTempFile;
use time::OffsetDateTime;
use tracing::subscriber::NoSubscriber;

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
        let snapshot = TaskSnapshot::replay(events);

        let mut sorted_refs: Vec<&Event> = events.iter().collect();
        sorted_refs.sort_by(|a, b| match a.ts.cmp(&b.ts) {
            Ordering::Equal => a.id.cmp(&b.id),
            other => other,
        });

        let comments = sorted_refs
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

        let last_updated = sorted_refs.last().map(|ev| ev.ts);

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
    /// Cached task list sorted by most recent updates.
    pub tasks: Vec<TaskView>,
    /// Currently selected task index.
    pub selected: usize,
}

impl<S: TaskStore> App<S> {
    /// Create an application instance and eagerly load tasks.
    pub fn new(store: S) -> Result<Self> {
        let mut app = Self {
            store,
            tasks: Vec::new(),
            selected: 0,
        };
        app.refresh_tasks()?;
        Ok(app)
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
        self.tasks = views;
        if let Some(id) = keep_id {
            self.selected = self
                .tasks
                .iter()
                .position(|view| view.snapshot.id == id)
                .unwrap_or(0);
        } else if self.tasks.is_empty() {
            self.selected = 0;
        } else if self.selected >= self.tasks.len() {
            self.selected = self.tasks.len() - 1;
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
        self.refresh_tasks_with(Some(task))?;
        Ok(task)
    }

    /// Update an existing task and refresh the view. Returns `true` when any changes were applied.
    #[allow(clippy::too_many_lines)]
    pub fn update_task(&mut self, task: TaskId, data: NewTaskData, actor: &Actor) -> Result<bool> {
        let current = self
            .tasks
            .iter()
            .find(|view| view.snapshot.id == task)
            .cloned()
            .map_or_else(
                || {
                    self.store
                        .load_events(task)
                        .map(|events| TaskView::from_events(&events))
                        .context("タスクの読み込みに失敗しました")
                },
                Ok,
            )?;

        let NewTaskData {
            title,
            state,
            labels,
            assignees,
            description,
        } = data;

        let mut events = Vec::new();

        if title != current.snapshot.title {
            events.push(Event::new(task, actor, EventKind::TaskTitleSet { title }));
        }

        match (current.snapshot.state.as_ref(), state.as_ref()) {
            (Some(old), Some(new)) if old != new => events.push(Event::new(
                task,
                actor,
                EventKind::TaskStateSet { state: new.clone() },
            )),
            (None, Some(new)) => events.push(Event::new(
                task,
                actor,
                EventKind::TaskStateSet { state: new.clone() },
            )),
            (Some(_), None) => events.push(Event::new(task, actor, EventKind::TaskStateCleared)),
            _ => {}
        }

        let current_labels = &current.snapshot.labels;
        let new_labels: BTreeSet<String> = labels.into_iter().collect();
        let added_labels: Vec<String> = new_labels.difference(current_labels).cloned().collect();
        if !added_labels.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::LabelsAdded { labels: added_labels },
            ));
        }
        let removed_labels: Vec<String> = current_labels.difference(&new_labels).cloned().collect();
        if !removed_labels.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::LabelsRemoved {
                    labels: removed_labels,
                },
            ));
        }

        let current_assignees = &current.snapshot.assignees;
        let new_assignees: BTreeSet<String> = assignees.into_iter().collect();
        let added_assignees: Vec<String> = new_assignees.difference(current_assignees).cloned().collect();
        if !added_assignees.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::AssigneesAdded {
                    assignees: added_assignees,
                },
            ));
        }
        let removed_assignees: Vec<String> = current_assignees.difference(&new_assignees).cloned().collect();
        if !removed_assignees.is_empty() {
            events.push(Event::new(
                task,
                actor,
                EventKind::AssigneesRemoved {
                    assignees: removed_assignees,
                },
            ));
        }

        let new_description = description.unwrap_or_default();
        if current.snapshot.description != new_description {
            if new_description.is_empty() {
                events.push(Event::new(
                    task,
                    actor,
                    EventKind::TaskDescriptionSet { description: None },
                ));
            } else {
                events.push(Event::new(
                    task,
                    actor,
                    EventKind::TaskDescriptionSet {
                        description: Some(new_description),
                    },
                ));
            }
        }

        if events.is_empty() {
            return Ok(false);
        }

        for event in &events {
            self.store
                .append_event(event)
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
}

/// Launch the interactive TUI.
pub fn run(store: GitStore) -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let result =
        tracing::subscriber::with_default(NoSubscriber::default(), || run_event_loop(&mut terminal, store));

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result
}

fn run_event_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, store: GitStore) -> Result<()> {
    let actor = resolve_actor();
    let app = App::new(store)?;
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

struct Ui<S: TaskStore> {
    app: App<S>,
    actor: Actor,
    message: Option<Message>,
    should_quit: bool,
}

impl<S: TaskStore> Ui<S> {
    fn new(app: App<S>, actor: Actor) -> Self {
        let _ = thread::current().id();
        Self {
            app,
            actor,
            message: None,
            should_quit: false,
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
            self.app
                .tasks
                .iter()
                .map(|view| {
                    let title = Span::styled(
                        &view.snapshot.title,
                        Style::default().add_modifier(Modifier::BOLD),
                    );
                    let meta = format!(
                        "{} | {}",
                        view.snapshot.id,
                        view.snapshot.state.as_deref().unwrap_or("state/unknown")
                    );
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
        let block = Block::default().title("詳細").borders(Borders::ALL);
        let inner = block.inner(area);
        f.render_widget(block, area);

        if let Some(task) = self.app.selected_task() {
            let mut lines = Vec::new();
            lines.push(Line::from(Span::styled(
                &task.snapshot.title,
                Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan),
            )));
            lines.push(Line::from(format!("ID: {}", task.snapshot.id)));
            if let Some(state) = &task.snapshot.state {
                lines.push(Line::from(format!("状態: {state}")));
            }
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
        } else {
            let paragraph = Paragraph::new("タスクが選択されていません").wrap(Wrap { trim: false });
            f.render_widget(paragraph, inner);
        }
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

    fn handle_key(&mut self, key: KeyEvent) -> Result<Option<UiAction>> {
        if key.kind != KeyEventKind::Press {
            return Ok(None);
        }

        self.handle_browse_key(key)
    }

    fn handle_browse_key(&mut self, key: KeyEvent) -> Result<Option<UiAction>> {
        match key.code {
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
            _ => Ok(None),
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
        format!(
            "j/k・↑↓:移動  n:新規  e:編集  c:コメント  r:再読込  q/Esc:終了  [{} <{}>]",
            self.actor.name, self.actor.email
        )
    }

    fn status_text(&self) -> String {
        self.message.as_ref().map_or_else(
            || "ステータスメッセージはありません".into(),
            |msg| msg.text.clone(),
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
            let view = ui.app.tasks.iter().find(|view| view.snapshot.id == task).cloned();
            let Some(view) = view else {
                ui.error("編集対象のタスクが見つかりません");
                return Ok(());
            };
            let template = edit_task_editor_template(&view);
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_edit_task_input(task, &raw)?;
        }
        UiAction::CreateTask => {
            let template = new_task_editor_template();
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_new_task_input(&raw)?;
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

fn edit_task_editor_template(task: &TaskView) -> String {
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
        format!("state: {}", state),
        format!("labels: {}", labels),
        format!("assignees: {}", assignees),
        "---".to_string(),
        "# この下で説明を編集してください。空欄で説明を削除します。".to_string(),
    ];
    if snapshot.description.is_empty() {
        lines.push(String::new());
    } else {
        lines.extend(snapshot.description.lines().map(str::to_owned));
    }
    lines.push(String::new());
    lines.join("\n")
}

fn new_task_editor_template() -> String {
    [
        "# 新規タスクを作成します。タイトルは必須です。",
        "# 空のまま保存すると作成をキャンセルしたものとして扱います。",
        "title: ",
        "state: ",
        "labels: ",
        "assignees: ",
        "---",
        "# この下に説明をMarkdown形式で記入してください。不要なら空のままにしてください。",
        "",
    ]
    .join("\n")
}

fn parse_new_task_editor_output(raw: &str) -> Result<Option<NewTaskData>, String> {
    let mut title = String::new();
    let mut state = String::new();
    let mut labels = String::new();
    let mut assignees = String::new();
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
                "title" => value.clone_into(&mut title),
                "state" => value.clone_into(&mut state),
                "labels" => value.clone_into(&mut labels),
                "assignees" => value.clone_into(&mut assignees),
                unknown => {
                    return Err(format!("未知のフィールドです: {unknown}"));
                }
            }
        } else {
            return Err(format!("フィールドの形式が正しくありません: {trimmed}"));
        }
    }

    let title = title.trim().to_owned();
    let state = state.trim();
    let labels = labels.trim();
    let assignees = assignees.trim();
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
        title,
        state,
        labels,
        assignees,
        description,
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
    use anyhow::Result;
    use git_mile_core::event::EventKind;
    use std::cell::RefCell;
    use std::collections::HashMap;
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

        let app = App::new(store)?;
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

        let mut app = App::new(store)?;
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
        let mut app = App::new(store)?;

        let data = NewTaskData {
            title: "Title".into(),
            state: Some("todo".into()),
            labels: vec!["type/docs".into()],
            assignees: Vec::new(),
            description: Some("Write documentation".into()),
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
        let mut app = App::new(store)?;

        let updated = app.update_task(
            task,
            NewTaskData {
                title: "Updated".into(),
                state: None,
                labels: vec!["type/docs".into()],
                assignees: vec!["bob".into()],
                description: Some("new description".into()),
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
        assert_eq!(
            view.snapshot.labels.iter().cloned().collect::<Vec<_>>(),
            vec!["type/docs".to_owned()]
        );
        assert_eq!(
            view.snapshot.assignees.iter().cloned().collect::<Vec<_>>(),
            vec!["bob".to_owned()]
        );
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
        let mut app = App::new(store)?;
        let snapshot = app
            .tasks
            .iter()
            .find(|view| view.snapshot.id == task)
            .expect("task should exist")
            .snapshot
            .clone();

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
