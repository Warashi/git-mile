use crate::task_writer::TaskStore;

use super::super::editor::{parse_filter_editor_output, summarize_task_filter};
use super::super::view::Ui;

impl<S: TaskStore> Ui<S> {
    pub(in crate::tui) fn apply_filter_editor_output(&mut self, raw: &str) {
        match parse_filter_editor_output(raw) {
            Ok(filter) => {
                if &filter == self.app.visibility().filter() {
                    self.info("フィルタに変更はありません");
                } else {
                    self.update_filter(filter.clone());
                    let summary = summarize_task_filter(&filter);
                    if self.app.visibility().has_visible_tasks() {
                        self.info(format!("フィルタを更新しました: {summary}"));
                    } else {
                        self.info(format!("フィルタを更新しました（該当なし）: {summary}"));
                    }
                }
            }
            Err(err) => self.error(format!("フィルタの解析に失敗しました: {err}")),
        }
    }
}
