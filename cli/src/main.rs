use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
use git_mile_core::{
    ChangeStatusInput, ChangeStatusOutcome, CreateMileInput, EntityId, EntitySnapshot, EntityStore,
    EntitySummary, MergeOutcome, MergeStrategy, MileEventKind, MileId, MileSnapshot, MileStatus,
    MileStore, MileSummary, OperationId, ReplicaId, app_version,
};
use git2::{Config, ErrorCode, Repository};
use serde_json::to_writer_pretty;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:?}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let Cli {
        repo,
        replica,
        author,
        email,
        command,
    } = Cli::parse();

    match command {
        Commands::Init => command_init(&repo)?,
        Commands::Create(args) => {
            let identity = resolve_identity(&repo, author.as_deref(), email.as_deref())?;
            let replica_id = resolve_replica(replica.as_deref());
            let default_message = format!("create mile {}", args.title);
            let snapshot = command_mile_create(
                &repo,
                &replica_id,
                &identity.signature,
                &args.title,
                args.description.as_deref(),
                args.message.clone().or_else(|| Some(default_message)),
                if args.draft {
                    MileStatus::Draft
                } else {
                    MileStatus::Open
                },
            )?;
            println!("{}", snapshot.id);
        }
        Commands::List(args) => {
            let mut miles = command_mile_list(&repo)?;
            if !args.all {
                miles.retain(|mile| mile.status != MileStatus::Closed);
            }

            match args.format {
                OutputFormat::Table => {
                    if miles.is_empty() {
                        println!("No miles found");
                    } else {
                        print_mile_table(&miles);
                    }
                }
                OutputFormat::Raw => print_mile_raw(&miles),
                OutputFormat::Json => {
                    let stdout = io::stdout();
                    let mut handle = stdout.lock();
                    to_writer_pretty(&mut handle, &miles)?;
                    handle.write_all(b"\n")?;
                }
            }
        }
        Commands::Show(args) => {
            let mile_id = parse_entity_id(&args.mile_id)?;
            let snapshot = command_mile_show(&repo, &mile_id)?;

            if args.json {
                let stdout = io::stdout();
                let mut handle = stdout.lock();
                to_writer_pretty(&mut handle, &snapshot)?;
                handle.write_all(b"\n")?;
            } else {
                print_mile_details(&snapshot);
            }
        }
        Commands::Open(args) => {
            let identity = resolve_identity(&repo, author.as_deref(), email.as_deref())?;
            let replica_id = resolve_replica(replica.as_deref());
            let mile_id = parse_entity_id(&args.mile_id)?;
            let outcome = command_mile_change_status(
                &repo,
                &replica_id,
                &identity.signature,
                &mile_id,
                MileStatus::Open,
                args.message
                    .clone()
                    .or_else(|| Some("open mile".to_string())),
            )?;
            if outcome.changed {
                println!("Mile {} opened", mile_id);
            } else {
                eprintln!("warning: mile {} already open", mile_id);
            }
        }
        Commands::Close(args) => {
            let identity = resolve_identity(&repo, author.as_deref(), email.as_deref())?;
            let replica_id = resolve_replica(replica.as_deref());
            let mile_id = parse_entity_id(&args.mile_id)?;
            let outcome = command_mile_change_status(
                &repo,
                &replica_id,
                &identity.signature,
                &mile_id,
                MileStatus::Closed,
                args.message
                    .clone()
                    .or_else(|| Some("close mile".to_string())),
            )?;
            if outcome.changed {
                println!("Mile {} closed", mile_id);
            } else {
                eprintln!("warning: mile {} already closed", mile_id);
            }
        }
        Commands::EntityDebug(entity_args) => handle_entity_debug(&repo, entity_args)?,
    }

    Ok(())
}

