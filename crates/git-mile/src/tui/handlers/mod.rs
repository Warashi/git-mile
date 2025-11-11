use std::io::Stdout;

use anyhow::Result;
use ratatui::{Terminal, backend::CrosstermBackend};

use crate::task_writer::TaskStore;

use super::view::{Ui, UiAction};

pub(super) mod edit;
pub(super) mod filter;
pub(super) mod navigation;
pub(super) mod state_picker;

pub(super) fn handle_ui_action<S: TaskStore>(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ui: &mut Ui<S>,
    action: UiAction,
) -> Result<()> {
    edit::handle_ui_action(terminal, ui, action)
}
