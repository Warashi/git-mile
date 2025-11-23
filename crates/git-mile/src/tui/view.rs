use std::thread;
use std::time::{Duration, Instant};

use git_mile_core::TaskFilter;
use git_mile_core::event::Actor;
use git_mile_core::id::TaskId;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
};

use git_mile_app::StateKind;
use git_mile_app::TaskStore;
use git_mile_app::TaskView;

use super::app::App;
use super::clipboard::{ClipboardSink, default_clipboard};
use super::constants::UI_MESSAGE_TTL_SECS;
use super::tree_view::TreeViewState;
use crate::config::KeyBindingsConfig;

#[derive(Debug, Clone)]
pub(super) struct StatePickerOption {
    pub(super) value: Option<String>,
}

impl StatePickerOption {
    pub(super) const fn new(value: Option<String>) -> Self {
        Self { value }
    }

    pub(super) fn matches(&self, other: Option<&str>) -> bool {
        match (&self.value, other) {
            (None, None) => true,
            (Some(left), Some(right)) => left == right,
            _ => false,
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct CommentViewerState {
    pub(super) task_id: TaskId,
    pub(super) scroll_offset: u16,
}

#[derive(Debug, Clone)]
pub(super) struct DescriptionViewerState {
    pub(super) task_id: TaskId,
    pub(super) scroll_offset: u16,
}

pub(super) struct StatePickerState {
    pub(super) task_id: TaskId,
    pub(super) options: Vec<StatePickerOption>,
    pub(super) selected: usize,
}

/// Focus state for detail view components.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum DetailFocus {
    /// No focus (browsing task list).
    None,
    /// Focus on tree view (floating window).
    TreeView,
    /// Focus on state picker popup.
    StatePicker,
    /// Focus on comment viewer popup.
    CommentViewer,
    /// Focus on description viewer popup.
    DescriptionViewer,
}

pub(super) struct Ui<S: TaskStore> {
    pub(super) app: App<S>,
    pub(super) actor: Actor,
    pub(super) message: Option<Message>,
    pub(super) should_quit: bool,
    /// Current focus in detail view.
    pub(super) detail_focus: DetailFocus,
    /// Tree view state.
    pub(super) tree_state: TreeViewState,
    /// State picker popup state.
    pub(super) state_picker: Option<StatePickerState>,
    /// Comment viewer popup state.
    pub(super) comment_viewer: Option<CommentViewerState>,
    /// Description viewer popup state.
    pub(super) description_viewer: Option<DescriptionViewerState>,
    pub(super) clipboard: Box<dyn ClipboardSink>,
    /// Keybindings configuration.
    pub(super) keybindings: KeyBindingsConfig,
}

impl<S: TaskStore> Ui<S> {
    pub(super) const MAIN_MIN_HEIGHT: u16 = 5;
    pub(super) const INSTRUCTIONS_HEIGHT: u16 = 3;
    pub(super) const FILTER_HEIGHT: u16 = 3;
    pub(super) const STATUS_MESSAGE_MIN_HEIGHT: u16 = 3;
    pub(super) const STATUS_FOOTER_MIN_HEIGHT: u16 =
        Self::INSTRUCTIONS_HEIGHT + Self::FILTER_HEIGHT + Self::STATUS_MESSAGE_MIN_HEIGHT;

    pub(super) fn new(app: App<S>, actor: Actor, keybindings: KeyBindingsConfig) -> Self {
        let clipboard = default_clipboard();
        Self::with_clipboard(app, actor, keybindings, clipboard)
    }

    pub(super) fn with_clipboard(
        app: App<S>,
        actor: Actor,
        keybindings: KeyBindingsConfig,
        clipboard: Box<dyn ClipboardSink>,
    ) -> Self {
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
            description_viewer: None,
            clipboard,
            keybindings,
        };
        ui.apply_default_filter();
        ui
    }

    fn apply_default_filter(&mut self) {
        if self.app.visibility().filter().is_empty() {
            let mut filter = TaskFilter::default();
            filter.state_kinds.exclude.insert(StateKind::Done);
            self.update_filter(filter);
        }
    }

    pub(super) fn update_filter(&mut self, filter: TaskFilter) {
        if self.app.visibility().filter() == &filter {
            return;
        }
        let keep_id = self.app.visibility().selected_task_id(&self.app.tasks);
        {
            let visibility = self.app.visibility_mut();
            visibility.set_filter(filter);
        }
        self.app.rebuild_visibility(keep_id);
    }

    pub(super) fn selected_task(&self) -> Option<&TaskView> {
        self.app.visibility().selected_task(&self.app.tasks)
    }

    pub(super) fn selected_task_id(&self) -> Option<TaskId> {
        self.app.visibility().selected_task_id(&self.app.tasks)
    }

    pub(super) fn draw(&self, f: &mut Frame<'_>) {
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
            DetailFocus::DescriptionViewer => self.draw_description_viewer_popup(f),
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

    pub(super) fn info(&mut self, message: impl Into<String>) {
        self.message = Some(Message::info(message));
    }

    pub(super) fn error(&mut self, message: impl Into<String>) {
        self.message = Some(Message::error(message));
    }

    pub(super) fn tick(&mut self) {
        if let Some(msg) = &self.message
            && msg.is_expired(Duration::from_secs(UI_MESSAGE_TTL_SECS))
        {
            self.message = None;
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum UiAction {
    AddComment { task: TaskId },
    EditTask { task: TaskId },
    CreateTask,
    CreateSubtask { parent: TaskId },
    EditFilter,
}

pub(super) struct Message {
    pub(super) text: String,
    pub(super) level: MessageLevel,
    created_at: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MessageLevel {
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

    pub(super) fn style(&self) -> Style {
        match self.level {
            MessageLevel::Info => Style::default().fg(Color::Green),
            MessageLevel::Error => Style::default().fg(Color::Red),
        }
    }

    fn is_expired(&self, ttl: Duration) -> bool {
        self.created_at.elapsed() >= ttl
    }
}
