use anyhow::Result;
use crossterm::event::{KeyEvent, KeyEventKind};
use git_mile_app::TaskStore;

use super::super::view::{CommentViewerState, DescriptionViewerState, DetailFocus, Ui, UiAction};
use crate::config::{Action, ViewType};

impl<S: TaskStore> Ui<S> {
    pub(in crate::tui) fn handle_key(&mut self, key: KeyEvent) -> Result<Option<UiAction>> {
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
            DetailFocus::DescriptionViewer => Ok(self.handle_description_viewer_key(key)),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn handle_task_list_key(&mut self, key: KeyEvent) -> Result<Option<UiAction>> {
        if self.keybindings.matches(ViewType::TaskList, Action::Quit, &key) {
            self.should_quit = true;
            return Ok(None);
        }

        if self.keybindings.matches(ViewType::TaskList, Action::Down, &key) {
            self.app.visibility_mut().select_next();
            return Ok(None);
        }

        if self.keybindings.matches(ViewType::TaskList, Action::Up, &key) {
            self.app.visibility_mut().select_prev();
            return Ok(None);
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::OpenTree, &key)
        {
            self.open_tree_view();
            return Ok(None);
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::JumpToParent, &key)
        {
            self.jump_to_parent();
            return Ok(None);
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::Refresh, &key)
        {
            self.app.refresh_tasks()?;
            self.info("タスクを再読込しました");
            return Ok(None);
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::AddComment, &key)
        {
            return self.selected_task_id().map_or_else(
                || {
                    self.error("コメント対象のタスクが選択されていません");
                    Ok(None)
                },
                |task| Ok(Some(UiAction::AddComment { task })),
            );
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::EditTask, &key)
        {
            return self.selected_task_id().map_or_else(
                || {
                    self.error("編集対象のタスクが選択されていません");
                    Ok(None)
                },
                |task| Ok(Some(UiAction::EditTask { task })),
            );
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::CreateTask, &key)
        {
            return Ok(Some(UiAction::CreateTask));
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::CreateSubtask, &key)
        {
            return self.selected_task_id().map_or_else(
                || {
                    self.error("子タスクを作成する親タスクが選択されていません");
                    Ok(None)
                },
                |parent| Ok(Some(UiAction::CreateSubtask { parent })),
            );
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::CopyTaskId, &key)
        {
            self.copy_selected_task_id();
            return Ok(None);
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::OpenStatePicker, &key)
        {
            self.open_state_picker();
            return Ok(None);
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::OpenCommentViewer, &key)
        {
            self.open_comment_viewer();
            return Ok(None);
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::OpenDescriptionViewer, &key)
        {
            self.open_description_viewer();
            return Ok(None);
        }

        if self
            .keybindings
            .matches(ViewType::TaskList, Action::EditFilter, &key)
        {
            return Ok(Some(UiAction::EditFilter));
        }

        Ok(None)
    }

    fn handle_tree_view_key(&mut self, key: KeyEvent) -> Option<UiAction> {
        if self.keybindings.matches(ViewType::TreeView, Action::Close, &key) {
            self.detail_focus = DetailFocus::None;
            return None;
        }

        if self.keybindings.matches(ViewType::TreeView, Action::Down, &key) {
            self.tree_view_down();
            return None;
        }

        if self.keybindings.matches(ViewType::TreeView, Action::Up, &key) {
            self.tree_view_up();
            return None;
        }

        if self
            .keybindings
            .matches(ViewType::TreeView, Action::Collapse, &key)
        {
            self.tree_view_collapse();
            return None;
        }

        if self.keybindings.matches(ViewType::TreeView, Action::Expand, &key) {
            self.tree_view_expand();
            return None;
        }

        if self.keybindings.matches(ViewType::TreeView, Action::Jump, &key) {
            self.tree_view_jump();
            return None;
        }

        None
    }

    fn handle_comment_viewer_key(&mut self, key: KeyEvent) -> Option<UiAction> {
        if self
            .keybindings
            .matches(ViewType::CommentViewer, Action::Close, &key)
        {
            self.close_comment_viewer();
            return None;
        }

        if self
            .keybindings
            .matches(ViewType::CommentViewer, Action::ScrollDown, &key)
        {
            self.comment_viewer_scroll_down(1);
            return None;
        }

        if self
            .keybindings
            .matches(ViewType::CommentViewer, Action::ScrollUp, &key)
        {
            self.comment_viewer_scroll_up(1);
            return None;
        }

        if self
            .keybindings
            .matches(ViewType::CommentViewer, Action::ScrollDownFast, &key)
        {
            self.comment_viewer_scroll_down(10);
            return None;
        }

        if self
            .keybindings
            .matches(ViewType::CommentViewer, Action::ScrollUpFast, &key)
        {
            self.comment_viewer_scroll_up(10);
            return None;
        }

        None
    }

    fn handle_description_viewer_key(&mut self, key: KeyEvent) -> Option<UiAction> {
        if self
            .keybindings
            .matches(ViewType::DescriptionViewer, Action::Close, &key)
        {
            self.close_description_viewer();
            return None;
        }

        if self
            .keybindings
            .matches(ViewType::DescriptionViewer, Action::ScrollDown, &key)
        {
            self.description_viewer_scroll_down(1);
            return None;
        }

        if self
            .keybindings
            .matches(ViewType::DescriptionViewer, Action::ScrollUp, &key)
        {
            self.description_viewer_scroll_up(1);
            return None;
        }

        if self
            .keybindings
            .matches(ViewType::DescriptionViewer, Action::ScrollDownFast, &key)
        {
            self.description_viewer_scroll_down(10);
            return None;
        }

        if self
            .keybindings
            .matches(ViewType::DescriptionViewer, Action::ScrollUpFast, &key)
        {
            self.description_viewer_scroll_up(10);
            return None;
        }

        None
    }

    fn jump_to_parent(&mut self) {
        if let Some(task) = self.selected_task() {
            let parent_id = self
                .app
                .get_parents(task.snapshot.id)
                .first()
                .map(|parent| parent.snapshot.id);
            if let Some(parent_id) = parent_id {
                self.app.visibility_mut().jump_to_task(parent_id);
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

    pub(in crate::tui) fn copy_selected_task_id(&mut self) {
        let Some(task) = self.selected_task() else {
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

    pub(in crate::tui) fn open_comment_viewer(&mut self) {
        let Some(task) = self.selected_task() else {
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

    pub(in crate::tui) fn open_description_viewer(&mut self) {
        let Some(task) = self.selected_task() else {
            self.error("説明を表示するタスクが選択されていません");
            return;
        };
        self.description_viewer = Some(DescriptionViewerState {
            task_id: task.snapshot.id,
            scroll_offset: 0,
        });
        self.detail_focus = DetailFocus::DescriptionViewer;
    }

    const fn close_description_viewer(&mut self) {
        self.description_viewer = None;
        self.detail_focus = DetailFocus::None;
    }

    const fn description_viewer_scroll_down(&mut self, lines: u16) {
        if let Some(viewer) = &mut self.description_viewer {
            viewer.scroll_offset = viewer.scroll_offset.saturating_add(lines);
        }
    }

    const fn description_viewer_scroll_up(&mut self, lines: u16) {
        if let Some(viewer) = &mut self.description_viewer {
            viewer.scroll_offset = viewer.scroll_offset.saturating_sub(lines);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;
    use anyhow::Error;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use git_mile_app::TaskRepository;
    use git_mile_app::TaskView;
    use git_mile_app::WorkflowConfig;
    use git_mile_core::TaskSnapshot;
    use git_mile_core::event::{Actor, Event};
    use git_mile_core::id::TaskId;
    use git2::Oid;
    use std::sync::Arc;

    #[derive(Clone)]
    struct EmptyStore;

    impl TaskStore for EmptyStore {
        type Error = Error;

        fn task_exists(&self, _task: TaskId) -> Result<bool, Self::Error> {
            Ok(false)
        }

        fn append_event(&self, _event: &Event) -> Result<Oid, Self::Error> {
            Oid::from_bytes(&[0; 20]).map_err(Into::into)
        }

        fn load_events(&self, _task: TaskId) -> Result<Vec<Event>, Self::Error> {
            Ok(Vec::new())
        }

        fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
            Ok(Vec::new())
        }

        fn list_tasks_modified_since(
            &self,
            _since: time::OffsetDateTime,
        ) -> Result<Vec<TaskId>, Self::Error> {
            Ok(Vec::new())
        }
    }

    #[allow(clippy::arc_with_non_send_sync)]
    fn test_ui() -> Ui<Arc<EmptyStore>> {
        let store = Arc::new(EmptyStore);
        let store_clone = Arc::clone(&store);
        let store_for_repo = Arc::new(store_clone);
        let repository = Arc::new(TaskRepository::new(store_for_repo));
        let app = App::new(
            store,
            repository,
            WorkflowConfig::default(),
            git_mile_app::HooksConfig::default(),
            std::path::PathBuf::new(),
        )
        .unwrap_or_else(|err| panic!("failed to init app: {err}"));
        Ui::new(
            app,
            Actor {
                name: "tester".into(),
                email: "tester@example.invalid".into(),
            },
            crate::config::keybindings::KeyBindingsConfig::default(),
        )
    }

    fn seed_task(ui: &mut Ui<Arc<EmptyStore>>) -> TaskId {
        let mut snapshot = TaskSnapshot::default();
        let id = TaskId::new();
        snapshot.id = id;
        snapshot.title = "task".into();
        let view = TaskView {
            snapshot,
            comments: Vec::new(),
            last_updated: None,
        };
        ui.app.tasks = vec![view];
        ui.app.rebuild_visibility(Some(id));
        id
    }

    #[test]
    fn quits_on_q_key() {
        let mut ui = test_ui();
        seed_task(&mut ui);
        let key = KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE);
        assert!(matches!(ui.handle_task_list_key(key), Ok(None)));
        assert!(ui.should_quit);
    }

    #[test]
    fn comment_viewer_escape_closes_popup() {
        let mut ui = test_ui();
        seed_task(&mut ui);
        ui.open_comment_viewer();
        assert_eq!(ui.detail_focus, DetailFocus::CommentViewer);
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        ui.handle_comment_viewer_key(key);
        assert!(ui.comment_viewer.is_none());
        assert_eq!(ui.detail_focus, DetailFocus::None);
    }

    #[test]
    fn description_viewer_opens_on_d_key() {
        let mut ui = test_ui();
        seed_task(&mut ui);
        let key = KeyEvent::new(KeyCode::Char('d'), KeyModifiers::NONE);
        assert!(matches!(ui.handle_task_list_key(key), Ok(None)));
        assert_eq!(ui.detail_focus, DetailFocus::DescriptionViewer);
        assert!(ui.description_viewer.is_some());
    }

    #[test]
    fn description_viewer_escape_closes_popup() {
        let mut ui = test_ui();
        seed_task(&mut ui);
        ui.open_description_viewer();
        assert_eq!(ui.detail_focus, DetailFocus::DescriptionViewer);
        let key = KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE);
        ui.handle_description_viewer_key(key);
        assert!(ui.description_viewer.is_none());
        assert_eq!(ui.detail_focus, DetailFocus::None);
    }
}
