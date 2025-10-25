use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rocksdb::{
    ColumnFamily, ColumnFamilyDescriptor, DB, DEFAULT_COLUMN_FAMILY_NAME, IteratorMode, Options,
};
use serde::{Deserialize, Serialize};

use crate::clock::LamportTimestamp;
use crate::dag::{EntityId, EntitySnapshot, OperationId};
use crate::error::{Error, Result};

const CACHE_VERSION: u32 = 2;
const VERSION_FILE: &str = "VERSION";
const META_CF_NAME: &str = "meta";
const JOURNAL_CF_NAME: &str = "journal";
const GENERATION_KEY_PREFIX: &str = "generation";

/// Hook trait invoked around repository read/write events.
pub trait RepositoryCacheHook: Send + Sync {
    /// Attempt to retrieve a cached snapshot before hitting the backing store.
    fn try_get_snapshot(&self, _entity_id: &EntityId) -> Result<Option<EntitySnapshot>> {
        Ok(None)
    }

    /// Store the snapshot after it is loaded from the backing store.
    fn on_entity_loaded(&self, _entity_id: &EntityId, _snapshot: &EntitySnapshot) -> Result<()> {
        Ok(())
    }

    /// Notify the hook that a pack was persisted for the given entity.
    fn on_pack_persisted(
        &self,
        _entity_id: &EntityId,
        _inserted: &[OperationId],
        _clock: &LamportTimestamp,
    ) -> Result<()> {
        Ok(())
    }

    /// Invalidate any cached state for the given entity identifier.
    fn invalidate_entity(&self, _entity_id: &EntityId) -> Result<()> {
        Ok(())
    }
}

/// Default no-op cache hook used when no caching is configured.
#[derive(Debug, Default)]
pub struct NoopCache;

impl RepositoryCacheHook for NoopCache {}

/// Logical namespace used to isolate cache entries per entity family.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum CacheNamespace {
    Issues,
    Milestones,
    Bridges,
    Users,
    Labels,
    Identities,
}

impl CacheNamespace {
    pub const ALL: [CacheNamespace; 6] = [
        CacheNamespace::Issues,
        CacheNamespace::Milestones,
        CacheNamespace::Bridges,
        CacheNamespace::Users,
        CacheNamespace::Labels,
        CacheNamespace::Identities,
    ];

    fn cf_name(self) -> &'static str {
        match self {
            CacheNamespace::Issues => "issues",
            CacheNamespace::Milestones => "milestones",
            CacheNamespace::Bridges => "bridges",
            CacheNamespace::Users => "users",
            CacheNamespace::Labels => "labels",
            CacheNamespace::Identities => "identities",
        }
    }

    fn default_policy(self) -> CachePolicy {
        let ttl = match self {
            CacheNamespace::Issues | CacheNamespace::Milestones | CacheNamespace::Bridges => {
                Duration::from_secs(60 * 60 * 24)
            }
            CacheNamespace::Users | CacheNamespace::Labels | CacheNamespace::Identities => {
                Duration::from_secs(60 * 60 * 72)
            }
        };
        CachePolicy { ttl }
    }
}

/// Per-namespace policy describing how entries should be retained.
#[derive(Clone, Debug)]
pub struct CachePolicy {
    pub ttl: Duration,
}

/// Configuration describing how a persistent cache should be opened.
#[derive(Clone, Debug)]
pub struct CacheConfig {
    pub path: PathBuf,
    pub policies: HashMap<CacheNamespace, CachePolicy>,
    pub maintenance_interval: Duration,
    pub version: u32,
}

impl CacheConfig {
    pub const CURRENT_VERSION: u32 = CACHE_VERSION;

