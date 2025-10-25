use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::str::{self, FromStr};
use std::sync::Arc;

use git2::{Commit, ErrorCode, FileMode, Oid, Repository, Signature, Tree};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::clock::{LamportTimestamp, ReplicaId};
use crate::error::{Error, Result};
use crate::repo::{LockMode, NoopCache, RepositoryCacheHook, RepositoryLock, RepositoryLockGuard};

use super::entity::{EntityId, OperationId};
use super::pack::{BlobRef, Operation, OperationBlob, OperationMetadata, OperationPack};

pub struct EntityStore {
    backend: GitBackend,
    _lock: RepositoryLockGuard,
    cache: Arc<dyn RepositoryCacheHook>,
}

impl EntityStore {
    pub fn open(repo_path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_mode(repo_path, LockMode::Write)
    }

    pub fn open_with_mode(repo_path: impl AsRef<Path>, mode: LockMode) -> Result<Self> {
        let cache: Arc<dyn RepositoryCacheHook> = Arc::new(NoopCache::default());
        Self::open_with_cache(repo_path, mode, cache)
    }

    pub fn open_with_cache(
        repo_path: impl AsRef<Path>,
        mode: LockMode,
        cache: Arc<dyn RepositoryCacheHook>,
    ) -> Result<Self> {
        let backend = GitBackend::open(repo_path)?;
        let repo_path = backend.repository_path().to_path_buf();
        let lock = RepositoryLock::acquire(&repo_path, mode)?;
        Ok(Self {
            backend,
            _lock: lock,
            cache,
        })
    }

    pub fn persist_pack(&self, pack: OperationPack) -> Result<PackPersistResult> {
        let entity_id = pack.entity_id.clone();
        let mut entity = self.backend.load_or_default(entity_id.clone())?;
        let inserted = self.backend.apply_pack(&mut entity, pack)?;
        self.backend
            .write_entity(&mut entity, "persist operation pack")?;

        let result = PackPersistResult {
            inserted,
            clock_snapshot: entity.clock.clone(),
        };

        self.cache.invalidate_entity(&entity_id)?;
        self.cache
            .on_pack_persisted(&entity_id, &result.inserted, &result.clock_snapshot)?;

        Ok(result)
    }

    pub fn load_entity(&self, entity_id: &EntityId) -> Result<EntitySnapshot> {
        if let Some(snapshot) = self.cache.try_get_snapshot(entity_id)? {
            return Ok(snapshot);
        }

        let entity = self
            .backend
            .load_entity(entity_id)?
            .ok_or_else(|| Error::validation(format!("entity {} not found", entity_id)))?;
        let snapshot = self.backend.snapshot_from_entity(entity)?;
        self.cache.on_entity_loaded(entity_id, &snapshot)?;
        Ok(snapshot)
    }

    pub fn list_entities(&self) -> Result<Vec<EntitySummary>> {
        self.backend.list_entities()
    }

    pub fn resolve_conflicts(
        &self,
        entity_id: &EntityId,
        strategy: MergeStrategy,
    ) -> Result<MergeOutcome> {
        let mut entity = self
            .backend
            .load_entity(entity_id)?
            .ok_or_else(|| Error::validation(format!("entity {} not found", entity_id)))?;

        if entity.heads.len() <= 1 {
            return Ok(MergeOutcome {
                heads: entity.sorted_heads(),
            });
        }

        let chosen_heads = match strategy {
            MergeStrategy::Ours => entity.heads.iter().max().cloned().into_iter().collect(),
            MergeStrategy::Theirs => entity.heads.iter().min().cloned().into_iter().collect(),
            MergeStrategy::Manual(selected) => {
                if selected.is_empty() {
                    return Err(Error::validation(
                        "manual merge strategy requires at least one head",
                    ));
                }

                let mut unique = HashSet::new();
                let mut filtered = Vec::new();
                for head in selected {
                    if !entity.heads.contains(&head) {
                        return Err(Error::validation(format!(
                            "operation {} is not a current head",
                            head
                        )));
                    }

                    if unique.insert(head.clone()) {
                        filtered.push(head);
                    }
                }

                filtered
            }
        };

        if chosen_heads.is_empty() {
            return Err(Error::validation("merge strategy did not select any heads"));
        }

        entity.heads = chosen_heads.iter().cloned().collect();
        self.backend
            .write_entity(&mut entity, "resolve entity conflicts")?;

        self.cache.invalidate_entity(entity_id)?;

        Ok(MergeOutcome {
            heads: entity.sorted_heads(),
        })
    }
}

