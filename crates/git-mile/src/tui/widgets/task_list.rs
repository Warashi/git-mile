use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState},
};

use crate::task_writer::TaskStore;

use super::super::view::Ui;

impl<S: TaskStore> Ui<S> {
    pub(in crate::tui) fn draw_task_list(&self, f: &mut Frame<'_>, area: Rect) {
        let visibility = self.app.visibility();
        let items = if visibility.has_visible_tasks() {
            let workflow = self.app.workflow();
            visibility
                .visible_tasks(&self.app.tasks)
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
            let message = if visibility.filter().is_empty() {
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
        if visibility.has_visible_tasks() {
            state.select(Some(visibility.selected_index()));
        }
        f.render_stateful_widget(list, area, &mut state);
    }
}
