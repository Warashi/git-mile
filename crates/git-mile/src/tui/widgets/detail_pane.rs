use std::borrow::Cow;

use git_mile_core::id::TaskId;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Wrap},
};

use git_mile_app::TaskStore;
use git_mile_app::TaskView;

use super::super::constants::{
    DETAIL_BREADCRUMB_HEIGHT, DETAIL_BREADCRUMB_TITLE_MAX_CHARS, DETAIL_CHILD_ENTRY_MARKER,
    DETAIL_CHILD_LIST_MAX_ROWS, DETAIL_CHILD_LIST_PADDING_ROWS, DETAIL_PARENT_TITLE_MAX_CHARS,
    DETAIL_SECTION_MIN_HEIGHT,
};
use super::super::view::Ui;
use super::util::{state_kind_marker, truncate_with_ellipsis};

impl<S: TaskStore> Ui<S> {
    pub(in crate::tui) fn draw_task_details(&self, f: &mut Frame<'_>, area: Rect) {
        if let Some(task) = self.selected_task() {
            let has_parents = !task.snapshot.parents.is_empty();
            let children = self.app.get_children(task.snapshot.id);
            let has_children = !children.is_empty();

            let mut constraints = Vec::new();
            if has_parents {
                constraints.push(Constraint::Length(DETAIL_BREADCRUMB_HEIGHT));
            }
            constraints.push(Constraint::Min(DETAIL_SECTION_MIN_HEIGHT));
            if has_children {
                let child_rows = u16::try_from(children.len()).unwrap_or(u16::MAX);
                let height = child_rows.min(DETAIL_CHILD_LIST_MAX_ROWS) + DETAIL_CHILD_LIST_PADDING_ROWS;
                constraints.push(Constraint::Length(height));
            }

            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints(constraints)
                .split(area);

            let mut chunk_idx = 0;

            if has_parents {
                self.draw_breadcrumb(f, chunks[chunk_idx], task.snapshot.id);
                chunk_idx += 1;
            }

            self.draw_main_task_details(f, chunks[chunk_idx], task);
            chunk_idx += 1;

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

    fn draw_breadcrumb(&self, f: &mut Frame<'_>, area: Rect, task_id: TaskId) {
        let ancestors = self.app.get_ancestor_chain(task_id);
        let mut breadcrumb_items: Vec<Span<'_>> = Vec::new();

        breadcrumb_items.push(Span::raw("Home"));

        for ancestor in &ancestors {
            breadcrumb_items.push(Span::raw(" > "));
            let ancestor_title = truncate_with_ellipsis(
                ancestor.snapshot.title.as_str(),
                DETAIL_BREADCRUMB_TITLE_MAX_CHARS,
            );
            breadcrumb_items.push(Span::raw(ancestor_title));
        }

        breadcrumb_items.push(Span::raw(" > "));
        breadcrumb_items.push(Span::styled("現在", Style::default().fg(Color::Cyan)));

        let line = Line::from(breadcrumb_items);
        let paragraph = Paragraph::new(line)
            .block(Block::default().borders(Borders::ALL))
            .wrap(Wrap { trim: false });
        f.render_widget(paragraph, area);
    }

    fn draw_main_task_details(&self, f: &mut Frame<'_>, area: Rect, task: &TaskView) {
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

        if !task.snapshot.parents.is_empty() {
            let parents = self.app.get_parents(task.snapshot.id);
            let parent_info = if parents.is_empty() {
                format!("親: {} 件（未読込）", task.snapshot.parents.len())
            } else {
                let parent_titles: Vec<_> = parents
                    .iter()
                    .map(|p| truncate_with_ellipsis(p.snapshot.title.as_str(), DETAIL_PARENT_TITLE_MAX_CHARS))
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

    fn draw_subtasks(&self, f: &mut Frame<'_>, area: Rect, _task_id: TaskId, children: &[&TaskView]) {
        let workflow = self.app.workflow();
        let items: Vec<ListItem<'_>> = children
            .iter()
            .map(|child| {
                let state_value = child.snapshot.state.as_deref();
                let state_marker = state_kind_marker(child.snapshot.state_kind);
                let state_label = workflow.display_label(state_value);

                let text = format!(
                    "{DETAIL_CHILD_ENTRY_MARKER} {} [{}]{}",
                    child.snapshot.title, state_label, state_marker
                );

                ListItem::new(text)
            })
            .collect();

        let title = format!("子タスク ({})", children.len());
        let list = List::new(items).block(Block::default().title(title).borders(Borders::ALL));

        f.render_widget(list, area);
    }

    pub(in crate::tui) fn draw_comments(&self, f: &mut Frame<'_>, area: Rect) {
        let block = Block::default().title("コメント").borders(Borders::ALL);
        let inner = block.inner(area);
        f.render_widget(block, area);

        if let Some(task) = self.selected_task() {
            if task.comments.is_empty() {
                let paragraph =
                    Paragraph::new("コメントはまだありません。").style(Style::default().fg(Color::DarkGray));
                f.render_widget(paragraph, inner);
            } else {
                let mut lines = Vec::new();
                for comment in &task.comments {
                    let header = format!(
                        "{} <{}> [{}]",
                        comment.actor.name, comment.actor.email, comment.created_at
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
}