pub struct PackPersistResult {
    pub inserted: Vec<OperationId>,
    pub clock_snapshot: LamportTimestamp,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntitySnapshot {
    pub entity_id: EntityId,
    pub clock_snapshot: LamportTimestamp,
    pub heads: Vec<OperationId>,
    pub operations: Vec<Operation>,
    pub blobs: Vec<OperationBlob>,
}

pub struct EntitySummary {
    pub entity_id: EntityId,
    pub head_count: usize,
}

pub enum MergeStrategy {
    Ours,
    Theirs,
    Manual(Vec<OperationId>),
}

pub struct MergeOutcome {
    pub heads: Vec<OperationId>,
}

struct GitBackend {
    repo: Repository,
}

impl GitBackend {
    fn open(repo_path: impl AsRef<Path>) -> Result<Self> {
        let path = repo_path.as_ref();
        let repo = match Repository::open(path) {
            Ok(repo) => repo,
            Err(err) if err.code() == ErrorCode::NotFound => Repository::init(path)?,
            Err(err) => return Err(err.into()),
        };

        Ok(Self { repo })
    }

    fn repository_path(&self) -> &Path {
        self.repo.path()
    }

    fn load_or_default(&self, entity_id: EntityId) -> Result<StoredEntity> {
        match self.load_entity(&entity_id)? {
            Some(entity) => Ok(entity),
            None => Ok(StoredEntity::new(entity_id)),
        }
    }

    fn load_entity(&self, entity_id: &EntityId) -> Result<Option<StoredEntity>> {
        let refname = entity_ref_name(entity_id);
        let reference = match self.repo.find_reference(&refname) {
            Ok(reference) => reference,
            Err(err) if err.code() == ErrorCode::NotFound => return Ok(None),
            Err(err) => return Err(err.into()),
        };

        let commit = reference.peel_to_commit()?;
        let tree = commit.tree()?;

        let clock = self.read_clock(&tree)?;
        let operations = self.read_operations(&tree)?;
        let mut heads = self.read_heads(&tree)?;
        if heads.is_empty() && !operations.is_empty() {
            heads = compute_heads(&operations);
        }
        let blobs = self.read_blobs(&tree)?;

        Ok(Some(StoredEntity {
            entity_id: entity_id.clone(),
            clock,
            operations,
            blobs,
            heads,
            commit_oid: Some(commit.id()),
        }))
    }

    fn apply_pack(
        &self,
        entity: &mut StoredEntity,
        pack: OperationPack,
    ) -> Result<Vec<OperationId>> {
        pack.validate()?;

        let OperationPack {
            entity_id: _,
            clock_snapshot,
            operations,
            content_blobs,
        } = pack;

        for blob in content_blobs.into_iter() {
            self.ensure_blob(entity, blob)?;
        }

        let mut inserted = Vec::new();
        let mut known_ops: HashSet<OperationId> = entity.operations.keys().cloned().collect();

        for operation in operations.into_iter() {
            if entity.operations.contains_key(&operation.id) {
                return Err(Error::validation(format!(
                    "operation {} already exists",
                    operation.id
                )));
            }

            for parent in &operation.parents {
                if !known_ops.contains(parent) {
                    return Err(Error::validation(format!(
                        "operation {} references missing parent {}",
                        operation.id, parent
                    )));
                }
            }

            known_ops.insert(operation.id.clone());
            entity.heads.insert(operation.id.clone());
            for parent in &operation.parents {
                entity.heads.remove(parent);
            }

            let op_id = operation.id.clone();
            entity.operations.insert(op_id.clone(), operation);
            inserted.push(op_id);
        }

        if clock_snapshot > entity.clock {
            entity.clock = clock_snapshot;
        }

        Ok(inserted)
    }

