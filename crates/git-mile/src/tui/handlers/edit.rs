use std::io::Stdout;

use anyhow::{Context, Result};
use git_mile_core::id::TaskId;
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use git_mile_app::TaskStore;

use super::super::editor::{
    comment_editor_template, edit_task_editor_template, filter_editor_template, new_task_editor_template,
    parse_comment_editor_output, parse_new_task_editor_output,
};
use super::super::terminal::{launch_editor, with_terminal_suspended};
use super::super::view::{Ui, UiAction};

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
            let template = filter_editor_template(ui.app.visibility().filter());
            let raw = with_terminal_suspended(terminal, || launch_editor(&template))?;
            ui.apply_filter_editor_output(&raw);
        }
    }
    Ok(())
}

impl<S: TaskStore> Ui<S> {
    pub(super) fn apply_comment_input(&mut self, task: TaskId, raw: &str) -> Result<()> {
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

    pub(super) fn apply_new_task_input(&mut self, raw: &str) -> Result<()> {
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

    pub(super) fn apply_new_subtask_input(&mut self, parent: TaskId, raw: &str) -> Result<()> {
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

    pub(super) fn apply_edit_task_input(&mut self, task: TaskId, raw: &str) -> Result<()> {
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
}