    /// Build a cache configuration for the given repository path.
    /// Returns `None` when the path does not look like a Git repository yet.
    pub fn for_repo(repo_path: impl AsRef<Path>) -> Option<Self> {
        let repo_path = repo_path.as_ref();
        let git_dir = find_git_dir(repo_path)?;
        let cache_root = git_dir.join("git-mile").join("cache");

        let mut policies = HashMap::new();
        for namespace in CacheNamespace::ALL {
            policies.insert(namespace, namespace.default_policy());
        }

        Some(Self {
            path: cache_root,
            policies,
            maintenance_interval: Duration::from_secs(300),
            version: Self::CURRENT_VERSION,
        })
    }
}

fn find_git_dir(repo_path: &Path) -> Option<PathBuf> {
    let git_dir = repo_path.join(".git");
    if git_dir.is_dir() {
        return Some(git_dir);
    }

    if repo_path.join("objects").is_dir() && repo_path.join("HEAD").is_file() {
        return Some(repo_path.to_path_buf());
    }

    None
}

/// Result returned when looking up a cache entry.
#[derive(Debug)]
pub enum CacheLoadOutcome<V> {
    Hit(V),
    Miss,
    Stale,
}

#[derive(Clone, Debug, Default)]
struct GenerationState {
    generation: u64,
    created_at: u64,
    base_clock: Option<LamportTimestamp>,
}

#[derive(Clone, Debug)]
pub struct CacheGenerationSnapshot {
    pub generation: u64,
    pub created_at: u64,
    pub base_clock: Option<LamportTimestamp>,
}

