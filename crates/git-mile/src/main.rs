//! CLI entry point for git-mile.

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::{EnvFilter, fmt::format::FmtSpan};

use commands::TaskService;
use config::ProjectConfig;
use git_mile_store_git::GitStore;
use rmcp::ServiceExt;

mod commands;
mod config;
mod mcp;
/// Helpers for computing task diffs shared by CLI/TUI/MCP.
pub mod task_patch;
pub mod task_writer;
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

    /// List tasks with optional filters.
    Ls {
        /// Match specific workflow states.
        #[arg(long = "state", short = 's')]
        states: Vec<String>,
        /// Require tasks to include these labels (logical AND).
        #[arg(long = "label", short = 'l')]
        labels: Vec<String>,
        /// Match tasks assigned to any of these actors.
        #[arg(long = "assignee", short = 'a')]
        assignees: Vec<String>,
        /// Include only these workflow state kinds.
        #[arg(long = "state-kind")]
        state_kinds: Vec<String>,
        /// Exclude these workflow state kinds.
        #[arg(long = "exclude-state-kind")]
        exclude_state_kinds: Vec<String>,
        /// Require tasks to include any of these parents.
        #[arg(long = "parent")]
        parents: Vec<String>,
        /// Require tasks to include any of these children.
        #[arg(long = "child")]
        children: Vec<String>,
        /// Match tasks updated at or after this timestamp (RFC3339).
        #[arg(long = "updated-since")]
        updated_since: Option<String>,
        /// Match tasks updated at or before this timestamp (RFC3339).
        #[arg(long = "updated-until")]
        updated_until: Option<String>,
        /// Case-insensitive substring matched against title/description/state/labels/assignees.
        #[arg(long = "text")]
        text: Option<String>,
        /// Output format.
        #[arg(long = "format", value_enum, default_value_t = LsFormat::Table)]
        format: LsFormat,
    },

    /// Launch interactive terminal UI.
    Tui,

    /// Start MCP server.
    Mcp,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
#[value(rename_all = "snake_case")]
pub(crate) enum LsFormat {
    /// Render a human-readable table.
    Table,
    /// Emit JSON array of task snapshots.
    Json,
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
    fn parse_ls_command_with_defaults() {
        let cli = Cli::parse_from(["git-mile", "ls"]);
        match cli.cmd {
            Command::Ls {
                states,
                labels,
                assignees,
                state_kinds,
                exclude_state_kinds,
                parents,
                children,
                updated_since,
                updated_until,
                text,
                format,
            } => {
                assert!(states.is_empty());
                assert!(labels.is_empty());
                assert!(assignees.is_empty());
                assert!(state_kinds.is_empty());
                assert!(exclude_state_kinds.is_empty());
                assert!(parents.is_empty());
                assert!(children.is_empty());
                assert!(updated_since.is_none());
                assert!(updated_until.is_none());
                assert!(text.is_none());
                assert_eq!(format, LsFormat::Table);
            }
            _ => panic!("expected ls command"),
        }
    }

    #[test]
    fn parse_ls_command_with_filters() {
        let cli = Cli::parse_from([
            "git-mile",
            "ls",
            "--state",
            "state/todo",
            "--label",
            "type/docs",
            "--label",
            "priority/high",
            "--assignee",
            "alice",
            "--text",
            "fix bug",
            "--format",
            "json",
        ]);
        match cli.cmd {
            Command::Ls {
                states,
                labels,
                assignees,
                state_kinds,
                exclude_state_kinds,
                parents,
                children,
                updated_since,
                updated_until,
                text,
                format,
            } => {
                assert_eq!(states, vec!["state/todo"]);
                assert_eq!(labels, vec!["type/docs", "priority/high"]);
                assert_eq!(assignees, vec!["alice"]);
                assert!(state_kinds.is_empty());
                assert!(exclude_state_kinds.is_empty());
                assert!(parents.is_empty());
                assert!(children.is_empty());
                assert!(updated_since.is_none());
                assert!(updated_until.is_none());
                assert_eq!(text.as_deref(), Some("fix bug"));
                assert_eq!(format, LsFormat::Json);
            }
            _ => panic!("expected ls command"),
        }
    }

    #[test]
    fn parse_ls_command_with_extended_filters() {
        let cli = Cli::parse_from([
            "git-mile",
            "ls",
            "--state-kind",
            "todo",
            "--exclude-state-kind",
            "done",
            "--parent",
            "00000000-0000-0000-0000-000000000001",
            "--child",
            "00000000-0000-0000-0000-000000000002",
            "--updated-since",
            "2024-01-01T00:00:00Z",
            "--updated-until",
            "2024-12-31T23:59:59Z",
        ]);
        match cli.cmd {
            Command::Ls {
                state_kinds,
                exclude_state_kinds,
                parents,
                children,
                updated_since,
                updated_until,
                ..
            } => {
                assert_eq!(state_kinds, vec!["todo"]);
                assert_eq!(exclude_state_kinds, vec!["done"]);
                assert_eq!(parents, vec!["00000000-0000-0000-0000-000000000001".to_string()]);
                assert_eq!(children, vec!["00000000-0000-0000-0000-000000000002".to_string()]);
                assert_eq!(updated_since.as_deref(), Some("2024-01-01T00:00:00Z"));
                assert_eq!(updated_until.as_deref(), Some("2024-12-31T23:59:59Z"));
            }
            _ => panic!("expected ls command"),
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
