use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use clap::{Parser, Subcommand, ValueEnum};
use git_mile_core::{
    EntityId, EntitySnapshot, EntityStore, EntitySummary, MergeOutcome, MergeStrategy, OperationId,
    app_version,
};

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err:?}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Entity(entity) => handle_entity(entity),
    }
}

#[derive(Parser)]
#[command(name = git_mile_core::APP_NAME, version = app_version())]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Entity(EntityArgs),
}

#[derive(Parser)]
struct EntityArgs {
    #[command(subcommand)]
    command: EntityCommand,
}

#[derive(Subcommand)]
enum EntityCommand {
    List(EntityListArgs),
    Show(EntityShowArgs),
    Resolve(EntityResolveArgs),
}

#[derive(Parser)]
struct EntityListArgs {
    #[arg(long, default_value = ".")]
    repo: PathBuf,
}

#[derive(Parser)]
struct EntityShowArgs {
    entity: String,
    #[arg(long, default_value = ".")]
    repo: PathBuf,
}

#[derive(Parser)]
struct EntityResolveArgs {
    entity: String,
    #[arg(long, value_enum, default_value = "ours")]
    strategy: ResolveStrategy,
    #[arg(long = "head")]
    heads: Vec<String>,
    #[arg(long, default_value = ".")]
    repo: PathBuf,
}

#[derive(Copy, Clone, Debug, ValueEnum, Eq, PartialEq)]
enum ResolveStrategy {
    Ours,
    Theirs,
    Manual,
}

fn handle_entity(args: EntityArgs) -> Result<()> {
    match args.command {
        EntityCommand::List(list) => {
            let summaries = command_entity_list(&list.repo)?;
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
            let snapshot = command_entity_show(&show.repo, &entity_id)?;
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
            let outcome = command_entity_resolve(&resolve.repo, &entity_id, strategy)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::{
        EntityId, LamportClock, Operation, OperationBlob, OperationId, OperationMetadata,
        OperationPack, ReplicaId,
    };
    use git2::Repository;
    use tempfile::TempDir;

    fn setup_repo() -> Result<(TempDir, EntityId, OperationId, OperationId, OperationId)> {
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
    fn list_entities_returns_summary() -> Result<()> {
        let (temp, entity_id, _, _, _) = setup_repo()?;
        let summaries = command_entity_list(temp.path())?;
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].entity_id, entity_id);
        assert_eq!(summaries[0].head_count, 2);
        Ok(())
    }

    #[test]
    fn show_entity_loads_snapshot() -> Result<()> {
        let (temp, entity_id, _, _, _) = setup_repo()?;
        let snapshot = command_entity_show(temp.path(), &entity_id)?;
        assert_eq!(snapshot.entity_id, entity_id);
        assert_eq!(snapshot.operations.len(), 3);
        assert_eq!(snapshot.heads.len(), 2);
        Ok(())
    }

    #[test]
    fn resolve_manual_reduces_heads() -> Result<()> {
        let (temp, entity_id, _, op_a, op_b) = setup_repo()?;
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
}
