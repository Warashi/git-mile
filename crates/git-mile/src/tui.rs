//! Terminal UI for browsing and updating tasks.

use std::cmp::Ordering;
use std::io::{self, Stdout};
use std::mem;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event as CrosstermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
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
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};
use time::OffsetDateTime;
use tui_textarea::TextArea;

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
    fn from_events(mut events: Vec<Event>) -> Self {
        events.sort_by(|a, b| match a.ts.cmp(&b.ts) {
            Ordering::Equal => a.id.cmp(&b.id),
            other => other,
        });
        let snapshot = TaskSnapshot::replay(events.clone());
        let comments = events
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

        let last_updated = events.last().map(|ev| ev.ts);

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
            views.push(TaskView::from_events(events));
        }
        views.sort_by(|a, b| match (a.last_updated, b.last_updated) {
            (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.snapshot.id.cmp(&b.snapshot.id),
        });
        self.tasks = views;
        if let Some(id) = keep_id {
            if let Some(idx) = self.tasks.iter().position(|view| view.snapshot.id == id) {
                self.selected = idx;
            } else if !self.tasks.is_empty() {
                self.selected = 0;
            } else {
                self.selected = 0;
            }
        } else if !self.tasks.is_empty() && self.selected >= self.tasks.len() {
            self.selected = self.tasks.len() - 1;
        } else if self.tasks.is_empty() {
            self.selected = 0;
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

    /// Move selection to the next task.
    pub fn select_next(&mut self) {
        if self.tasks.is_empty() {
            return;
        }
        if self.selected + 1 < self.tasks.len() {
            self.selected += 1;
        }
    }

    /// Move selection to the previous task.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Append a comment to the given task and refresh the view.
    pub fn add_comment(&mut self, task: TaskId, body: String, actor: Actor) -> Result<()> {
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
    pub fn create_task(&mut self, data: NewTaskData, actor: Actor) -> Result<TaskId> {
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
}

/// Input collected from the new task form.
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

    let result = run_event_loop(&mut terminal, store);

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
            match event::read()? {
                CrosstermEvent::Key(key) => ui.handle_key(key)?,
                CrosstermEvent::Resize(_, _) => ui.request_redraw(),
                _ => {}
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
    mode: Mode,
    message: Option<Message>,
    should_quit: bool,
}

impl<S: TaskStore> Ui<S> {
    fn new(app: App<S>, actor: Actor) -> Self {
        Self {
            app,
            actor,
            mode: Mode::Browse,
            message: None,
            should_quit: false,
        }
    }

    fn draw(&mut self, f: &mut ratatui::Frame<'_>) {
        let size = f.area();
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(5), Constraint::Length(7)])
            .split(size);

        self.draw_main(f, chunks[0]);
        self.draw_status(f, chunks[1]);

        if let Mode::Comment(input) = &mut self.mode {
            render_comment_overlay(f, size, input);
        }
        if let Mode::NewTask(form) = &mut self.mode {
            render_new_task_overlay(f, size, form);
        }
    }

    fn draw_main(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
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

    fn draw_task_list(&mut self, f: &mut ratatui::Frame<'_>, area: Rect) {
        let items = if self.app.tasks.is_empty() {
            vec![ListItem::new(Line::from("タスクがありません"))]
        } else {
            self.app
                .tasks
                .iter()
                .map(|view| {
                    let title = Span::styled(
                        view.snapshot.title.clone(),
                        Style::default().add_modifier(Modifier::BOLD),
                    );
                    let meta = format!(
                        "{} | {}",
                        view.snapshot.id,
                        view.snapshot
                            .state
                            .clone()
                            .unwrap_or_else(|| "state/unknown".into())
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
                task.snapshot.title.clone(),
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
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                lines.push(Line::from(format!("ラベル: {labels}")));
            }
            if !task.snapshot.assignees.is_empty() {
                let assignees = task
                    .snapshot
                    .assignees
                    .iter()
                    .map(|s| s.as_str())
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

    fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        let current_mode = mem::replace(&mut self.mode, Mode::Browse);
        let next_mode = match current_mode {
            Mode::Browse => self.handle_browse_key(key)?,
            Mode::Comment(input) => self.handle_comment_mode(key, input)?,
            Mode::NewTask(form) => self.handle_new_task_mode(key, form)?,
        };
        self.mode = next_mode;
        Ok(())
    }

    fn handle_browse_key(&mut self, key: KeyEvent) -> Result<Mode> {
        let next_mode = match key.code {
            KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                self.should_quit = true;
                Mode::Browse
            }
            KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                self.app.select_next();
                Mode::Browse
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
                self.app.select_prev();
                Mode::Browse
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.app.refresh_tasks()?;
                self.info("タスクを再読込しました");
                Mode::Browse
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                if let Some(task) = self.app.selected_task_id() {
                    Mode::Comment(CommentInput::new(task))
                } else {
                    self.error("コメント対象のタスクが選択されていません");
                    Mode::Browse
                }
            }
            KeyCode::Char('n') | KeyCode::Char('N') => Mode::NewTask(NewTaskForm::new()),
            _ => Mode::Browse,
        };
        Ok(next_mode)
    }

    fn handle_comment_mode(&mut self, key: KeyEvent, mut input: CommentInput) -> Result<Mode> {
        if key.code == KeyCode::Esc {
            self.info("コメントをキャンセルしました");
            return Ok(Mode::Browse);
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') | KeyCode::Enter => {
                    let body = input.textarea.lines().join("\n");
                    let trimmed = body.trim();
                    if trimmed.is_empty() {
                        self.error("コメント本文が空です");
                        return Ok(Mode::Comment(input));
                    }
                    self.app
                        .add_comment(input.task, trimmed.to_owned(), self.actor.clone())?;
                    self.info("コメントを追加しました");
                    return Ok(Mode::Browse);
                }
                _ => {}
            }
        }

        input.textarea.input(key);
        Ok(Mode::Comment(input))
    }

    fn handle_new_task_mode(&mut self, key: KeyEvent, mut form: NewTaskForm) -> Result<Mode> {
        match form.handle_key(key) {
            FormAction::Continue => Ok(Mode::NewTask(form)),
            FormAction::Cancel => {
                self.info("新規タスク作成をキャンセルしました");
                Ok(Mode::Browse)
            }
            FormAction::Submit => match form.build_data() {
                Ok(data) => {
                    let id = self.app.create_task(data, self.actor.clone())?;
                    self.info(format!("タスクを作成しました: {id}"));
                    Ok(Mode::Browse)
                }
                Err(msg) => {
                    self.error(msg);
                    Ok(Mode::NewTask(form))
                }
            },
        }
    }

    fn info(&mut self, message: impl Into<String>) {
        self.message = Some(Message::info(message));
    }

    fn error(&mut self, message: impl Into<String>) {
        self.message = Some(Message::error(message));
    }

    fn instructions(&self) -> String {
        match self.mode {
            Mode::Browse => format!(
                "j/k・↑↓:移動  n:新規  c:コメント  r:再読込  q/Esc:終了  [{} <{}>]",
                self.actor.name, self.actor.email
            ),
            Mode::Comment(_) => "Ctrl+S / Ctrl+Enter:送信  Esc:キャンセル".into(),
            Mode::NewTask(_) => "Tab/Shift+Tab:移動  Ctrl+S / Ctrl+Enter:作成  Esc:キャンセル".into(),
        }
    }

    fn status_text(&self) -> String {
        if let Some(msg) = &self.message {
            msg.text.clone()
        } else {
            "ステータスメッセージはありません".into()
        }
    }

    fn status_style(&self) -> Style {
        if let Some(msg) = &self.message {
            msg.style()
        } else {
            Style::default()
        }
    }

    fn tick(&mut self) {
        if let Some(msg) = &self.message {
            if msg.is_expired(Duration::from_secs(5)) {
                self.message = None;
            }
        }
    }

    fn request_redraw(&mut self) {}
}

enum Mode {
    Browse,
    Comment(CommentInput),
    NewTask(NewTaskForm),
}

struct CommentInput {
    task: TaskId,
    textarea: TextArea<'static>,
}

impl CommentInput {
    fn new(task: TaskId) -> Self {
        let mut textarea = TextArea::default();
        textarea.set_placeholder_text("コメントを入力してください");
        Self { task, textarea }
    }
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

struct NewTaskForm {
    step: TaskFormStep,
    title: LineEditor,
    state: LineEditor,
    labels: LineEditor,
    assignees: LineEditor,
    description: TextArea<'static>,
}

impl NewTaskForm {
    fn new() -> Self {
        let mut description = TextArea::default();
        description.set_placeholder_text("Markdown 形式で説明を入力できます");
        Self {
            step: TaskFormStep::Title,
            title: LineEditor::default(),
            state: LineEditor::default(),
            labels: LineEditor::default(),
            assignees: LineEditor::default(),
            description,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) -> FormAction {
        if key.code == KeyCode::Esc {
            return FormAction::Cancel;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('s') | KeyCode::Enter => return FormAction::Submit,
                _ => {}
            }
        }

        match key.code {
            KeyCode::Tab => {
                self.step = self.step.next();
                FormAction::Continue
            }
            KeyCode::BackTab => {
                self.step = self.step.prev();
                FormAction::Continue
            }
            KeyCode::Enter => {
                if self.step == TaskFormStep::Description {
                    self.description.input(key);
                } else {
                    self.step = self.step.next();
                }
                FormAction::Continue
            }
            _ => {
                if self.step == TaskFormStep::Description {
                    self.description.input(key);
                } else if let Some(editor) = self.current_line_editor_mut() {
                    editor.handle_key(key);
                }
                FormAction::Continue
            }
        }
    }

    fn build_data(&self) -> Result<NewTaskData, String> {
        let title = self.title.content().trim().to_owned();
        if title.is_empty() {
            return Err("タイトルを入力してください".into());
        }

        let state = match self.state.content().trim() {
            "" => None,
            other => Some(other.to_owned()),
        };
        let labels = parse_list(self.labels.content());
        let assignees = parse_list(self.assignees.content());

        let description_raw = self.description.lines().join("\n");
        let description = if description_raw.trim().is_empty() {
            None
        } else {
            Some(description_raw.trim_end().to_owned())
        };

        Ok(NewTaskData {
            title,
            state,
            labels,
            assignees,
            description,
        })
    }

    fn current_line_editor_mut(&mut self) -> Option<&mut LineEditor> {
        match self.step {
            TaskFormStep::Title => Some(&mut self.title),
            TaskFormStep::State => Some(&mut self.state),
            TaskFormStep::Labels => Some(&mut self.labels),
            TaskFormStep::Assignees => Some(&mut self.assignees),
            TaskFormStep::Description => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum TaskFormStep {
    Title,
    State,
    Labels,
    Assignees,
    Description,
}

impl TaskFormStep {
    fn next(self) -> Self {
        match self {
            TaskFormStep::Title => TaskFormStep::State,
            TaskFormStep::State => TaskFormStep::Labels,
            TaskFormStep::Labels => TaskFormStep::Assignees,
            TaskFormStep::Assignees => TaskFormStep::Description,
            TaskFormStep::Description => TaskFormStep::Description,
        }
    }

    fn prev(self) -> Self {
        match self {
            TaskFormStep::Title => TaskFormStep::Title,
            TaskFormStep::State => TaskFormStep::Title,
            TaskFormStep::Labels => TaskFormStep::State,
            TaskFormStep::Assignees => TaskFormStep::Labels,
            TaskFormStep::Description => TaskFormStep::Assignees,
        }
    }
}

enum FormAction {
    Continue,
    Submit,
    Cancel,
}

#[derive(Default, Clone)]
struct LineEditor {
    buffer: String,
    cursor: usize,
}

impl LineEditor {
    fn content(&self) -> &str {
        &self.buffer
    }

    fn handle_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Left => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor < self.char_count() {
                    self.cursor += 1;
                }
            }
            KeyCode::Home => {
                self.cursor = 0;
            }
            KeyCode::End => {
                self.cursor = self.char_count();
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let start = self.byte_index(self.cursor - 1);
                    let end = self.byte_index(self.cursor);
                    self.buffer.drain(start..end);
                    self.cursor -= 1;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.char_count() {
                    let start = self.byte_index(self.cursor);
                    let end = self.byte_index(self.cursor + 1);
                    self.buffer.drain(start..end);
                }
            }
            KeyCode::Char(c) => {
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                {
                    let idx = self.byte_index(self.cursor);
                    self.buffer.insert(idx, c);
                    self.cursor += 1;
                }
            }
            _ => {}
        }
    }

    fn char_count(&self) -> usize {
        self.buffer.chars().count()
    }

    fn byte_index(&self, char_idx: usize) -> usize {
        if char_idx == 0 {
            return 0;
        }
        if char_idx >= self.char_count() {
            return self.buffer.len();
        }
        self.buffer
            .char_indices()
            .nth(char_idx)
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| self.buffer.len())
    }

    fn split_at_cursor(&self) -> (String, Option<char>, String) {
        let mut chars = self.buffer.chars();
        let before: String = chars.by_ref().take(self.cursor).collect();
        let cursor_char = chars.next();
        let after: String = chars.collect();
        (before, cursor_char, after)
    }
}

fn render_line_field(
    f: &mut ratatui::Frame<'_>,
    area: Rect,
    label: &str,
    editor: &LineEditor,
    active: bool,
    placeholder: &str,
) {
    let mut block = Block::default().title(label).borders(Borders::ALL);
    if active {
        block = block.border_style(Style::default().fg(Color::Cyan));
    }
    let (before, cursor_char, after) = editor.split_at_cursor();

    let mut spans = Vec::new();
    if active {
        if before.is_empty() && cursor_char.is_none() && after.is_empty() {
            spans.push(Span::styled(placeholder, Style::default().fg(Color::DarkGray)));
            spans.push(Span::styled(" ", Style::default().bg(Color::Cyan)));
        } else {
            spans.push(Span::raw(before));
            spans.push(Span::styled(
                cursor_char
                    .map(|c| c.to_string())
                    .unwrap_or_else(|| " ".to_string()),
                Style::default().bg(Color::Cyan),
            ));
            spans.push(Span::raw(after));
        }
    } else if before.is_empty() && cursor_char.is_none() && after.is_empty() {
        spans.push(Span::styled(placeholder, Style::default().fg(Color::DarkGray)));
    } else {
        let mut text = before;
        if let Some(c) = cursor_char {
            text.push(c);
        }
        text.push_str(&after);
        spans.push(Span::raw(text));
    }

    let paragraph = Paragraph::new(Line::from(spans))
        .block(block)
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_comment_overlay(f: &mut ratatui::Frame<'_>, size: Rect, input: &mut CommentInput) {
    let area = centered_rect(70, 60, size);
    f.render_widget(Clear, area);
    let block = Block::default()
        .title("コメント入力 (Ctrl+S / Ctrl+Enter:送信, Esc:キャンセル)")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    input.textarea.set_block(block);
    f.render_widget(input.textarea.widget(), area);
}

fn render_new_task_overlay(f: &mut ratatui::Frame<'_>, size: Rect, form: &mut NewTaskForm) {
    let area = centered_rect(80, 80, size);
    f.render_widget(Clear, area);

    let block = Block::default()
        .title("新規タスク (Tab/Shift+Tab:移動, Ctrl+S / Ctrl+Enter:作成, Esc:キャンセル)")
        .borders(Borders::ALL);
    let inner = block.inner(area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(5),
        ])
        .split(inner);

    render_line_field(
        f,
        chunks[0],
        "タイトル*",
        &form.title,
        form.step == TaskFormStep::Title,
        "タスク名を入力",
    );
    render_line_field(
        f,
        chunks[1],
        "状態",
        &form.state,
        form.step == TaskFormStep::State,
        "state/todo など",
    );
    render_line_field(
        f,
        chunks[2],
        "ラベル(カンマ区切り)",
        &form.labels,
        form.step == TaskFormStep::Labels,
        "type/docs, area/cli",
    );
    render_line_field(
        f,
        chunks[3],
        "担当者(カンマ区切り)",
        &form.assignees,
        form.step == TaskFormStep::Assignees,
        "alice, bob",
    );

    let mut desc_block = Block::default().title("説明 (Markdown 可)").borders(Borders::ALL);
    if form.step == TaskFormStep::Description {
        desc_block = desc_block.border_style(Style::default().fg(Color::Cyan));
    }
    form.description.set_block(desc_block);
    f.render_widget(form.description.widget(), chunks[4]);
}

fn parse_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_owned())
        .collect()
}

