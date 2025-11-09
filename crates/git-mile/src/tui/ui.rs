use std::borrow::Cow;
use std::collections::BTreeSet;
use std::io::Stdout;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use git_mile_core::TaskFilter;
use git_mile_core::event::Actor;
use git_mile_core::id::TaskId;
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};
use unicode_segmentation::UnicodeSegmentation;

use crate::config::{StateKind, WorkflowState};

use super::app::{App, TaskStore, TaskView};
use super::clipboard::{ClipboardSink, default_clipboard};
use super::editor::{
    comment_editor_template, edit_task_editor_template, filter_editor_template, new_task_editor_template,
    parse_comment_editor_output, parse_filter_editor_output, parse_new_task_editor_output,
    summarize_task_filter,
};
use super::terminal::{launch_editor, with_terminal_suspended};

/// Tree node for hierarchical task display.
#[derive(Debug, Clone)]
pub(super) struct TreeNode {
    /// Task ID.
    task_id: TaskId,
    /// Child nodes.
    children: Vec<TreeNode>,
    /// Whether this node is expanded.
    expanded: bool,
}

/// State for tree view navigation.
#[derive(Debug, Clone)]
pub(super) struct TreeViewState {
    /// Root nodes of the tree.
    pub(super) roots: Vec<TreeNode>,
    /// Flattened list of visible nodes (for navigation).
    pub(super) visible_nodes: Vec<(usize, TaskId)>, // (depth, task_id)
    /// Currently selected index in `visible_nodes`.
    pub(super) selected: usize,
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
    pub(super) fn selected_task_id(&self) -> Option<TaskId> {
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
pub(super) struct StatePickerOption {
    pub(super) value: Option<String>,
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
}

pub(super) fn truncate_with_ellipsis(input: &str, max_graphemes: usize) -> Cow<'_, str> {
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

pub(super) struct Ui<S: TaskStore> {
    pub(super) app: App<S>,
    actor: Actor,
    pub(super) message: Option<Message>,
    pub(super) should_quit: bool,
    /// Current focus in detail view.
    pub(super) detail_focus: DetailFocus,
    /// Tree view state.
    pub(super) tree_state: TreeViewState,
    /// State picker popup state.
    pub(super) state_picker: Option<StatePickerState>,
    /// Comment viewer popup state.
    comment_viewer: Option<CommentViewerState>,
    clipboard: Box<dyn ClipboardSink>,
}

impl<S: TaskStore> Ui<S> {
    const MAIN_MIN_HEIGHT: u16 = 5;
    const INSTRUCTIONS_HEIGHT: u16 = 3;
    const FILTER_HEIGHT: u16 = 3;
    const STATUS_MESSAGE_MIN_HEIGHT: u16 = 3;
    pub(super) const STATUS_FOOTER_MIN_HEIGHT: u16 =
        Self::INSTRUCTIONS_HEIGHT + Self::FILTER_HEIGHT + Self::STATUS_MESSAGE_MIN_HEIGHT;

    pub(super) fn new(app: App<S>, actor: Actor) -> Self {
        let clipboard = default_clipboard();
        Self::with_clipboard(app, actor, clipboard)
    }

    pub(super) fn with_clipboard(app: App<S>, actor: Actor, clipboard: Box<dyn ClipboardSink>) -> Self {
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
                let child_rows = u16::try_from(children.len()).unwrap_or(u16::MAX);
                let height = child_rows.min(10) + 2;
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

    pub(super) const fn status_layout_constraints() -> [Constraint; 3] {
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

        let scroll_top = self.tree_scroll_offset(area.height);
        let paragraph = Paragraph::new(lines).scroll((scroll_top, 0));
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
        if let Some(task) = self
            .app
            .tasks
            .iter()
            .find(|view| view.snapshot.id == viewer.task_id)
        {
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
    fn tree_scroll_offset(&self, viewport_height: u16) -> u16 {
        if viewport_height == 0 {
            return 0;
        }
        let total_rows = self.tree_state.visible_nodes.len();
        let visible_capacity = usize::from(viewport_height);
        if total_rows <= visible_capacity {
            return 0;
        }

        let selected = self.tree_state.selected;
        let half_page = usize::from(viewport_height) / 2;
        let mut desired = selected.saturating_sub(half_page);
        let max_offset = total_rows.saturating_sub(visible_capacity);
        if desired > max_offset {
            desired = max_offset;
        }

        let capped = desired.min(u16::MAX as usize);
        u16::try_from(capped).unwrap_or(u16::MAX)
    }

    pub(super) fn handle_key(&mut self, key: KeyEvent) -> Result<Option<UiAction>> {
        if key.kind != KeyEventKind::Press {
            return Ok(None);
        }

        self.handle_browse_key(key)
    }

    fn handle_browse_key(&mut self, key: KeyEvent) -> Result<Option<UiAction>> {
        match self.detail_focus {
            DetailFocus::None => self.handle_task_list_key(key),
            DetailFocus::TreeView => Ok(self.handle_tree_view_key(key)),
            DetailFocus::StatePicker => Ok(self.handle_state_picker_key(key)),
            DetailFocus::CommentViewer => Ok(self.handle_comment_viewer_key(key)),
        }
    }

    fn handle_task_list_key(&mut self, key: KeyEvent) -> Result<Option<UiAction>> {
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
        }
    }

    fn handle_tree_view_key(&mut self, key: KeyEvent) -> Option<UiAction> {
        match key.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                self.detail_focus = DetailFocus::None;
                None
            }
            KeyCode::Down | KeyCode::Char('j' | 'J') => {
                self.tree_view_down();
                None
            }
            KeyCode::Up | KeyCode::Char('k' | 'K') => {
                self.tree_view_up();
                None
            }
            KeyCode::Char('h' | 'H') => {
                self.tree_view_collapse();
                None
            }
            KeyCode::Char('l' | 'L') => {
                self.tree_view_expand();
                None
            }
            KeyCode::Enter => {
                self.tree_view_jump();
                None
            }
            _ => None,
        }
    }

    fn handle_state_picker_key(&mut self, key: KeyEvent) -> Option<UiAction> {
        match key.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                self.close_state_picker();
                None
            }
            KeyCode::Down | KeyCode::Char('j' | 'J') => {
                self.state_picker_down();
                None
            }
            KeyCode::Up | KeyCode::Char('k' | 'K') => {
                self.state_picker_up();
                None
            }
            KeyCode::Enter => {
                self.apply_state_picker_selection();
                None
            }
            _ => None,
        }
    }

