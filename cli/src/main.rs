use std::env;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
use git_mile_core::{
    AddProtectionInput, AdoptIdentityInput, ChangeStatusInput, ChangeStatusOutcome,
    CreateIdentityInput, CreateMileInput, EntityId, EntitySnapshot, EntityStore, EntitySummary,
    IdentityProtection, IdentityStore, IdentitySummary, MergeOutcome, MergeStrategy, MileEventKind,
    MileId, MileSnapshot, MileStatus, MileStore, MileSummary, OperationId, ProtectionKind,
    ReplicaId, app_version,
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
        Commands::Create { command } => handle_create_command(
            &repo,
            replica.as_deref(),
            author.as_deref(),
            email.as_deref(),
            command,
        )?,
        Commands::List { command } => handle_list_command(
            &repo,
            replica.as_deref(),
            author.as_deref(),
            email.as_deref(),
            command,
        )?,
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
            let replica_id = resolve_replica(replica.as_deref());
            let identity =
                resolve_identity(&repo, &replica_id, author.as_deref(), email.as_deref())?;
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
            let replica_id = resolve_replica(replica.as_deref());
            let identity =
                resolve_identity(&repo, &replica_id, author.as_deref(), email.as_deref())?;
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
        Commands::Adopt { command } => handle_adopt_command(
            &repo,
            replica.as_deref(),
            author.as_deref(),
            email.as_deref(),
            command,
        )?,
        Commands::Protect { command } => handle_protect_command(
            &repo,
            replica.as_deref(),
            author.as_deref(),
            email.as_deref(),
            command,
        )?,
        Commands::Mile { command } => handle_mile_command(
            &repo,
            replica.as_deref(),
            author.as_deref(),
            email.as_deref(),
            command,
        )?,
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
    Create {
        #[command(subcommand)]
        command: CreateCommand,
    },
    List {
        #[command(subcommand)]
        command: ListCommand,
    },
    Show(ShowArgs),
    Open(StatusArgs),
    Close(StatusArgs),
    Adopt {
        #[command(subcommand)]
        command: AdoptCommand,
    },
    Protect {
        #[command(subcommand)]
        command: ProtectCommand,
    },
    Mile {
        #[command(subcommand)]
        command: MileCommand,
    },
    EntityDebug(EntityArgs),
}

#[derive(Parser)]
struct MileCreateArgs {
    title: String,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    message: Option<String>,
    #[arg(long)]
    draft: bool,
}