fn resolve_actor() -> Actor {
    let name = std::env::var("GIT_MILE_ACTOR_NAME")
        .or_else(|_| std::env::var("GIT_AUTHOR_NAME"))
        .unwrap_or_else(|_| "git-mile".to_owned());
    let email = std::env::var("GIT_MILE_ACTOR_EMAIL")
        .or_else(|_| std::env::var("GIT_AUTHOR_EMAIL"))
        .unwrap_or_else(|_| "git-mile@example.invalid".to_owned());
    Actor { name, email }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

impl TaskStore for GitStore {
    fn list_tasks(&self) -> Result<Vec<TaskId>> {
        GitStore::list_tasks(self)
    }

    fn load_events(&self, task: TaskId) -> Result<Vec<Event>> {
        GitStore::load_events(self, task)
    }

    fn append_event(&self, event: &Event) -> Result<()> {
        GitStore::append_event(self, event).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
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
            Ok(self
                .events
                .borrow()
                .get(&task)
                .cloned()
                .unwrap_or_else(|| Vec::new()))
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
        let mut ev = Event::new(task, actor(), kind);
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

        let view = TaskView::from_events(events);
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
        let titles: Vec<_> = app.tasks.iter().map(|view| view.snapshot.title.clone()).collect();
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
        app.add_comment(target, "hello".into(), actor())?;

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

        let id = app.create_task(data, actor())?;
        assert_eq!(app.tasks.len(), 1);
        assert_eq!(app.selected_task_id(), Some(id));

        let snap = &app.selected_task().unwrap().snapshot;
        assert_eq!(snap.title, "Title");
        assert_eq!(snap.state.as_deref(), Some("todo"));
        assert_eq!(snap.description, "Write documentation");
        let labels: Vec<&str> = snap.labels.iter().map(|s| s.as_str()).collect();
        assert_eq!(labels, vec!["type/docs"]);
        Ok(())
    }

    #[test]
    fn parse_list_trims_entries() {
        assert_eq!(
            parse_list("one, two , , three"),
            vec!["one", "two", "three"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
    }
}
