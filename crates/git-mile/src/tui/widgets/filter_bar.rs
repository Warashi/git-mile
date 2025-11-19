use std::borrow::Cow;

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::Style,
    widgets::{Block, Borders, Paragraph, Wrap},
};

use git_mile_app::TaskStore;

use super::super::editor::summarize_task_filter;
use super::super::view::{DetailFocus, Message, Ui};

impl<S: TaskStore> Ui<S> {
    pub(in crate::tui) fn draw_status(&self, f: &mut Frame<'_>, area: Rect) {
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

    pub(in crate::tui) const fn status_layout_constraints() -> [Constraint; 3] {
        [
            Constraint::Length(Self::INSTRUCTIONS_HEIGHT),
            Constraint::Length(Self::FILTER_HEIGHT),
            Constraint::Min(Self::STATUS_MESSAGE_MIN_HEIGHT),
        ]
    }

    pub(in crate::tui) fn instructions(&self) -> String {
        match self.detail_focus {
            DetailFocus::None => {
                let base = "j/k:移動 ↵:ツリー n:新規 s:子タスク e:編集 c:コメント v:コメント表示 d:説明表示 r:再読込 p:親へ y:IDコピー t:状態 f:フィルタ q:終了";
                format!("{} [{} <{}>]", base, self.actor.name, self.actor.email)
            }
            DetailFocus::TreeView => "j/k:移動 h:閉じる l:開く ↵:ジャンプ q/Esc:閉じる".to_string(),
            DetailFocus::StatePicker => "j/k:移動 ↵:決定 q/Esc:キャンセル".to_string(),
            DetailFocus::CommentViewer | DetailFocus::DescriptionViewer => {
                "j/k:スクロール Ctrl-d/Ctrl-u:半画面スクロール q/Esc:閉じる".to_string()
            }
        }
    }

    fn filter_summary_text(&self) -> String {
        summarize_task_filter(self.app.visibility().filter())
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
}