impl From<GenerationState> for CacheGenerationSnapshot {
    fn from(state: GenerationState) -> Self {
        Self {
            generation: state.generation,
            created_at: state.created_at,
            base_clock: state.base_clock,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct GenerationRecord {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    created_at: u64,
    #[serde(default)]
    base_clock: Option<LamportTimestamp>,
}

impl GenerationRecord {
    fn from_state(version: u32, state: &GenerationState) -> Self {
        Self {
            version,
            generation: state.generation,
            created_at: state.created_at,
            base_clock: state.base_clock.clone(),
        }
    }

    fn into_state(self) -> GenerationState {
        GenerationState {
            generation: self.generation,
            created_at: self.created_at,
            base_clock: self.base_clock,
        }
    }
}

#[derive(Serialize, Deserialize)]
struct CacheJournalEntry {
    namespace: String,
    entity_id: EntityId,
    generation: u64,
    inserted: Vec<OperationId>,
    persisted_at: u64,
    #[serde(default)]
    base_clock: Option<LamportTimestamp>,
}

/// Abstract repository used to persist cached entity snapshots.
pub trait CacheRepository: Send + Sync {
    fn get(
        &self,
        namespace: CacheNamespace,
        entity_id: &EntityId,
    ) -> Result<CacheLoadOutcome<EntitySnapshot>>;

    fn put(
        &self,
        namespace: CacheNamespace,
        entity_id: &EntityId,
        snapshot: &EntitySnapshot,
    ) -> Result<()>;

    fn invalidate(&self, namespace: CacheNamespace, entity_id: &EntityId) -> Result<()>;

    fn generation(&self, _namespace: CacheNamespace) -> Result<Option<CacheGenerationSnapshot>> {
        Ok(None)
    }

    fn on_pack_persisted(
        &self,
        _namespace: CacheNamespace,
        _entity_id: &EntityId,
        _inserted: &[OperationId],
        _clock: &LamportTimestamp,
    ) -> Result<()> {
        Ok(())
    }
}

/// RocksDB-backed persistent cache shared across entity stores.
#[derive(Clone)]
pub struct PersistentCache {
    inner: Arc<PersistentCacheInner>,
}

struct PersistentCacheInner {
    db: Arc<DB>,
    policies: HashMap<CacheNamespace, CachePolicy>,
    maintenance_interval: Duration,
    version: u32,
    shutdown: Arc<AtomicBool>,
    maintenance_handle: Mutex<Option<thread::JoinHandle<()>>>,
    generations: Mutex<HashMap<CacheNamespace, GenerationState>>,
}

impl PersistentCache {
    /// Open (or create) a persistent cache using the provided configuration.
    pub fn open(config: CacheConfig) -> Result<Self> {
        ensure_cache_directory(&config.path, config.version)?;

        let mut db_opts = Options::default();
        db_opts.create_if_missing(true);
        db_opts.create_missing_column_families(true);

        let mut descriptors = Vec::with_capacity(CacheNamespace::ALL.len() + 3);
        descriptors.push(ColumnFamilyDescriptor::new(
            DEFAULT_COLUMN_FAMILY_NAME,
            Options::default(),
        ));

        for namespace in CacheNamespace::ALL {
            descriptors.push(ColumnFamilyDescriptor::new(
                namespace.cf_name(),
                Options::default(),
            ));
        }
        descriptors.push(ColumnFamilyDescriptor::new(
            META_CF_NAME,
            Options::default(),
        ));
        descriptors.push(ColumnFamilyDescriptor::new(
            JOURNAL_CF_NAME,
            Options::default(),
        ));

        let db = Arc::new(DB::open_cf_descriptors(
            &db_opts,
            &config.path,
            descriptors,
        )?);
        let inner = Arc::new(PersistentCacheInner {
            db,
            policies: config.policies,
            maintenance_interval: config.maintenance_interval,
            version: config.version,
            shutdown: Arc::new(AtomicBool::new(false)),
            maintenance_handle: Mutex::new(None),
            generations: Mutex::new(HashMap::new()),
        });

        inner.initialize_generations()?;

        if !inner.maintenance_interval.is_zero() {
            start_maintenance(inner.clone())?;
        }

        Ok(Self { inner })
    }

    fn policy(&self, namespace: CacheNamespace) -> CachePolicy {
        self.inner
            .policies
            .get(&namespace)
            .cloned()
            .unwrap_or_else(|| namespace.default_policy())
    }

    fn db(&self) -> &DB {
        &self.inner.db
    }

    pub fn generation(&self, namespace: CacheNamespace) -> Result<CacheGenerationSnapshot> {
        self.inner
            .current_generation(namespace)
            .map(CacheGenerationSnapshot::from)
    }
}

impl PersistentCacheInner {
    fn initialize_generations(&self) -> Result<()> {
        for namespace in CacheNamespace::ALL {
            let _ = self.current_generation(namespace)?;
        }
        Ok(())
    }

    fn current_generation(&self, namespace: CacheNamespace) -> Result<GenerationState> {
        if let Ok(guard) = self.generations.lock() {
            if let Some(state) = guard.get(&namespace) {
                return Ok(state.clone());
            }
        }

        let cf = self.meta_cf()?;
        let key = generation_key(namespace);
        let state = match self.db.get_cf(&cf, key.as_bytes())? {
            Some(raw) => {
                let record: GenerationRecord = serde_cbor::from_slice(&raw)?;
                record.into_state()
            }
            None => {
                let state = GenerationState {
                    generation: 0,
                    created_at: epoch_seconds()?,
                    base_clock: None,
                };
                let record = GenerationRecord::from_state(self.version, &state);
                let encoded = serde_cbor::to_vec(&record)?;
                self.db.put_cf(&cf, key.as_bytes(), encoded)?;
                state
            }
        };

        if let Ok(mut guard) = self.generations.lock() {
            guard.insert(namespace, state.clone());
        }

        Ok(state)
    }

    fn bump_generation(
        &self,
        namespace: CacheNamespace,
        base_clock: Option<LamportTimestamp>,
    ) -> Result<GenerationState> {
        let mut state = self.current_generation(namespace)?;
        state.generation = state.generation.saturating_add(1);
        state.created_at = epoch_seconds()?;
        if base_clock.is_some() {
            state.base_clock = base_clock;
        }
        self.persist_generation(namespace, &state)?;
        Ok(state)
    }

    fn persist_generation(&self, namespace: CacheNamespace, state: &GenerationState) -> Result<()> {
        let cf = self.meta_cf()?;
        let record = GenerationRecord::from_state(self.version, state);
        let encoded = serde_cbor::to_vec(&record)?;
        self.db
            .put_cf(&cf, generation_key(namespace).as_bytes(), encoded)?;
        if let Ok(mut guard) = self.generations.lock() {
            guard.insert(namespace, state.clone());
        }
        Ok(())
    }

    fn append_journal_entry(
        &self,
        namespace: CacheNamespace,
        entity_id: &EntityId,
        state: &GenerationState,
        inserted: &[OperationId],
    ) -> Result<()> {
        let cf = self.journal_cf()?;
        let entry = CacheJournalEntry {
            namespace: namespace.cf_name().to_string(),
            entity_id: entity_id.clone(),
            generation: state.generation,
            inserted: inserted.to_vec(),
            persisted_at: epoch_seconds()?,
            base_clock: state.base_clock.clone(),
        };
        let key = journal_key(namespace, entity_id, state.generation);
        let encoded = serde_cbor::to_vec(&entry)?;
        self.db.put_cf(&cf, key.as_bytes(), encoded)?;
        Ok(())
    }

    fn meta_cf(&self) -> Result<&ColumnFamily> {
        self.db
            .cf_handle(META_CF_NAME)
            .ok_or_else(|| Error::validation("cache metadata column family missing"))
    }

    fn journal_cf(&self) -> Result<&ColumnFamily> {
        self.db
            .cf_handle(JOURNAL_CF_NAME)
            .ok_or_else(|| Error::validation("cache journal column family missing"))
    }
}

impl Drop for PersistentCache {
    fn drop(&mut self) {
        if Arc::strong_count(&self.inner) == 1 {
            self.inner.shutdown.store(true, Ordering::SeqCst);
            if let Ok(mut guard) = self.inner.maintenance_handle.lock() {
                if let Some(handle) = guard.take() {
                    let _ = handle.join();
                }
            }
        }
    }
}

impl CacheRepository for PersistentCache {
    fn get(
        &self,
        namespace: CacheNamespace,
        entity_id: &EntityId,
    ) -> Result<CacheLoadOutcome<EntitySnapshot>> {
        let cf = match self.db().cf_handle(namespace.cf_name()) {
            Some(handle) => handle,
            None => return Ok(CacheLoadOutcome::Miss),
        };

        let key = entity_key(entity_id);
        let raw = match self.db().get_cf(&cf, key.as_bytes())? {
            Some(value) => value,
            None => return Ok(CacheLoadOutcome::Miss),
        };

        match decode_entry(&raw) {
            Ok(entry) => {
                if entry.version != self.inner.version {
                    let _ = self.db().delete_cf(&cf, key.as_bytes());
                    return Ok(CacheLoadOutcome::Stale);
                }

                let generation_state = self.inner.current_generation(namespace)?;
                if entry.generation < generation_state.generation {
                    let _ = self.db().delete_cf(&cf, key.as_bytes());
                    return Ok(CacheLoadOutcome::Stale);
                }

                let now = epoch_seconds()?;
                if now > entry.expires_at {
                    let _ = self.db().delete_cf(&cf, key.as_bytes());
                    return Ok(CacheLoadOutcome::Stale);
                }

                let checksum = crc32fast::hash(&entry.payload);
                if checksum != entry.checksum {
                    let _ = self.db().delete_cf(&cf, key.as_bytes());
                    return Ok(CacheLoadOutcome::Stale);
                }

                let snapshot: EntitySnapshot = serde_cbor::from_slice(&entry.payload)?;
                if snapshot.entity_id != *entity_id {
                    let _ = self.db().delete_cf(&cf, key.as_bytes());
                    return Ok(CacheLoadOutcome::Stale);
                }

                Ok(CacheLoadOutcome::Hit(snapshot))
            }
            Err(_) => {
                let _ = self.db().delete_cf(&cf, key.as_bytes());
                Ok(CacheLoadOutcome::Stale)
            }
        }
    }

    fn put(
        &self,
        namespace: CacheNamespace,
        entity_id: &EntityId,
        snapshot: &EntitySnapshot,
    ) -> Result<()> {
        let cf = match self.db().cf_handle(namespace.cf_name()) {
            Some(handle) => handle,
            None => return Ok(()),
        };

        let ttl = self.policy(namespace).ttl;
        let generation = self.inner.current_generation(namespace)?.generation;
        let payload = serde_cbor::to_vec(snapshot)?;
        let stored_at = epoch_seconds()?;
        let ttl_secs = ttl.as_secs().max(1);
        let entry = StoredEntry {
            version: self.inner.version,
            entity_id: entity_id.clone(),
            clock: snapshot.clock_snapshot.clone(),
            generation,
            stored_at,
            expires_at: stored_at.saturating_add(ttl_secs),
            checksum: crc32fast::hash(&payload),
            payload,
        };

        let encoded = serde_cbor::to_vec(&entry)?;
        let key = entity_key(entity_id);
        self.db().put_cf(&cf, key.as_bytes(), encoded)?;
        Ok(())
    }

    fn invalidate(&self, namespace: CacheNamespace, entity_id: &EntityId) -> Result<()> {
        if let Some(cf) = self.db().cf_handle(namespace.cf_name()) {
            let key = entity_key(entity_id);
            let _ = self.db().delete_cf(&cf, key.as_bytes())?;
        }
        Ok(())
    }

    fn generation(&self, namespace: CacheNamespace) -> Result<Option<CacheGenerationSnapshot>> {
        self.inner
            .current_generation(namespace)
            .map(CacheGenerationSnapshot::from)
            .map(Some)
    }

    fn on_pack_persisted(
        &self,
        namespace: CacheNamespace,
        entity_id: &EntityId,
        inserted: &[OperationId],
        clock: &LamportTimestamp,
    ) -> Result<()> {
        let latest_clock = inserted
            .iter()
            .map(|op| op.timestamp().clone())
            .max()
            .unwrap_or_else(|| clock.clone());

        let generation = self
            .inner
            .bump_generation(namespace, Some(latest_clock.clone()))?;
        self.inner
            .append_journal_entry(namespace, entity_id, &generation, inserted)?;

        // Conservatively invalidate the entity so the next read rebuilds the cache.
        self.invalidate(namespace, entity_id)
    }
}

fn ensure_cache_directory(path: &Path, version: u32) -> Result<()> {
    if path.exists() {
        let version_file = path.join(VERSION_FILE);
        let matches_version = version_file
            .exists()
            .then(|| fs::read_to_string(&version_file))
            .transpose()?
            .and_then(|contents| contents.trim().parse::<u32>().ok())
            .map(|stored| stored == version)
            .unwrap_or(false);

        if !matches_version {
            fs::remove_dir_all(path)?;
        }
    }

    fs::create_dir_all(path)?;
    fs::write(path.join(VERSION_FILE), version.to_string())?;
    Ok(())
}

fn start_maintenance(inner: Arc<PersistentCacheInner>) -> Result<()> {
    let db = Arc::clone(&inner.db);
    let shutdown = Arc::clone(&inner.shutdown);
    let interval = inner.maintenance_interval;
    let version = inner.version;

    let handle = thread::Builder::new()
        .name("git-mile-cache-maint".into())
        .spawn(move || {
            while !shutdown.load(Ordering::Relaxed) {
                thread::sleep(interval);

                let now = match epoch_seconds() {
                    Ok(value) => value,
                    Err(err) => {
                        eprintln!(
                            "warning: failed to read system time for cache maintenance: {err}"
                        );
                        continue;
                    }
                };

                for namespace in CacheNamespace::ALL {
                    let Some(cf) = db.cf_handle(namespace.cf_name()) else {
                        continue;
                    };

                    let mut iter = db.iterator_cf(cf, IteratorMode::Start);

                    while let Some(item) = iter.next() {
                        match item {
                            Ok((key, value)) => {
                                let remove = match decode_entry(&value) {
                                    Ok(entry) => entry.version != version || now > entry.expires_at,
                                    Err(_) => true,
                                };

                                if remove {
                                    let key_bytes = key.as_ref();
                                    if let Err(err) = db.delete_cf(cf, key_bytes) {
                                        eprintln!(
                                            "warning: failed to delete expired cache entry {:?}: {err}",
                                            String::from_utf8_lossy(key_bytes)
                                        );
                                    }
                                }
                            }
                            Err(err) => {
                                eprintln!(
                                    "warning: cache iterator for {} yielded error: {err}",
                                    namespace.cf_name()
                                );
                            }
                        }
                    }
                }
            }
        })?;

    if let Ok(mut guard) = inner.maintenance_handle.lock() {
        *guard = Some(handle);
    }

    Ok(())
}

fn generation_key(namespace: CacheNamespace) -> String {
    format!("{GENERATION_KEY_PREFIX}:{}", namespace.cf_name())
}

fn journal_key(namespace: CacheNamespace, entity_id: &EntityId, generation: u64) -> String {
    format!("{}:{}:{:020}", namespace.cf_name(), entity_id, generation)
}

fn entity_key(entity_id: &EntityId) -> String {
    entity_id.to_string()
}

fn epoch_seconds() -> Result<u64> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| Error::validation("system clock before unix epoch"))?;
    Ok(duration.as_secs())
}

fn decode_entry(bytes: &[u8]) -> serde_cbor::Result<StoredEntry> {
    serde_cbor::from_slice(bytes)
}

#[derive(Serialize, Deserialize)]
struct StoredEntry {
    version: u32,
    entity_id: EntityId,
    clock: LamportTimestamp,
    #[serde(default)]
    generation: u64,
    stored_at: u64,
    expires_at: u64,
    checksum: u32,
    #[serde(with = "serde_bytes")]
    payload: Vec<u8>,
}

/// Tracks cache metrics per adapter.
#[derive(Clone, Default)]
pub struct CacheMetrics {
    inner: Arc<CacheMetricsInner>,
}

#[derive(Default)]
struct CacheMetricsInner {
    hits: AtomicU64,
    misses: AtomicU64,
    stores: AtomicU64,
    evictions: AtomicU64,
    rebuilds: AtomicU64,
}

impl CacheMetrics {
    fn record_hit(&self) {
        self.inner.hits.fetch_add(1, Ordering::Relaxed);
    }