    fn ensure_blob(&self, entity: &mut StoredEntity, blob: OperationBlob) -> Result<()> {
        if entity.blobs.contains_key(blob.digest()) {
            return Ok(());
        }

        let expected = BlobRef::from_bytes(blob.data());
        if &expected != blob.digest() {
            return Err(Error::validation("operation blob digest mismatch"));
        }

        let digest = blob.digest().clone();
        let oid = self.repo.blob(blob.data())?;
        entity.blobs.insert(digest, oid);
        Ok(())
    }

    fn write_entity(&self, entity: &mut StoredEntity, message: &str) -> Result<()> {
        let tree_oid = self.build_entity_tree(entity)?;
        let tree = self.repo.find_tree(tree_oid)?;

        let mut parents = Vec::new();
        if let Some(parent_oid) = entity.commit_oid {
            parents.push(self.repo.find_commit(parent_oid)?);
        }
        let parent_refs: Vec<&Commit> = parents.iter().collect();

        let signature = self.signature()?;
        let commit_message = format!("{}: {}", message, entity.entity_id);
        let commit_oid = self.repo.commit(
            Some(&entity_ref_name(&entity.entity_id)),
            &signature,
            &signature,
            &commit_message,
            &tree,
            &parent_refs,
        )?;

        entity.commit_oid = Some(commit_oid);
        Ok(())
    }

    fn snapshot_from_entity(&self, entity: StoredEntity) -> Result<EntitySnapshot> {
        let StoredEntity {
            entity_id,
            clock,
            operations,
            blobs,
            heads,
            ..
        } = entity;

        let mut operations_vec = operations.into_values().collect::<Vec<_>>();
        operations_vec.sort_by(|a, b| a.id.cmp(&b.id));

        let mut blob_entries: Vec<_> = blobs.into_iter().collect();
        blob_entries.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));

        let mut blob_vec = Vec::with_capacity(blob_entries.len());
        for (digest, oid) in blob_entries {
            let blob = self.repo.find_blob(oid)?;
            let restored = OperationBlob::from_bytes(blob.content().to_vec());
            if restored.digest() != &digest {
                return Err(Error::validation(format!(
                    "blob digest mismatch for {}",
                    digest
                )));
            }
            blob_vec.push(restored);
        }

        let mut heads_vec = heads.into_iter().collect::<Vec<_>>();
        heads_vec.sort();

