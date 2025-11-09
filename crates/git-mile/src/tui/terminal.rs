use std::env;
use std::fs;
use std::io::{Stdout, Write};
use std::path::PathBuf;
use std::process::Command;

use anyhow::{Context, Result, anyhow};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::{Terminal, backend::CrosstermBackend};
use tempfile::NamedTempFile;

pub(super) fn with_terminal_suspended<F, T>(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    f: F,
) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    suspend_terminal(terminal)?;
    let result = f();
    resume_terminal(terminal)?;
    result
}

pub(super) fn suspend_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    terminal.show_cursor()?;
    terminal.flush()?;
    disable_raw_mode().context("failed to disable raw mode")?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen).context("failed to leave alternate screen")?;
    Ok(())
}

pub(super) fn resume_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    execute!(terminal.backend_mut(), EnterAlternateScreen).context("failed to re-enter alternate screen")?;
    enable_raw_mode().context("failed to enable raw mode")?;
    terminal.clear()?;
    terminal.hide_cursor()?;
    terminal.flush()?;
    Ok(())
}

pub(super) fn resolve_editor_command() -> String {
    env::var("GIT_MILE_EDITOR")
        .or_else(|_| env::var("VISUAL"))
        .or_else(|_| env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".into())
}

pub(super) fn launch_editor(initial: &str) -> Result<String> {
    let mut tempfile = NamedTempFile::new().context("一時ファイルの作成に失敗しました")?;
    tempfile
        .write_all(initial.as_bytes())
        .context("一時ファイルへの書き込みに失敗しました")?;
    tempfile
        .flush()
        .context("一時ファイルのフラッシュに失敗しました")?;

    let temp_path: PathBuf = tempfile.path().to_path_buf();

    let editor = resolve_editor_command();
    let mut parts =
        shell_words::split(&editor).map_err(|err| anyhow!("エディタコマンドを解析できません: {err}"))?;
    if parts.is_empty() {
        parts.push(editor);
    }
    let program = parts.remove(0);

    let status = Command::new(&program)
        .args(&parts)
        .arg(&temp_path)
        .status()
        .with_context(|| format!("エディタ {program} の起動に失敗しました"))?;
    if !status.success() {
        return Err(anyhow!("エディタが異常終了しました (終了コード: {status})"));
    }

    let contents =
        fs::read_to_string(&temp_path).context("エディタで編集した内容の読み込みに失敗しました")?;
    Ok(contents)
}