    fn record_miss(&self) {
        self.inner.misses.fetch_add(1, Ordering::Relaxed);
    }

    fn record_store(&self) {
        self.inner.stores.fetch_add(1, Ordering::Relaxed);
    }

    fn record_eviction(&self) {
        self.inner.evictions.fetch_add(1, Ordering::Relaxed);
    }

    fn record_rebuild(&self) {
        self.inner.rebuilds.fetch_add(1, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> CacheMetricsSnapshot {
        CacheMetricsSnapshot {
            hits: self.inner.hits.load(Ordering::Relaxed),
            misses: self.inner.misses.load(Ordering::Relaxed),
            stores: self.inner.stores.load(Ordering::Relaxed),
            evictions: self.inner.evictions.load(Ordering::Relaxed),
            rebuilds: self.inner.rebuilds.load(Ordering::Relaxed),
        }
    }
}

/// Point-in-time view of adapter metrics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CacheMetricsSnapshot {
    pub hits: u64,
    pub misses: u64,
    pub stores: u64,
    pub evictions: u64,
    pub rebuilds: u64,
}

/// Adapter that bridges [`CacheRepository`] implementations to the repository hooks.
pub struct EntityCacheAdapter {
    namespace: CacheNamespace,
    repository: Arc<dyn CacheRepository>,
    metrics: CacheMetrics,
}

impl EntityCacheAdapter {
    pub fn new(repository: Arc<dyn CacheRepository>, namespace: CacheNamespace) -> Self {
        Self {
            namespace,
            repository,
            metrics: CacheMetrics::default(),
        }
    }