    const fn handle_comment_viewer_key(&mut self, key: KeyEvent) -> Option<UiAction> {
        match key.code {
            KeyCode::Char('q' | 'Q') | KeyCode::Esc => {
                self.close_comment_viewer();
                None
            }
            KeyCode::Char('j' | 'J') => {
                self.comment_viewer_scroll_down(1);
                None
            }
            KeyCode::Char('k' | 'K') => {
                self.comment_viewer_scroll_up(1);
                None
            }
            KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Half-page down (approximate with 10 lines)
                self.comment_viewer_scroll_down(10);
                None
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                // Half-page up (approximate with 10 lines)
                self.comment_viewer_scroll_up(10);
                None
            }
            _ => None,
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

    pub(super) fn copy_selected_task_id(&mut self) {
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

    pub(super) fn open_state_picker(&mut self) {
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

    pub(super) fn state_picker_down(&mut self) {
        if let Some(picker) = &mut self.state_picker {
            if picker.options.is_empty() {
                return;
            }
            let max_index = picker.options.len() - 1;
            picker.selected = (picker.selected + 1).min(max_index);
        }
    }

    pub(super) const fn state_picker_up(&mut self) {
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

    const fn close_comment_viewer(&mut self) {
        self.comment_viewer = None;
        self.detail_focus = DetailFocus::None;
    }

    const fn comment_viewer_scroll_down(&mut self, lines: u16) {
        if let Some(viewer) = &mut self.comment_viewer {
            viewer.scroll_offset = viewer.scroll_offset.saturating_add(lines);
        }
    }

    const fn comment_viewer_scroll_up(&mut self, lines: u16) {
        if let Some(viewer) = &mut self.comment_viewer {
            viewer.scroll_offset = viewer.scroll_offset.saturating_sub(lines);
        }
    }

    pub(super) fn apply_state_picker_selection(&mut self) {
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

    pub(super) fn open_tree_view(&mut self) {
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

    pub(super) const fn tree_view_down(&mut self) {
        if self.tree_state.selected + 1 < self.tree_state.visible_nodes.len() {
            self.tree_state.selected += 1;
        }
    }

    pub(super) const fn tree_view_up(&mut self) {
        if self.tree_state.selected > 0 {
            self.tree_state.selected -= 1;
        }
    }

    pub(super) fn tree_view_collapse(&mut self) {
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

    pub(super) fn tree_view_expand(&mut self) {
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

    pub(super) fn tree_view_jump(&mut self) {
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

    pub(super) fn info(&mut self, message: impl Into<String>) {
        self.message = Some(Message::info(message));
    }

    pub(super) fn error(&mut self, message: impl Into<String>) {
        self.message = Some(Message::error(message));
    }

    pub(super) fn instructions(&self) -> String {
        match self.detail_focus {
            DetailFocus::None => {
                let base = "j/k:移動 ↵:ツリー n:新規 s:子タスク e:編集 c:コメント v:コメント表示 r:再読込 p:親へ y:IDコピー t:状態 f:フィルタ q:終了";
                format!("{} [{} <{}>]", base, self.actor.name, self.actor.email)
            }
            DetailFocus::TreeView => "j/k:移動 h:閉じる l:開く ↵:ジャンプ q/Esc:閉じる".to_string(),
            DetailFocus::StatePicker => "j/k:移動 ↵:決定 q/Esc:キャンセル".to_string(),
            DetailFocus::CommentViewer => {
                "j/k:スクロール Ctrl-d/Ctrl-u:半画面スクロール q/Esc:閉じる".to_string()
            }
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

    pub(super) fn tick(&mut self) {
        if let Some(msg) = &self.message
            && msg.is_expired(Duration::from_secs(5))
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

pub(super) fn handle_ui_action<S: TaskStore>(
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