#[derive(Parser)]
#[command(name = git_mile_core::APP_NAME, version = app_version())]
struct Cli {
    #[arg(long, global = true, value_name = "PATH", default_value = ".")]
    repo: PathBuf,
    #[arg(long, global = true, value_name = "ID")]
    replica: Option<String>,
    #[arg(long, global = true, value_name = "NAME")]
    author: Option<String>,
    #[arg(long, global = true, value_name = "EMAIL")]
    email: Option<String>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Init,
    Create(CreateArgs),
    List(ListArgs),
    Show(ShowArgs),
    Open(StatusArgs),
    Close(StatusArgs),
    EntityDebug(EntityArgs),
}

#[derive(Parser)]
struct CreateArgs {
    title: String,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    message: Option<String>,
    #[arg(long)]
    draft: bool,
}

#[derive(Parser)]
struct ListArgs {
    #[arg(long)]
    all: bool,
    #[arg(long, value_enum, default_value = "table")]
    format: OutputFormat,
}

#[derive(Parser)]
struct ShowArgs {
    mile_id: String,
    #[arg(long)]
    json: bool,
}

#[derive(Parser)]
struct StatusArgs {
    mile_id: String,
    #[arg(long)]
    message: Option<String>,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum OutputFormat {
    Table,
    Json,
    Raw,
}

#[derive(Parser)]
struct EntityArgs {
    #[command(subcommand)]
    command: EntityCommand,
}

#[derive(Subcommand)]
enum EntityCommand {
    List,
    Show(EntityShowArgs),
    Resolve(EntityResolveArgs),
}

#[derive(Parser)]
struct EntityShowArgs {
    entity: String,
}

#[derive(Parser)]
struct EntityResolveArgs {
    entity: String,
    #[arg(long, value_enum, default_value = "ours")]
    strategy: ResolveStrategy,
    #[arg(long = "head")]
    heads: Vec<String>,
}

#[derive(Copy, Clone, Debug, ValueEnum, Eq, PartialEq)]
enum ResolveStrategy {
    Ours,
    Theirs,
    Manual,
}

struct Identity {
    signature: String,
}

fn command_init(repo: &Path) -> Result<()> {
    match Repository::open(repo) {
        Ok(_) => {
            println!("Repository already initialized at {}", repo.display());
            Ok(())
        }
        Err(err) if err.code() == ErrorCode::NotFound => {
            if !repo.exists() {
                fs::create_dir_all(repo)
                    .with_context(|| format!("failed to create directory {}", repo.display()))?;
            }
            Repository::init(repo).with_context(|| {
                format!("failed to initialize repository at {}", repo.display())
            })?;
            println!("Initialized repository at {}", repo.display());
            Ok(())
        }
        Err(err) => {
            Err(err).with_context(|| format!("failed to open repository at {}", repo.display()))
        }
    }
}

fn command_mile_create(
    repo: &Path,
    replica_id: &ReplicaId,
    author: &str,
    title: &str,
    description: Option<&str>,
    message: Option<String>,
    initial_status: MileStatus,
) -> Result<MileSnapshot> {
    let store = MileStore::open(repo)?;
    Ok(store.create_mile(CreateMileInput {
        replica_id: replica_id.clone(),
        author: author.to_string(),
        message,
        title: title.to_string(),
        description: description.map(|value| value.to_string()),
        initial_status,
    })?)
}

fn command_mile_list(repo: &Path) -> Result<Vec<MileSummary>> {
    let store = MileStore::open(repo)?;
    Ok(store.list_miles()?)
}

fn command_mile_show(repo: &Path, mile_id: &MileId) -> Result<MileSnapshot> {
    let store = MileStore::open(repo)?;
    Ok(store.load_mile(mile_id)?)
}

fn command_mile_change_status(
    repo: &Path,
    replica_id: &ReplicaId,
    author: &str,
    mile_id: &MileId,
    status: MileStatus,
    message: Option<String>,
) -> Result<ChangeStatusOutcome> {
    let store = MileStore::open(repo)?;
    Ok(store.change_status(ChangeStatusInput {
        mile_id: mile_id.clone(),
        replica_id: replica_id.clone(),
        author: author.to_string(),
        message,
        status,
    })?)
}

fn handle_entity_debug(repo: &Path, args: EntityArgs) -> Result<()> {
    match args.command {
        EntityCommand::List => {
            let summaries = command_entity_list(repo)?;
            if summaries.is_empty() {
                println!("No entities found");
            } else {
                for summary in summaries {
                    println!("{} (heads: {})", summary.entity_id, summary.head_count);
                }
            }
        }
        EntityCommand::Show(show) => {
            let entity_id = parse_entity_id(&show.entity)?;
            let snapshot = command_entity_show(repo, &entity_id)?;
            println!("Entity: {}", entity_id);
            println!("Clock: {}", snapshot.clock_snapshot);
            println!("Heads:");
            for head in &snapshot.heads {
                println!("  - {}", head);
            }
            println!("Operations ({} total):", snapshot.operations.len());
            for operation in &snapshot.operations {
                println!(
                    "  - {} (parents: {})",
                    operation.id,
                    operation.parents.len()
                );
            }
        }
        EntityCommand::Resolve(resolve) => {
            let entity_id = parse_entity_id(&resolve.entity)?;
            let strategy = to_merge_strategy(resolve.strategy, &resolve.heads)?;
            let outcome = command_entity_resolve(repo, &entity_id, strategy)?;
            println!("Updated heads:");
            for head in outcome.heads {
                println!("  - {}", head);
            }
        }
    }
    Ok(())
}

fn command_entity_list(repo: &Path) -> Result<Vec<EntitySummary>> {
    let store = open_store(repo)?;
    store.list_entities().map_err(Into::into)
}

fn command_entity_show(repo: &Path, entity_id: &EntityId) -> Result<EntitySnapshot> {
    let store = open_store(repo)?;
    store.load_entity(entity_id).map_err(Into::into)
}

fn command_entity_resolve(
    repo: &Path,
    entity_id: &EntityId,
    strategy: MergeStrategy,
) -> Result<MergeOutcome> {
    let store = open_store(repo)?;
    store
        .resolve_conflicts(entity_id, strategy)
        .map_err(Into::into)
}

fn open_store(path: &Path) -> Result<EntityStore> {
    EntityStore::open(path)
        .with_context(|| format!("failed to open repository at {}", path.display()))
}

fn parse_entity_id(value: &str) -> Result<EntityId> {
    EntityId::from_str(value).map_err(|err| anyhow!("invalid entity id '{value}': {err}"))
}

fn to_merge_strategy(strategy: ResolveStrategy, heads: &[String]) -> Result<MergeStrategy> {
    match strategy {
        ResolveStrategy::Ours => Ok(MergeStrategy::Ours),
        ResolveStrategy::Theirs => Ok(MergeStrategy::Theirs),
        ResolveStrategy::Manual => {
            if heads.is_empty() {
                return Err(anyhow!(
                    "manual merge strategy requires at least one --head value"
                ));
            }

            let parsed = heads
                .iter()
                .map(|head| {
                    OperationId::from_str(head)
                        .map_err(|err| anyhow!("invalid operation id '{head}': {err}"))
                })
                .collect::<Result<Vec<_>>>()?;

            Ok(MergeStrategy::Manual(parsed))
        }
    }
}

fn resolve_replica(value: Option<&str>) -> ReplicaId {
    if let Some(replica) = value {
        return ReplicaId::new(replica.to_string());
    }

    let host = env::var("HOSTNAME")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            env::var("COMPUTERNAME")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .or_else(|| {
            env::var("HOST")
                .ok()
                .filter(|value| !value.trim().is_empty())
        });

    match host {
        Some(value) => ReplicaId::new(value),
        None => {
            eprintln!("warning: using fallback replica id 'git-mile'");
            ReplicaId::new("git-mile")
        }
    }
}

fn resolve_identity(
    repo: &Path,
    name_override: Option<&str>,
    email_override: Option<&str>,
) -> Result<Identity> {
    let mut name = name_override.map(|value| value.to_string());
    let mut email = email_override.map(|value| value.to_string());

    if name.is_none() || email.is_none() {
        if let Ok(repo) = Repository::discover(repo) {
            if let Ok(config) = repo.config() {
                if name.is_none() {
                    name = read_config(&config, "user.name");
                }
                if email.is_none() {
                    email = read_config(&config, "user.email");
                }
            }
        }
    }

    if name.is_none() || email.is_none() {
        if let Ok(config) = Config::open_default() {
            if name.is_none() {
                name = read_config(&config, "user.name");
            }
            if email.is_none() {
                email = read_config(&config, "user.email");
            }
        }
    }

    let name = name.unwrap_or_else(|| "git-mile".to_string());
    let email = email.unwrap_or_else(|| "git-mile@example.com".to_string());
    let signature = if email.is_empty() {
        name
    } else {
        format!("{name} <{email}>")
    };

    Ok(Identity { signature })
}

fn read_config(config: &Config, key: &str) -> Option<String> {
    config
        .get_string(key)
        .ok()
        .filter(|value| !value.is_empty())
}

fn print_mile_table(miles: &[MileSummary]) {
    println!("{:<40} {:<10} {}", "ID", "STATUS", "TITLE");
    for mile in miles {
        println!("{:<40} {:<10} {}", mile.id, mile.status, mile.title);
    }
}

fn print_mile_raw(miles: &[MileSummary]) {
    for mile in miles {
        println!("{}\t{}\t{}", mile.id, mile.status, mile.title);
    }
}

fn print_mile_details(snapshot: &MileSnapshot) {
    println!("Mile: {}", snapshot.id);
    println!("Title: {}", snapshot.title);
    println!("Status: {}", snapshot.status);
    println!("Created: {}", snapshot.created_at);
    println!("Updated: {}", snapshot.updated_at);
    match &snapshot.description {
        Some(description) if !description.trim().is_empty() => {
            println!("Description:\n{description}");
        }
        Some(_) => println!("Description: (empty)"),
        None => println!("Description: (none)"),
    }
    println!("Events:");
    for event in &snapshot.events {
        print_event(event);
    }
}

fn print_event(event: &git_mile_core::MileEvent) {
    let summary = match &event.payload {
        MileEventKind::Created(data) => {
            format!("{} created mile \"{}\"", event.timestamp, data.title)
        }
        MileEventKind::StatusChanged(data) => {
            format!("{} status -> {}", event.timestamp, data.status)
        }
        MileEventKind::Unknown { event_type, .. } => {
            let kind = event_type.as_deref().unwrap_or("unknown");
            format!("{} unknown event {kind}", event.timestamp)
        }
    };

    println!("  - {summary}");
    println!("    author: {}", event.metadata.author);
    if let Some(message) = &event.metadata.message {
        println!("    message: {}", message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::{
        EntityId, LamportClock, Operation, OperationBlob, OperationMetadata, OperationPack,
        ReplicaId,
    };
    use git2::Repository;
    use tempfile::TempDir;

    fn setup_entity_repo() -> Result<(TempDir, EntityId, OperationId, OperationId, OperationId)> {
        let temp = tempfile::tempdir()?;
        Repository::init_bare(temp.path())?;
        let store = open_store(temp.path())?;

        let entity_id = EntityId::new();
        let mut clock = LamportClock::new(ReplicaId::new("cli-tests"));

        let root_blob = OperationBlob::from_bytes(b"root".to_vec());
        let root_op = Operation::new(
            OperationId::new(clock.tick()?),
            vec![],
            root_blob.digest().clone(),
            OperationMetadata::new("tester", Some("root".to_string())),
        );
        let root_pack = OperationPack::new(
            entity_id.clone(),
            clock.snapshot(),
            vec![root_op.clone()],
            vec![root_blob.clone()],
        )?;
        store.persist_pack(root_pack)?;

        let branch_a_blob = OperationBlob::from_bytes(b"a".to_vec());
        let branch_b_blob = OperationBlob::from_bytes(b"b".to_vec());
        let op_a = Operation::new(
            OperationId::new(clock.tick()?),
            vec![root_op.id.clone()],
            branch_a_blob.digest().clone(),
            OperationMetadata::new("tester", Some("a".to_string())),
        );
        let op_b = Operation::new(
            OperationId::new(clock.tick()?),
            vec![root_op.id.clone()],
            branch_b_blob.digest().clone(),
            OperationMetadata::new("tester", Some("b".to_string())),
        );
        let pack = OperationPack::new(
            entity_id.clone(),
            clock.snapshot(),
            vec![op_a.clone(), op_b.clone()],
            vec![branch_a_blob, branch_b_blob],
        )?;
        store.persist_pack(pack)?;

        Ok((temp, entity_id, root_op.id, op_a.id, op_b.id))
    }

    #[test]
    fn entity_list_returns_summary() -> Result<()> {
        let (temp, _, _, _, _) = setup_entity_repo()?;
        let summaries = command_entity_list(temp.path())?;
        assert_eq!(summaries.len(), 1);
        Ok(())
    }

    #[test]
    fn entity_show_loads_snapshot() -> Result<()> {
        let (temp, entity_id, _, _, _) = setup_entity_repo()?;
        let snapshot = command_entity_show(temp.path(), &entity_id)?;
        assert_eq!(snapshot.entity_id, entity_id);
        assert_eq!(snapshot.operations.len(), 3);
        Ok(())
    }

    #[test]
    fn entity_manual_resolve_reduces_heads() -> Result<()> {
        let (temp, entity_id, _, op_a, op_b) = setup_entity_repo()?;
        let outcome = command_entity_resolve(
            temp.path(),
            &entity_id,
            MergeStrategy::Manual(vec![op_a.clone()]),
        )?;
        assert_eq!(outcome.heads, vec![op_a.clone()]);

        let snapshot = command_entity_show(temp.path(), &entity_id)?;
        assert_eq!(snapshot.heads, vec![op_a]);
        assert!(!snapshot.heads.contains(&op_b));
        Ok(())
    }

    #[test]
    fn mile_create_and_list_flow() -> Result<()> {
        let temp = tempfile::tempdir()?;
        Repository::init_bare(temp.path())?;
        let replica = ReplicaId::new("cli-tests");

        let snapshot = command_mile_create(
            temp.path(),
            &replica,
            "tester <tester@example.com>",
            "Initial Mile",
            Some("details"),
            None,
            MileStatus::Open,
        )?;

        let miles = command_mile_list(temp.path())?;
        assert_eq!(miles.len(), 1);
        assert_eq!(miles[0].id, snapshot.id);
        assert_eq!(miles[0].status, MileStatus::Open);
        Ok(())
    }

    #[test]
    fn mile_change_status_is_idempotent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        Repository::init_bare(temp.path())?;
        let replica = ReplicaId::new("cli-tests");

        let snapshot = command_mile_create(
            temp.path(),
            &replica,
            "tester <tester@example.com>",
            "Initial Mile",
            None,
            None,
            MileStatus::Open,
        )?;

        let closed = command_mile_change_status(
            temp.path(),
            &replica,
            "tester <tester@example.com>",
            &snapshot.id,
            MileStatus::Closed,
            None,
        )?;
        assert!(closed.changed);

        let repeat = command_mile_change_status(
            temp.path(),
            &replica,
            "tester <tester@example.com>",
            &snapshot.id,
            MileStatus::Closed,
            None,
        )?;
        assert!(!repeat.changed);
        Ok(())
    }
}
