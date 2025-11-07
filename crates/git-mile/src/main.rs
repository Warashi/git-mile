//! CLI entry point for git-mile.

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt::format::FmtSpan};

use commands::TaskService;
use config::ProjectConfig;
use git_mile_store_git::GitStore;
use rmcp::ServiceExt;

mod commands;
mod config;
mod mcp;
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
        #[arg(short = 'p', long = "parent")]
        parents: Vec<String>,
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

    /// Start MCP server.
    Mcp,
}

fn main() -> Result<()> {
    let Cli { repo, cmd } = Cli::parse();

    if should_install_tracing(&cmd) {
        install_tracing();
    }

    let repo_path = repo.unwrap_or_else(|| ".".to_owned());
    execute_command(&repo_path, cmd)
}

fn execute_command(repo_path: &str, command: Command) -> Result<()> {
    let workflow = ProjectConfig::load(repo_path)?.workflow;
    match (command, workflow) {
        (Command::Tui, workflow) => {
            let store = GitStore::open(repo_path)?;
            tui::run(store, workflow)
        }

        (Command::Mcp, workflow) => {
            let store = GitStore::open(repo_path)?;
            let server = mcp::GitMileServer::new(store, workflow);
            tokio::runtime::Runtime::new()?
                .block_on(async move {
                    let transport = (tokio::io::stdin(), tokio::io::stdout());
                    let server = server
                        .serve(transport)
                        .await
                        .map_err(|e| anyhow::anyhow!("{e:?}"))?;
                    server.waiting().await.map_err(|e| anyhow::anyhow!("{e:?}"))
                })
                .map(|_| ())
        }

        (other, workflow) => {
            let store = GitStore::open(repo_path)?;
            let service = TaskService::new(store, workflow);
            commands::run(other, &service)
        }
    }
}

const fn should_install_tracing(cmd: &Command) -> bool {
    !matches!(cmd, Command::Mcp)
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

    #[test]
    fn skips_tracing_in_mcp_mode() {
        assert!(!should_install_tracing(&Command::Mcp));
    }

    #[test]
    fn installs_tracing_for_other_commands() {
        assert!(should_install_tracing(&Command::Tui));
    }
}