        Ok(EntitySnapshot {
            entity_id,
            clock_snapshot: clock,
            heads: heads_vec,
            operations: operations_vec,
            blobs: blob_vec,
        })
    }

    fn list_entities(&self) -> Result<Vec<EntitySummary>> {
        let mut summaries = Vec::new();
        let mut references = self.repo.references_glob("refs/git-mile/entities/*")?;

        while let Some(reference) = references.next() {
            let reference = reference?;
            let name = reference
                .name()
                .ok_or_else(|| Error::validation("invalid reference name"))?;
            let Some(id_str) = name.strip_prefix("refs/git-mile/entities/") else {
                continue;
            };

            let entity_id = EntityId::from_str(id_str).map_err(|err| {
                Error::validation(format!("invalid entity id in reference: {err}"))
            })?;

            if let Some(entity) = self.load_entity(&entity_id)? {
                summaries.push(EntitySummary {
                    entity_id,
                    head_count: entity.heads.len(),
                });
            }
        }

        summaries.sort_by(|a, b| a.entity_id.to_string().cmp(&b.entity_id.to_string()));
        Ok(summaries)
    }

    fn read_clock(&self, tree: &Tree) -> Result<LamportTimestamp> {
        let entry = tree
            .get_name("clock.json")
            .ok_or_else(|| Error::validation("entity missing clock.json"))?;
        let blob = self.repo.find_blob(entry.id())?;
        Ok(serde_json::from_slice(blob.content())?)
    }

    fn read_heads(&self, tree: &Tree) -> Result<HashSet<OperationId>> {
        let Some(entry) = tree.get_name("index.json") else {
            return Ok(HashSet::new());
        };

        let blob = self.repo.find_blob(entry.id())?;
        let index: EntityIndex = serde_json::from_slice(blob.content())?;
        Ok(index.heads.into_iter().collect())
    }

    fn read_blobs(&self, tree: &Tree) -> Result<HashMap<BlobRef, Oid>> {
        let Some(entry) = tree.get_name("blobs") else {
            return Ok(HashMap::new());
        };

        let blobs_tree = self.repo.find_tree(entry.id())?;
        let mut blobs = HashMap::new();

        for blob_entry in blobs_tree.iter() {
            if blob_entry.kind() != Some(git2::ObjectType::Blob) {
                continue;
            }

            let name = blob_entry
                .name()
                .ok_or_else(|| Error::validation("invalid blob entry name"))?;
            let digest = name.strip_suffix(".blob").unwrap_or(name);
            blobs.insert(BlobRef::from_str(digest)?, blob_entry.id());
        }

        Ok(blobs)
    }

    fn read_operations(&self, tree: &Tree) -> Result<BTreeMap<OperationId, Operation>> {
        let Some(entry) = tree.get_name("pack") else {
            return Ok(BTreeMap::new());
        };

        let pack_tree = self.repo.find_tree(entry.id())?;
        let mut operations = BTreeMap::new();

        for node in pack_tree.iter() {
            if node.kind() != Some(git2::ObjectType::Tree) {
                continue;
            }
            let op_tree = self.repo.find_tree(node.id())?;
            let id = OperationId::from_str(&self.read_text_blob(&op_tree, "id")?)
                .map_err(|err| Error::validation(format!("invalid operation id: {err}")))?;

            let metadata: OperationMetadata =
                serde_json::from_slice(self.read_blob(&op_tree, "meta.json")?.content())?;
            let parents = self
                .read_text_blob(&op_tree, "parents")?
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    OperationId::from_str(line)
                        .map_err(|err| Error::validation(format!("invalid parent id: {err}")))
                })
                .collect::<Result<Vec<_>>>()?;
            let payload = BlobRef::from_str(self.read_text_blob(&op_tree, "payload")?.trim())?;

            operations.insert(
                id.clone(),
                Operation {
                    id,
                    parents,
                    payload,
                    metadata,
                },
            );
        }

        Ok(operations)
    }

    fn read_blob(&self, tree: &Tree, name: &str) -> Result<git2::Blob<'_>> {
        let entry = tree
            .get_name(name)
            .ok_or_else(|| Error::validation(format!("missing {name} file")))?;
        Ok(self.repo.find_blob(entry.id())?)
    }

    fn read_text_blob(&self, tree: &Tree, name: &str) -> Result<String> {
        let blob = self.read_blob(tree, name)?;
        let content = str::from_utf8(blob.content())
            .map_err(|_| Error::validation(format!("{name} is not valid UTF-8")))?;
        Ok(content.to_string())
    }

    fn build_entity_tree(&self, entity: &StoredEntity) -> Result<Oid> {
        let mut builder = self.repo.treebuilder(None)?;

        let clock_bytes = serde_json::to_vec(&entity.clock)?;
        let clock_oid = self.repo.blob(&clock_bytes)?;
        builder.insert("clock.json", clock_oid, FileMode::Blob.into())?;

        let index = EntityIndex {
            heads: entity.sorted_heads(),
        };
        let index_bytes = serde_json::to_vec(&index)?;
        let index_oid = self.repo.blob(&index_bytes)?;
        builder.insert("index.json", index_oid, FileMode::Blob.into())?;

        if let Some(blobs_oid) = self.build_blobs_tree(entity)? {
            builder.insert("blobs", blobs_oid, FileMode::Tree.into())?;
        }

        if let Some(pack_oid) = self.build_pack_tree(entity)? {
            builder.insert("pack", pack_oid, FileMode::Tree.into())?;
        }

        Ok(builder.write()?)
    }

    fn build_blobs_tree(&self, entity: &StoredEntity) -> Result<Option<Oid>> {
        if entity.blobs.is_empty() {
            return Ok(None);
        }

        let mut builder = self.repo.treebuilder(None)?;
        let mut entries: Vec<_> = entity.blobs.iter().collect();
        entries.sort_by(|(left, _), (right, _)| left.as_str().cmp(right.as_str()));

        for (digest, oid) in entries {
            let filename = format!("{}.blob", digest.as_str());
            builder.insert(filename, *oid, FileMode::Blob.into())?;
        }

        Ok(Some(builder.write()?))
    }

    fn build_pack_tree(&self, entity: &StoredEntity) -> Result<Option<Oid>> {
        if entity.operations.is_empty() {
            return Ok(None);
        }

        let mut builder = self.repo.treebuilder(None)?;

        for (op_id, operation) in &entity.operations {
            let mut op_builder = self.repo.treebuilder(None)?;

            let id_blob = self.repo.blob(operation.id.to_string().as_bytes())?;
            op_builder.insert("id", id_blob, FileMode::Blob.into())?;

            let meta_blob = self.repo.blob(&serde_json::to_vec(&operation.metadata)?)?;
            op_builder.insert("meta.json", meta_blob, FileMode::Blob.into())?;

            let parents_blob = self.repo.blob(
                operation
                    .parents
                    .iter()
                    .map(|parent| parent.to_string())
                    .collect::<Vec<_>>()
                    .join("\n")
                    .as_bytes(),
            )?;
            op_builder.insert("parents", parents_blob, FileMode::Blob.into())?;

            let payload_blob = self.repo.blob(operation.payload.as_str().as_bytes())?;
            op_builder.insert("payload", payload_blob, FileMode::Blob.into())?;

            let op_tree_oid = op_builder.write()?;
            builder.insert(
                operation_dir_name(op_id),
                op_tree_oid,
                FileMode::Tree.into(),
            )?;
        }

        Ok(Some(builder.write()?))
    }

    fn signature(&self) -> Result<Signature<'_>> {
        match self.repo.signature() {
            Ok(signature) => Ok(signature),
            Err(_) => Ok(Signature::now("git-mile", "git-mile@localhost")?),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct EntityIndex {
    heads: Vec<OperationId>,
}

struct StoredEntity {
    entity_id: EntityId,
    clock: LamportTimestamp,
    operations: BTreeMap<OperationId, Operation>,
    blobs: HashMap<BlobRef, Oid>,
    heads: HashSet<OperationId>,
    commit_oid: Option<Oid>,
}

impl StoredEntity {
    fn new(entity_id: EntityId) -> Self {
        Self {
            entity_id,
            clock: LamportTimestamp::new(0, ReplicaId::new("system")),
            operations: BTreeMap::new(),
            blobs: HashMap::new(),
            heads: HashSet::new(),
            commit_oid: None,
        }
    }

    fn sorted_heads(&self) -> Vec<OperationId> {
        let mut heads = self.heads.iter().cloned().collect::<Vec<_>>();
        heads.sort();
        heads
    }
}

fn entity_ref_name(entity_id: &EntityId) -> String {
    format!("refs/git-mile/entities/{}", entity_id)
}

fn compute_heads(operations: &BTreeMap<OperationId, Operation>) -> HashSet<OperationId> {
    let mut heads: HashSet<OperationId> = operations.keys().cloned().collect();
    for operation in operations.values() {
        for parent in &operation.parents {
            heads.remove(parent);
        }
    }
    heads
}

fn operation_dir_name(operation_id: &OperationId) -> String {
    let counter = format!("{:020}", operation_id.timestamp().counter());
    let replica = sanitize_replica(operation_id.replica_id().as_str());
    let digest = Sha256::digest(operation_id.to_string().as_bytes());
    let suffix = hex::encode(&digest[..4]);
    format!("{counter}-{replica}-{suffix}")
}

fn sanitize_replica(replica: &str) -> String {
    replica
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{LamportClock, ReplicaId};
    use crate::dag::pack::{OperationMetadata, OperationPack};

    fn init_store() -> (tempfile::TempDir, EntityStore) {
        let temp = tempfile::tempdir().expect("create temp dir");
        Repository::init_bare(temp.path()).expect("init repo");
        let store = EntityStore::open(temp.path()).expect("open store");
        (temp, store)
    }

    fn build_operation(
        clock: &mut LamportClock,
        parents: Vec<OperationId>,
        payload: &OperationBlob,
    ) -> Operation {
        Operation::new(
            OperationId::new(clock.tick().unwrap()),
            parents,
            payload.digest().clone(),
            OperationMetadata::new("tester", Some("op".to_string())),
        )
    }

    #[test]
    fn persist_pack_roundtrip() {
        let (_tmp, store) = init_store();
        let entity_id = EntityId::new();
        let mut clock = LamportClock::new(ReplicaId::new("node-a"));
        let blob = OperationBlob::from_bytes(b"payload".to_vec());
        let operation = build_operation(&mut clock, vec![], &blob);

        let pack = OperationPack::new(
            entity_id.clone(),
            clock.snapshot(),
            vec![operation.clone()],
            vec![blob.clone()],
        )
        .expect("pack should validate");

        let result = store.persist_pack(pack).expect("persist pack");
        assert_eq!(result.inserted.len(), 1);

        let snapshot = store.load_entity(&entity_id).expect("load entity");
        assert_eq!(snapshot.operations.len(), 1);
        assert_eq!(snapshot.blobs.len(), 1);
        assert_eq!(snapshot.heads.len(), 1);
        assert_eq!(snapshot.heads[0], operation.id);
        assert_eq!(snapshot.blobs[0].digest(), &operation.payload);
    }

    #[test]
    fn list_entities_reports_summaries() {
        let (_tmp, store) = init_store();
        let entity_id = EntityId::new();
        let mut clock = LamportClock::new(ReplicaId::new("node-a"));
        let blob = OperationBlob::from_bytes(b"payload".to_vec());
        let operation = build_operation(&mut clock, vec![], &blob);
        let pack = OperationPack::new(
            entity_id.clone(),
            clock.snapshot(),
            vec![operation],
            vec![blob],
        )
        .expect("pack");
        store.persist_pack(pack).expect("persist");

        let summaries = store.list_entities().expect("list");
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].entity_id, entity_id);
        assert_eq!(summaries[0].head_count, 1);
    }

    #[test]
    fn resolve_conflicts_prefers_latest_head() {
        let (_tmp, store) = init_store();
        let entity_id = EntityId::new();
        let mut clock = LamportClock::new(ReplicaId::new("node-a"));

        let base_blob = OperationBlob::from_bytes(b"base".to_vec());
        let base_op = build_operation(&mut clock, vec![], &base_blob);
        let base_pack = OperationPack::new(
            entity_id.clone(),
            clock.snapshot(),
            vec![base_op.clone()],
            vec![base_blob.clone()],
        )
        .expect("base pack");
        store.persist_pack(base_pack).expect("persist base");

        let branch_blob_a = OperationBlob::from_bytes(b"branch-a".to_vec());
        let branch_blob_b = OperationBlob::from_bytes(b"branch-b".to_vec());
        let op_a = build_operation(&mut clock, vec![base_op.id.clone()], &branch_blob_a);
        let op_b = build_operation(&mut clock, vec![base_op.id.clone()], &branch_blob_b);
        let pack = OperationPack::new(
            entity_id.clone(),
            clock.snapshot(),
            vec![op_a.clone(), op_b.clone()],
            vec![branch_blob_a, branch_blob_b],
        )
        .expect("branch pack");
        store.persist_pack(pack).expect("persist branches");

        let snapshot = store.load_entity(&entity_id).expect("load");
        assert_eq!(snapshot.heads.len(), 2);

        let outcome = store
            .resolve_conflicts(&entity_id, MergeStrategy::Ours)
            .expect("resolve");
        assert_eq!(outcome.heads.len(), 1);

        let snapshot = store.load_entity(&entity_id).expect("load after resolve");
        assert_eq!(snapshot.heads.len(), 1);
        let expected_head = std::cmp::max(op_a.id, op_b.id);
        assert_eq!(snapshot.heads[0], expected_head);
    }
}