    pub fn namespace(&self) -> CacheNamespace {
        self.namespace
    }

    pub fn metrics(&self) -> CacheMetricsSnapshot {
        self.metrics.snapshot()
    }
}

impl RepositoryCacheHook for EntityCacheAdapter {
    fn try_get_snapshot(&self, entity_id: &EntityId) -> Result<Option<EntitySnapshot>> {
        match self.repository.get(self.namespace, entity_id)? {
            CacheLoadOutcome::Hit(snapshot) => {
                self.metrics.record_hit();
                Ok(Some(snapshot))
            }
            CacheLoadOutcome::Miss => {
                self.metrics.record_miss();
                Ok(None)
            }
            CacheLoadOutcome::Stale => {
                self.metrics.record_rebuild();
                self.metrics.record_miss();
                Ok(None)
            }
        }
    }

    fn on_entity_loaded(&self, entity_id: &EntityId, snapshot: &EntitySnapshot) -> Result<()> {
        self.repository.put(self.namespace, entity_id, snapshot)?;
        self.metrics.record_store();
        Ok(())
    }

    fn on_pack_persisted(
        &self,
        entity_id: &EntityId,
        inserted: &[OperationId],
        clock: &LamportTimestamp,
    ) -> Result<()> {
        self.repository
            .on_pack_persisted(self.namespace, entity_id, inserted, clock)?;
        Ok(())
    }