#[derive(Parser)]
struct MileListArgs {
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

#[derive(Subcommand)]
enum CreateCommand {
    Mile(MileCreateArgs),
    Identity(IdentityCreateArgs),
}

#[derive(Parser)]
struct IdentityCreateArgs {
    #[arg(long = "display-name")]
    display_name: String,
    #[arg(long)]
    email: String,
    #[arg(long)]
    login: Option<String>,
    #[arg(long)]
    signature: Option<String>,
    #[arg(long)]
    message: Option<String>,
    #[arg(long)]
    adopt: bool,
    #[arg(long = "protect-pgp", value_name = "FINGERPRINT")]
    protect_pgp: Vec<String>,
    #[arg(long = "protect-pgp-armored", value_name = "PATH")]
    protect_pgp_armored: Vec<PathBuf>,
}

#[derive(Parser)]
struct IdentityListArgs {
    #[arg(long, value_enum, default_value = "table")]
    format: OutputFormat,
}

#[derive(Parser)]
struct IdentityAdoptArgs {
    identity_id: String,
    #[arg(long)]
    signature: Option<String>,
    #[arg(long)]
    message: Option<String>,
}

#[derive(Parser)]
struct IdentityProtectArgs {
    identity_id: String,
    #[arg(long = "pgp-fingerprint")]
    pgp_fingerprint: String,
    #[arg(long = "armored-key")]
    armored_key: Option<PathBuf>,
    #[arg(long)]
    message: Option<String>,
}

#[derive(Subcommand)]
enum MileCommand {
    Create(MileCreateArgs),
    List(MileListArgs),
}

#[derive(Subcommand)]
enum ListCommand {
    Mile(MileListArgs),
    Identity(IdentityListArgs),
}

#[derive(Subcommand)]
enum AdoptCommand {
    Identity(IdentityAdoptArgs),
}

#[derive(Subcommand)]
enum ProtectCommand {
    Identity(IdentityProtectArgs),
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

fn run_mile_create(
    repo: &Path,
    replica: Option<&str>,
    author: Option<&str>,
    email: Option<&str>,
    args: MileCreateArgs,
) -> Result<()> {
    let MileCreateArgs {
        title,
        description,
        message,
        draft,
    } = args;

    let replica_id = resolve_replica(replica);
    let identity = resolve_identity(repo, &replica_id, author, email)?;
    let message = message.or_else(|| Some(format!("create mile {}", title)));

    let snapshot = command_mile_create(
        repo,
        &replica_id,
        &identity.signature,
        &title,
        description.as_deref(),
        message,
        if draft {
            MileStatus::Draft
        } else {
            MileStatus::Open
        },
    )?;
    println!("{}", snapshot.id);
    Ok(())
}

fn run_mile_list(repo: &Path, args: MileListArgs) -> Result<()> {
    let MileListArgs { all, format } = args;
    let mut miles = command_mile_list(repo)?;
    if !all {
        miles.retain(|mile| mile.status != MileStatus::Closed);
    }

    match format {
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

    Ok(())
}

fn handle_create_command(
    repo: &Path,
    replica: Option<&str>,
    author: Option<&str>,
    email: Option<&str>,
    command: CreateCommand,
) -> Result<()> {
    match command {
        CreateCommand::Mile(args) => run_mile_create(repo, replica, author, email, args)?,
        CreateCommand::Identity(args) => run_identity_create(repo, replica, author, email, args)?,
    }

    Ok(())
}

fn handle_list_command(
    repo: &Path,
    replica: Option<&str>,
    author: Option<&str>,
    email: Option<&str>,
    command: ListCommand,
) -> Result<()> {
    match command {
        ListCommand::Mile(args) => run_mile_list(repo, args)?,
        ListCommand::Identity(args) => run_identity_list(repo, replica, author, email, args)?,
    }

    Ok(())
}

fn handle_adopt_command(
    repo: &Path,
    replica: Option<&str>,
    author: Option<&str>,
    email: Option<&str>,
    command: AdoptCommand,
) -> Result<()> {
    match command {
        AdoptCommand::Identity(args) => run_identity_adopt(repo, replica, author, email, args)?,
    }

    Ok(())
}

fn handle_protect_command(
    repo: &Path,
    replica: Option<&str>,
    author: Option<&str>,
    email: Option<&str>,
    command: ProtectCommand,
) -> Result<()> {
    match command {
        ProtectCommand::Identity(args) => run_identity_protect(repo, replica, author, email, args)?,
    }

    Ok(())
}

fn handle_mile_command(
    repo: &Path,
    replica: Option<&str>,
    author: Option<&str>,
    email: Option<&str>,
    command: MileCommand,
) -> Result<()> {
    match command {
        MileCommand::Create(args) => run_mile_create(repo, replica, author, email, args)?,
        MileCommand::List(args) => run_mile_list(repo, args)?,
    }
    Ok(())
}

fn run_identity_create(
    repo: &Path,
    replica: Option<&str>,
    author: Option<&str>,
    email: Option<&str>,
    args: IdentityCreateArgs,
) -> Result<()> {
    let IdentityCreateArgs {
        display_name,
        email: identity_email,
        login,
        signature,
        message,
        adopt,
        protect_pgp,
        protect_pgp_armored,
    } = args;

    if !protect_pgp_armored.is_empty() && protect_pgp.len() != protect_pgp_armored.len() {
        return Err(anyhow!(
            "--protect-pgp-armored must be specified the same number of times as --protect-pgp"
        ));
    }

    let replica_id = resolve_replica(replica);
    let actor = resolve_identity(repo, &replica_id, author, email)?;
    let store = IdentityStore::open(repo)?;

    let mut protections = Vec::new();
    for (index, fingerprint) in protect_pgp.into_iter().enumerate() {
        let armored_public_key = protect_pgp_armored
            .get(index)
            .map(|path| {
                fs::read_to_string(path)
                    .with_context(|| format!("failed to read armored key at {}", path.display()))
            })
            .transpose()?;

        protections.push(IdentityProtection {
            kind: ProtectionKind::Pgp,
            fingerprint,
            armored_public_key,
        });
    }

    let snapshot = store.create_identity(CreateIdentityInput {
        replica_id: replica_id.clone(),
        author: actor.signature.clone(),
        message,
        display_name,
        email: identity_email,
        login,
        initial_signature: signature,
        adopt_immediately: adopt,
        protections,
    })?;
    println!("{}", snapshot.id);
    Ok(())
}

fn run_identity_list(
    repo: &Path,
    _replica: Option<&str>,
    _author: Option<&str>,
    _email: Option<&str>,
    args: IdentityListArgs,
) -> Result<()> {
    let IdentityListArgs { format } = args;
    let store = IdentityStore::open(repo)?;
    let identities = store.list_identities()?;

    match format {
        OutputFormat::Table => {
            if identities.is_empty() {
                println!("No identities found");
            } else {
                print_identity_table(&identities);
            }
        }
        OutputFormat::Raw => print_identity_raw(&identities),
        OutputFormat::Json => {
            let stdout = io::stdout();
            let mut handle = stdout.lock();
            to_writer_pretty(&mut handle, &identities)?;
            handle.write_all(b"\n")?;
        }
    }

    Ok(())
}

fn run_identity_adopt(
    repo: &Path,
    replica: Option<&str>,
    author: Option<&str>,
    email: Option<&str>,
    args: IdentityAdoptArgs,
) -> Result<()> {
    let IdentityAdoptArgs {
        identity_id,
        signature,
        message,
    } = args;
    let identity_id = parse_entity_id(&identity_id)?;
    let replica_id = resolve_replica(replica);
    let actor = resolve_identity(repo, &replica_id, author, email)?;
    let store = IdentityStore::open(repo)?;
    let current = store.load_identity(&identity_id)?;
    let signature =
        signature.unwrap_or_else(|| format!("{} <{}>", current.display_name, current.email));
    let outcome = store.adopt_identity(AdoptIdentityInput {
        identity_id: identity_id.clone(),
        replica_id: replica_id.clone(),
        author: actor.signature.clone(),
        message,
        signature,
    })?;
    if outcome.changed {
        println!("Identity {} adopted for {}", identity_id, replica_id);
    } else {
        println!(
            "Identity {} already adopted for {}",
            identity_id, replica_id
        );
    }

    Ok(())
}

fn run_identity_protect(
    repo: &Path,
    replica: Option<&str>,
    author: Option<&str>,
    email: Option<&str>,
    args: IdentityProtectArgs,
) -> Result<()> {
    let IdentityProtectArgs {
        identity_id,
        pgp_fingerprint,
        armored_key,
        message,
    } = args;

    let identity_id = parse_entity_id(&identity_id)?;
    let replica_id = resolve_replica(replica);
    let actor = resolve_identity(repo, &replica_id, author, email)?;
    let store = IdentityStore::open(repo)?;

    let armored_public_key = match armored_key {
        Some(path) => Some(
            fs::read_to_string(&path)
                .with_context(|| format!("failed to read armored key at {}", path.display()))?,
        ),
        None => None,
    };

    let outcome = store.add_protection(AddProtectionInput {
        identity_id: identity_id.clone(),
        replica_id: replica_id.clone(),
        author: actor.signature.clone(),
        message,
        protection: IdentityProtection {
            kind: ProtectionKind::Pgp,
            fingerprint: pgp_fingerprint,
            armored_public_key,
        },
    })?;

    if outcome.changed {
        println!("Protection added to identity {}", identity_id);
    } else {
        println!("Protection already registered on identity {}", identity_id);
    }

    Ok(())
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
    replica_id: &ReplicaId,
    name_override: Option<&str>,
    email_override: Option<&str>,
) -> Result<Identity> {
    let overrides_present = name_override.is_some() || email_override.is_some();
    let mut name = name_override.map(|value| value.to_string());
    let mut email = email_override.map(|value| value.to_string());
    let mut identity_signature: Option<String> = None;

    if !overrides_present || name.is_none() || email.is_none() {
        if let Ok(store) = IdentityStore::open(repo) {
            if let Ok(Some(snapshot)) = store.find_adopted_by_replica(replica_id) {
                if !overrides_present && name.is_none() && email.is_none() {
                    if let Some(signature) = snapshot.signature.clone() {
                        return Ok(Identity { signature });
                    }
                }

                if name.is_none() {
                    name = Some(snapshot.display_name.clone());
                }
                if email.is_none() {
                    email = Some(snapshot.email.clone());
                }

                if !overrides_present {
                    identity_signature = snapshot.signature.clone();
                }
            }
        }
    }

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

    let signature = if !overrides_present {
        if let Some(signature) = identity_signature {
            signature
        } else if email.is_empty() {
            name.clone()
        } else {
            format!("{name} <{email}>")
        }
    } else if email.is_empty() {
        name.clone()
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

fn print_identity_table(identities: &[IdentitySummary]) {
    println!(
        "{:<40} {:<16} {:<20} {}",
        "ID", "STATUS", "ADOPTED_BY", "DISPLAY NAME"
    );
    for identity in identities {
        let adopted = identity
            .adopted_by
            .as_ref()
            .map(|replica| replica.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{:<40} {:<16} {:<20} {}",
            identity.id, identity.status, adopted, identity.display_name
        );
    }
}

fn print_identity_raw(identities: &[IdentitySummary]) {
    for identity in identities {
        let adopted = identity
            .adopted_by
            .as_ref()
            .map(|replica| replica.to_string())
            .unwrap_or_else(|| "-".to_string());
        println!(
            "{}\t{}\t{}\t{}",
            identity.id, identity.status, adopted, identity.display_name
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::{
        AdoptIdentityInput, CreateIdentityInput, EntityId, IdentityStatus, IdentityStore,
        LamportClock, Operation, OperationBlob, OperationMetadata, OperationPack, ReplicaId,
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

    #[test]
    fn identity_command_flow() -> Result<()> {
        let temp = tempfile::tempdir()?;
        Repository::init_bare(temp.path())?;
        let repo = temp.path();
        let replica = "cli-tests";

        handle_create_command(
            repo,
            Some(replica),
            Some("Tester"),
            Some("tester@example.com"),
            CreateCommand::Identity(IdentityCreateArgs {
                display_name: "Alice".into(),
                email: "alice@example.com".into(),
                login: Some("alice".into()),
                signature: None,
                message: None,
                adopt: false,
                protect_pgp: Vec::new(),
                protect_pgp_armored: Vec::new(),
            }),
        )?;

        let store = IdentityStore::open(repo)?;
        let summaries = store.list_identities()?;
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].status, IdentityStatus::PendingAdoption);
        let identity_id = summaries[0].id.clone();

        handle_adopt_command(
            repo,
            Some(replica),
            Some("Tester"),
            Some("tester@example.com"),
            AdoptCommand::Identity(IdentityAdoptArgs {
                identity_id: identity_id.to_string(),
                signature: Some("Alice <alice@example.com>".into()),
                message: None,
            }),
        )?;

        handle_protect_command(
            repo,
            Some(replica),
            Some("Tester"),
            Some("tester@example.com"),
            ProtectCommand::Identity(IdentityProtectArgs {
                identity_id: identity_id.to_string(),
                pgp_fingerprint: "FP".into(),
                armored_key: None,
                message: None,
            }),
        )?;

        let snapshot = IdentityStore::open(repo)?.load_identity(&identity_id)?;
        assert_eq!(snapshot.status, IdentityStatus::Protected);
        assert_eq!(snapshot.protections.len(), 1);
        Ok(())
    }

    #[test]
    fn resolve_identity_prefers_adopted_identity() -> Result<()> {
        let temp = tempfile::tempdir()?;
        Repository::init_bare(temp.path())?;
        let repo = temp.path();
        let replica = ReplicaId::new("cli-tests");

        let store = IdentityStore::open(repo)?;
        let identity = store.create_identity(CreateIdentityInput {
            replica_id: replica.clone(),
            author: "tester".into(),
            message: None,
            display_name: "Alice".into(),
            email: "alice@example.com".into(),
            login: None,
            initial_signature: None,
            adopt_immediately: false,
            protections: vec![],
        })?;
        store.adopt_identity(AdoptIdentityInput {
            identity_id: identity.id.clone(),
            replica_id: replica.clone(),
            author: "tester".into(),
            message: None,
            signature: "Alice <alice@example.com>".into(),
        })?;

        let resolved = resolve_identity(repo, &replica, None, None)?;
        assert_eq!(resolved.signature, "Alice <alice@example.com>");
        Ok(())
    }

    #[test]
    fn mile_create_uses_adopted_identity_signature() -> Result<()> {
        let temp = tempfile::tempdir()?;
        Repository::init_bare(temp.path())?;
        let repo = temp.path();
        let replica = ReplicaId::new("cli-tests");

        let store = IdentityStore::open(repo)?;
        let identity = store.create_identity(CreateIdentityInput {
            replica_id: replica.clone(),
            author: "tester".into(),
            message: None,
            display_name: "Alice".into(),
            email: "alice@example.com".into(),
            login: None,
            initial_signature: None,
            adopt_immediately: false,
            protections: vec![],
        })?;
        store.adopt_identity(AdoptIdentityInput {
            identity_id: identity.id.clone(),
            replica_id: replica.clone(),
            author: "tester".into(),
            message: None,
            signature: "Alice <alice@example.com>".into(),
        })?;

        run_mile_create(
            repo,
            Some(replica.as_str()),
            None,
            None,
            MileCreateArgs {
                title: "Identity Mile".into(),
                description: None,
                message: None,
                draft: false,
            },
        )?;

        let miles = command_mile_list(repo)?;
        assert_eq!(miles.len(), 1);
        let snapshot = command_mile_show(repo, &miles[0].id)?;
        assert_eq!(
            snapshot.events[0].metadata.author,
            "Alice <alice@example.com>"
        );
        Ok(())
    }
}
