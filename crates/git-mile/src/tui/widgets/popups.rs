use git_mile_core::id::TaskId;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use git_mile_app::TaskStore;
use git_mile_app::WorkflowState;

use super::super::tree_view::TreeNode;
use super::super::view::Ui;
use super::util::state_kind_marker;

impl<S: TaskStore> Ui<S> {
    pub(in crate::tui) fn draw_tree_view_popup(&self, f: &mut Frame<'_>) {
        let area = f.area();
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

        let block = Block::default()
            .title("タスクツリー")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(Color::Black));

        f.render_widget(Clear, popup_area);
        let inner = block.inner(popup_area);
        f.render_widget(block, popup_area);

        self.draw_tree_content(f, inner);
    }

    fn draw_tree_content(&self, f: &mut Frame<'_>, area: Rect) {
        let mut lines = Vec::new();
        let workflow = self.app.workflow();

        for (i, (depth, task_id)) in self.tree_state.visible_nodes.iter().enumerate() {
            let is_selected = i == self.tree_state.selected;
            let Some(task) = self.app.tasks.iter().find(|t| t.snapshot.id == *task_id) else {
                continue;
            };

            let indent = "  ".repeat(*depth);
            let children = self.app.get_children(*task_id);
            let has_children = !children.is_empty();
            let tree_char = if has_children {
                self.find_node_in_state(*task_id)
                    .map_or("▶", |node| if node.expanded { "▼" } else { "▶" })
            } else {
                "■"
            };

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

    pub(in crate::tui) fn draw_state_picker_popup(&self, f: &mut Frame<'_>) {
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

    pub(in crate::tui) fn draw_comment_viewer_popup(&self, f: &mut Frame<'_>) {
        let Some(viewer) = &self.comment_viewer else {
            return;
        };
        let area = f.area();

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

    pub(in crate::tui) fn draw_description_viewer_popup(&self, f: &mut Frame<'_>) {
        let Some(viewer) = &self.description_viewer else {
            return;
        };
        let area = f.area();

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
            .title(format!("説明: {task_title}"))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Cyan));
        f.render_widget(Clear, popup_area);
        f.render_widget(block.clone(), popup_area);
        let inner = block.inner(popup_area);

        if let Some(task) = self
            .app
            .tasks
            .iter()
            .find(|view| view.snapshot.id == viewer.task_id)
        {
            if task.snapshot.description.is_empty() {
                let paragraph =
                    Paragraph::new("説明はまだありません。").style(Style::default().fg(Color::DarkGray));
                f.render_widget(paragraph, inner);
            } else {
                let mut lines = Vec::new();
                for line in task.snapshot.description.lines() {
                    lines.push(Line::from(line.to_owned()));
                }
                let paragraph = Paragraph::new(lines)
                    .wrap(Wrap { trim: false })
                    .scroll((viewer.scroll_offset, 0));
                f.render_widget(paragraph, inner);
            }
        }
    }

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
}
