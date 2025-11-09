use std::env;
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use crossterm::{
    event::{self, Event as CrosstermEvent},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use git_mile_core::event::Actor;
use git_mile_store_git::GitStore;
use git2::Config;
use ratatui::{Terminal, backend::CrosstermBackend};
use tracing::subscriber::NoSubscriber;

use crate::config::WorkflowConfig;

mod app;
mod clipboard;
mod editor;
mod task_cache;
mod task_visibility;
mod terminal;
mod ui;

use self::app::App;
use self::ui::{Ui, handle_ui_action};

/// Launch the interactive TUI.
pub fn run(store: GitStore, workflow: WorkflowConfig) -> Result<()> {
    enable_raw_mode().context("failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen).context("failed to enter alternate screen")?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    terminal.hide_cursor()?;

    let result = tracing::subscriber::with_default(NoSubscriber::default(), || {
        run_event_loop(&mut terminal, store, workflow)
    });

    disable_raw_mode().ok();
    execute!(terminal.backend_mut(), LeaveAlternateScreen).ok();
    terminal.show_cursor().ok();

    result
}

fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    store: GitStore,
    workflow: WorkflowConfig,
) -> Result<()> {
    let actor = resolve_actor();
    let app = App::new(store, workflow)?;
    let mut ui = Ui::new(app, actor);

    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(200);

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

fn resolve_actor() -> Actor {
    let name = env::var("GIT_MILE_ACTOR_NAME")
        .or_else(|_| env::var("GIT_AUTHOR_NAME"))
        .or_else(|_| {
            Config::open_default()
                .and_then(|config| config.get_string("user.name"))
                .map_err(|_| env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| "git-mile".to_owned());
    let email = env::var("GIT_MILE_ACTOR_EMAIL")
        .or_else(|_| env::var("GIT_AUTHOR_EMAIL"))
        .or_else(|_| {
            Config::open_default()
                .and_then(|config| config.get_string("user.email"))
                .map_err(|_| env::VarError::NotPresent)
        })
        .unwrap_or_else(|_| "git-mile@example.invalid".to_owned());
    Actor { name, email }
}

#[cfg(test)]
mod tests;
