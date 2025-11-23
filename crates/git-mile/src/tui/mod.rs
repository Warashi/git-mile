use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event as CrosstermEvent},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use git_mile_store_git::GitStore;
use ratatui::{Terminal, backend::CrosstermBackend};
use tracing::subscriber::NoSubscriber;

use git_mile_app::TaskRepository;
use git_mile_app::{WorkflowConfig, default_actor};
use crate::config::keybindings::{KeyBindingsConfig, load_config, validate_tui_config};

mod app;
mod clipboard;
pub mod constants;
mod editor;
mod handlers;
mod task_visibility;
mod terminal;
mod tree_view;
mod view;
mod widgets;

use self::app::App;
use self::constants::TUI_TICK_RATE_MS;
use self::handlers::handle_ui_action;
use self::view::Ui;

/// Launch the interactive TUI.
pub fn run(
    store: GitStore,
    workflow: WorkflowConfig,
    hooks_config: git_mile_app::HooksConfig,
    base_dir: std::path::PathBuf,
) -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let result = tracing::subscriber::with_default(NoSubscriber::default(), || {
        run_event_loop(&mut terminal, store, workflow, hooks_config, base_dir)
    });

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result
}

#[allow(clippy::arc_with_non_send_sync)]
fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    store: GitStore,
    workflow: WorkflowConfig,
    hooks_config: git_mile_app::HooksConfig,
    base_dir: std::path::PathBuf,
) -> Result<()> {
    let store_arc = Arc::new(store);
    let store_arc_clone = Arc::clone(&store_arc);
    let store_arc_for_repo = Arc::new(store_arc_clone);
    let repository = TaskRepository::new(store_arc_for_repo);
    let repo_root = base_dir
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let actor = default_actor(&repo_root);
    let app = App::new(store_arc, Arc::new(repository), workflow, hooks_config, base_dir)?;

    // Load TUI configuration
    let keybindings = match load_config(None)? {
        Some(config) => {
            // Validate the loaded configuration
            validate_tui_config(&config)?;
            config.keybindings
        }
        None => KeyBindingsConfig::default(),
    };

    let mut ui = Ui::new(app, actor, keybindings);

    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(TUI_TICK_RATE_MS);

    loop {
        terminal.draw(|f| ui.draw(f))?;
        if ui.should_quit {
            break;
        }

        let timeout = tick_rate.checked_sub(last_tick.elapsed()).unwrap_or_default();

        if event::poll(timeout)? {
            let evt = event::read()?;
            if let CrosstermEvent::Key(key) = evt
                && let Some(action) = ui.handle_key(key)?
                && let Err(err) = handle_ui_action(terminal, &mut ui, action)
            {
                ui.error(format!("エディタ処理中に失敗しました: {err}"));
            }
        }

        if last_tick.elapsed() >= tick_rate {
            ui.tick();
            last_tick = Instant::now();
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
