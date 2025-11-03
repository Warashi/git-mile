//! CLI entry point for git-mile.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use std::str::FromStr;
use tracing_subscriber::{fmt::format::FmtSpan, EnvFilter};

use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_core::TaskSnapshot;
use git_mile_store_git::GitStore;

mod tui;

/// Git-backed tasks without touching the working tree.
#[derive(Parser, Debug)]
#[command(
    name = "git-mile",
    version,
    about = "git-mile: tasks stored as git commits under refs/git-mile/tasks/*"
)]
struct Cli {
    /// Path to repo or any subdir (defaults to current).
    #[arg(long)]
    repo: Option<String>,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Create a new task with initial fields.
    New {
        #[arg(long)]
        title: String,
        #[arg(long)]
        state: Option<String>,
        #[arg(short = 'l', long = "label")]
        labels: Vec<String>,
        #[arg(short = 'a', long = "assignee")]
        assignees: Vec<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long, default_value = "git-mile")]
        actor_name: String,
        #[arg(long, default_value = "git-mile@example.invalid")]
        actor_email: String,
    },

    /// Add a comment to an existing task.
    Comment {
        #[arg(long)]
        task: String,
        #[arg(long)]
        message: String,
        #[arg(long, default_value = "git-mile")]
        actor_name: String,
        #[arg(long, default_value = "git-mile@example.invalid")]
        actor_email: String,
    },

    /// Show a materialized snapshot of a task.
    Show {
        #[arg(long)]
        task: String,
    },

    /// List task ids.
    Ls,

    /// Launch interactive terminal UI.
    Tui,
}

fn main() -> Result<()> {
    install_tracing();
    let cli = Cli::parse();

    let repo_path = cli.repo.unwrap_or_else(|| ".".to_owned());
    execute_command(&repo_path, cli.cmd)
}

fn execute_command(repo_path: &str, command: Command) -> Result<()> {
    match command {
        Command::New {
            title,
            state,
            labels,
            assignees,
            description,
            actor_name,
            actor_email,
        } => {
            let store = GitStore::open(repo_path)?;
            let task = TaskId::new();
            let actor = Actor {
                name: actor_name,
                email: actor_email,
            };
            let ev = Event::new(
                task,
                actor,
                EventKind::TaskCreated {
                    title,
                    labels,
                    assignees,
                    description,
                    state,
                },
            );
            let oid = store.append_event(&ev)?;
            println!("created task: {task} ({oid})");
        }

        Command::Comment {
            task,
            message,
            actor_name,
            actor_email,
        } => {
            let store = GitStore::open(repo_path)?;
            let task = TaskId::from_str(&task).context("Invalid task id")?;
            let actor = Actor {
                name: actor_name,
                email: actor_email,
            };
            let ev = Event::new(
                task,
                actor,
                EventKind::CommentAdded {
                    comment_id: EventId::new(),
                    body_md: message,
                },
            );
            let oid = store.append_event(&ev)?;
            println!("commented: {task} ({oid})");
        }

        Command::Show { task } => {
            let store = GitStore::open(repo_path)?;
            let task = TaskId::from_str(&task).context("Invalid task id")?;
            let events = store.load_events(task)?;
            let snap = TaskSnapshot::replay(events);
            println!("{}", serde_json::to_string_pretty(&snap)?);
        }

        Command::Ls => {
            let store = GitStore::open(repo_path)?;
            for id in store.list_tasks()? {
                println!("{id}");
            }
        }

        Command::Tui => {
            let store = GitStore::open(repo_path)?;
            tui::run(store)?;
        }
    }

    Ok(())
}

fn install_tracing() {
    // EnvFilterに RUST_LOG を渡せる。デフォルトは INFO。
    let filter = EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into());
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .with_span_events(FmtSpan::NONE)
        .compact()
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_new_command() {
        let cli = Cli::parse_from([
            "git-mile",
            "--repo",
            ".",
            "new",
            "--title",
            "Improve docs",
            "--state",
            "state/todo",
            "--label",
            "type/docs",
            "--assignee",
            "alice",
        ]);

        match cli.cmd {
            Command::New {
                title,
                state,
                labels,
                assignees,
                ..
            } => {
                assert_eq!(title, "Improve docs");
                assert_eq!(state.as_deref(), Some("state/todo"));
                assert_eq!(labels, vec!["type/docs"]);
                assert_eq!(assignees, vec!["alice"]);
            }
            _ => panic!("expected new command"),
        }
    }

    #[test]
    fn parse_comment_command() {
        let cli = Cli::parse_from([
            "git-mile",
            "comment",
            "--task",
            "01J9Q2S4C8M7X0ABCDEF123456",
            "--message",
            "Looks good",
        ]);

        match cli.cmd {
            Command::Comment { task, message, .. } => {
                assert_eq!(task, "01J9Q2S4C8M7X0ABCDEF123456");
                assert_eq!(message, "Looks good");
            }
            _ => panic!("expected comment command"),
        }
    }

    #[test]
    fn parse_tui_command() {
        let cli = Cli::parse_from(["git-mile", "tui"]);
        match cli.cmd {
            Command::Tui => {}
            _ => panic!("expected tui command"),
        }
    }
}