    fn invalidate_entity(&self, entity_id: &EntityId) -> Result<()> {
        self.repository.invalidate(self.namespace, entity_id)?;
        self.metrics.record_eviction();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{LamportClock, LamportTimestamp, ReplicaId};
    use crate::dag::{Operation, OperationBlob, OperationId, OperationMetadata};
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    fn sample_snapshot() -> EntitySnapshot {
        let entity_id = EntityId::new();
        let replica = ReplicaId::new("cache-test");
        let mut clock = LamportClock::new(replica.clone());
        let op_ts = clock.tick().expect("tick clock");
        let op_id = OperationId::new(op_ts.clone());
        let operation = Operation::new(
            op_id.clone(),
            vec![],
            crate::dag::BlobRef::from_bytes(b"payload"),
            OperationMetadata::new("tester", Some("init".to_string())),
        );
        let blob = OperationBlob::from_bytes(b"payload".to_vec());

        EntitySnapshot {
            entity_id,
            clock_snapshot: clock.snapshot(),
            heads: vec![op_id],
            operations: vec![operation],
            blobs: vec![blob],
        }
    }

    #[test]
    fn cache_config_requires_initialized_repo() {
        let temp = tempdir().expect("create temp dir");
        assert!(CacheConfig::for_repo(temp.path()).is_none());

        git2::Repository::init(temp.path()).expect("init repo");

        let config = CacheConfig::for_repo(temp.path());
        assert!(config.is_some());
    }

    #[test]
    fn persistent_cache_roundtrips_snapshot() {
        let temp = tempdir().expect("create temp dir");
        git2::Repository::init(temp.path()).expect("init repo");
        let mut config = CacheConfig::for_repo(temp.path()).expect("config");
        config.maintenance_interval = Duration::from_secs(0);

        let cache = PersistentCache::open(config).expect("open cache");
        let snapshot = sample_snapshot();
        let entity_id = snapshot.entity_id.clone();

        cache
            .put(CacheNamespace::Issues, &entity_id, &snapshot)
            .expect("put snapshot");

        match cache
            .get(CacheNamespace::Issues, &entity_id)
            .expect("load snapshot")
        {
            CacheLoadOutcome::Hit(loaded) => assert_eq!(loaded, snapshot),
            CacheLoadOutcome::Miss => panic!("expected cache hit, but entry missing"),
            CacheLoadOutcome::Stale => panic!("expected cache hit, but entry stale"),
        }
    }

    #[test]
    fn ttl_expiry_causes_stale_result() {
        let temp = tempdir().expect("create temp dir");
        git2::Repository::init(temp.path()).expect("init repo");
        let mut config = CacheConfig::for_repo(temp.path()).expect("config");
        config.maintenance_interval = Duration::from_secs(0);
        config.policies.insert(
            CacheNamespace::Issues,
            CachePolicy {
                ttl: Duration::from_secs(1),
            },
        );

        let cache = PersistentCache::open(config).expect("open cache");
        let snapshot = sample_snapshot();
        let entity_id = snapshot.entity_id.clone();
        cache
            .put(CacheNamespace::Issues, &entity_id, &snapshot)
            .expect("put snapshot");

        thread::sleep(Duration::from_secs(2));

        match cache
            .get(CacheNamespace::Issues, &entity_id)
            .expect("load snapshot")
        {
            CacheLoadOutcome::Stale | CacheLoadOutcome::Miss => {}
            CacheLoadOutcome::Hit(_) => panic!("expected cache miss after ttl"),
        }
    }

    #[test]
    fn generation_advances_on_pack_persisted() {
        let temp = tempdir().expect("create temp dir");
        git2::Repository::init(temp.path()).expect("init repo");
        let mut config = CacheConfig::for_repo(temp.path()).expect("config");
        config.maintenance_interval = Duration::from_secs(0);

        let cache = PersistentCache::open(config).expect("open cache");
        let snapshot = sample_snapshot();
        let entity_id = snapshot.entity_id.clone();

        cache
            .put(CacheNamespace::Issues, &entity_id, &snapshot)
            .expect("put snapshot");

        let before = cache
            .generation(CacheNamespace::Issues)
            .expect("generation before");
        assert_eq!(before.generation, 0);

        match cache
            .get(CacheNamespace::Issues, &entity_id)
            .expect("load snapshot")
        {
            CacheLoadOutcome::Hit(_) => {}
            other => panic!("expected cache hit, got {other:?}"),
        }

        let updated_clock = LamportTimestamp::new(42, ReplicaId::new("gen"));
        let op_id = OperationId::new(updated_clock.clone());
        cache
            .on_pack_persisted(
                CacheNamespace::Issues,
                &entity_id,
                &[op_id.clone()],
                &updated_clock,
            )
            .expect("on pack persisted");

        let after = cache
            .generation(CacheNamespace::Issues)
            .expect("generation after");
        assert!(after.generation > before.generation);
        assert_eq!(after.base_clock, Some(updated_clock.clone()));

        match cache
            .get(CacheNamespace::Issues, &entity_id)
            .expect("load snapshot after persist")
        {
            CacheLoadOutcome::Stale | CacheLoadOutcome::Miss => {}
            other => panic!("expected stale or miss entry, got {other:?}"),
        }

        let journal_cf = cache
            .inner
            .db
            .cf_handle(JOURNAL_CF_NAME)
            .expect("journal cf");
        let mut found = false;
        for record in cache.inner.db.iterator_cf(journal_cf, IteratorMode::Start) {
            let (_key, value) = record.expect("journal entry");
            let entry: CacheJournalEntry =
                serde_cbor::from_slice(&value).expect("decode journal entry");
            if entry.entity_id == entity_id {
                found = true;
                assert_eq!(entry.generation, after.generation);
                assert_eq!(entry.inserted.len(), 1);
                assert_eq!(entry.base_clock, Some(updated_clock.clone()));
                break;
            }
        }
        assert!(found, "expected journal entry for entity");
    }
}
