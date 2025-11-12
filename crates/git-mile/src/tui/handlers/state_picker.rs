use std::collections::BTreeSet;

use crossterm::event::{KeyCode, KeyEvent};

use git_mile_app::TaskStore;

use super::super::view::{DetailFocus, StatePickerOption, StatePickerState, Ui, UiAction};

impl<S: TaskStore> Ui<S> {
    pub(in crate::tui) fn handle_state_picker_key(&mut self, key: KeyEvent) -> Option<UiAction> {
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

    pub(in crate::tui) fn open_state_picker(&mut self) {
        let Some(task) = self.selected_task() else {
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

    pub(in crate::tui) fn state_picker_down(&mut self) {
        if let Some(picker) = &mut self.state_picker {
            if picker.options.is_empty() {
                return;
            }
            let max_index = picker.options.len() - 1;
            picker.selected = (picker.selected + 1).min(max_index);
        }
    }

    pub(in crate::tui) const fn state_picker_up(&mut self) {
        if let Some(picker) = &mut self.state_picker {
            if picker.options.is_empty() {
                return;
            }
            picker.selected = picker.selected.saturating_sub(1);
        }
    }

    pub(in crate::tui) fn apply_state_picker_selection(&mut self) {
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

    fn close_state_picker(&mut self) {
        self.state_picker = None;
        self.detail_focus = DetailFocus::None;
    }
}
