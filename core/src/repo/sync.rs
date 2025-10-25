use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use async_trait::async_trait;
use tokio::runtime::Builder;
use tokio::sync::{mpsc, oneshot};
use tokio::time::sleep;

use crate::dag::{EntityId, OperationId};
use crate::error::{Error, Result};
use crate::repo::{CacheGenerationSnapshot, CacheNamespace};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SyncPhase {
    Prepare,
    Fetch,
    Apply,
    Finalize,
}

#[derive(Clone, Debug)]
pub struct SyncContext {
    pub repo_path: PathBuf,
    pub phase: SyncPhase,
    pub generation: Option<CacheGenerationSnapshot>,
}

impl SyncContext {
    pub fn new(
        repo_path: impl AsRef<Path>,
        phase: SyncPhase,
        generation: Option<CacheGenerationSnapshot>,
    ) -> Self {
        Self {
            repo_path: repo_path.as_ref().to_path_buf(),
            phase,
            generation,
        }
    }
}

#[async_trait]
pub trait SyncHook: Send + Sync {
    async fn run(&self, ctx: &SyncContext) -> Result<()>;
}

pub struct SyncHookRegistry {
    hooks: Mutex<HashMap<SyncPhase, Vec<Arc<dyn SyncHook>>>>,
}

impl SyncHookRegistry {
    pub fn new() -> Self {
        Self {
            hooks: Mutex::new(HashMap::new()),
        }
    }

    pub fn register(&self, phase: SyncPhase, hook: Arc<dyn SyncHook>) {
        let mut guard = self.hooks.lock().expect("sync hook registry poisoned");
        guard.entry(phase).or_default().push(hook);
    }

    pub async fn dispatch(&self, ctx: &SyncContext) -> Result<()> {
        let hooks = {
            let guard = self.hooks.lock().expect("sync hook registry poisoned");
            guard.get(&ctx.phase).cloned().unwrap_or_default()
        };

        for hook in hooks {
            hook.run(ctx).await?;
        }

        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncTaskStatus {
    Pending,
    Applied,
    Failed,
}

#[derive(Clone, Debug)]
pub struct IndexDelta {
    pub namespace: CacheNamespace,
    pub entity_id: EntityId,
    pub generation: u64,
    pub operations: Vec<OperationId>,
}

impl IndexDelta {
    pub fn new(
        namespace: CacheNamespace,
        entity_id: EntityId,
        generation: u64,
        operations: Vec<OperationId>,
    ) -> Self {
        Self {
            namespace,
            entity_id,
            generation,
            operations,
        }
    }

    fn key(&self) -> String {
        format!(
            "{:?}:{}:{}",
            self.namespace, self.entity_id, self.generation
        )
    }
}

struct SyncEnvelope {
    delta: IndexDelta,
    respond_to: Option<oneshot::Sender<Result<()>>>,
}

enum SyncCommand {
    Apply(SyncEnvelope),
}

pub struct BackgroundSyncWorker {
    inner: Arc<BackgroundSyncWorkerInner>,
}

struct BackgroundSyncWorkerInner {
    sender: mpsc::Sender<SyncCommand>,
    queue_depth: AtomicUsize,
    statuses: Mutex<HashMap<String, SyncTaskStatus>>,
    shutdown: Mutex<Option<oneshot::Sender<()>>>,
    handle: Mutex<Option<thread::JoinHandle<()>>>,
}

impl BackgroundSyncWorker {
    pub fn spawn(buffer: usize) -> Result<Self> {
        let (sender, receiver) = mpsc::channel(buffer);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        let inner = Arc::new(BackgroundSyncWorkerInner {
            sender: sender.clone(),
            queue_depth: AtomicUsize::new(0),
            statuses: Mutex::new(HashMap::new()),
            shutdown: Mutex::new(Some(shutdown_tx)),
            handle: Mutex::new(None),
        });

        let worker_inner = Arc::clone(&inner);
        let runtime = Builder::new_multi_thread().enable_all().build()?;

        let handle = thread::Builder::new()
            .name("git-mile-sync-worker".into())
            .spawn(move || {
                runtime.block_on(async move {
                    worker_inner.run(receiver, shutdown_rx).await;
                });
            })?;

        *inner.handle.lock().expect("sync worker handle poisoned") = Some(handle);

        Ok(Self { inner })
    }

    pub fn enqueue_delta(&self, delta: IndexDelta) -> Result<oneshot::Receiver<Result<()>>> {
        let key = delta.key();
        let namespace_label = delta.namespace.as_str();
        {
            let mut statuses = self
                .inner
                .statuses
                .lock()
                .expect("sync status map poisoned");
            statuses.insert(key, SyncTaskStatus::Pending);
        }

        self.inner.queue_depth.fetch_add(1, Ordering::SeqCst);
        metrics::gauge!("sync.queue_depth", "namespace" => namespace_label)
            .set(self.queue_depth() as f64);

        let (tx, rx) = oneshot::channel();
        let envelope = SyncEnvelope {
            delta,
            respond_to: Some(tx),
        };

        if let Err(err) = self.inner.sender.try_send(SyncCommand::Apply(envelope)) {
            self.inner.queue_depth.fetch_sub(1, Ordering::SeqCst);
            metrics::gauge!("sync.queue_depth", "namespace" => namespace_label)
                .set(self.queue_depth() as f64);
            return Err(Error::validation(format!(
                "failed to enqueue sync work: {err}"
            )));
        }

        Ok(rx)
    }

    pub fn queue_depth(&self) -> usize {
        self.inner.queue_depth.load(Ordering::SeqCst)
    }

    pub fn status_snapshot(&self) -> HashMap<String, SyncTaskStatus> {
        self.inner
            .statuses
            .lock()
            .expect("sync status map poisoned")
            .clone()
    }
}

impl BackgroundSyncWorkerInner {
    async fn run(
        self: Arc<Self>,
        mut receiver: mpsc::Receiver<SyncCommand>,
        mut shutdown: oneshot::Receiver<()>,
    ) {
        loop {
            tokio::select! {
                _ = &mut shutdown => {
                    break;
                }
                maybe_command = receiver.recv() => {
                    match maybe_command {
                        Some(SyncCommand::Apply(envelope)) => {
                            let key = envelope.delta.key();
                            let result = process_delta(&envelope.delta).await;
                            self.queue_depth.fetch_sub(1, Ordering::SeqCst);
                            metrics::gauge!(
                                "sync.queue_depth",
                                "namespace" => envelope.delta.namespace.as_str()
                            )
                            .set(self.queue_depth.load(Ordering::SeqCst) as f64);
                            {
                                let mut statuses = self.statuses.lock().expect("sync status map poisoned");
                                statuses.insert(
                                    key.clone(),
                                    if result.is_ok() {
                                        SyncTaskStatus::Applied
                                    } else {
                                        SyncTaskStatus::Failed
                                    },
                                );
                            }
                            metrics::counter!(
                                "sync.delta_processed",
                                "namespace" => envelope.delta.namespace.as_str(),
                                "status" => if result.is_ok() { "applied" } else { "failed" }
                            )
                            .increment(1);
                            if let Some(respond_to) = envelope.respond_to {
                                let _ = respond_to.send(result);
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    }
}

async fn process_delta(delta: &IndexDelta) -> Result<()> {
    if delta.operations.is_empty() {
        return Err(Error::validation("index delta did not include operations"));
    }

    sleep(Duration::from_millis(5)).await;
    Ok(())
}

impl Drop for BackgroundSyncWorkerInner {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.lock().expect("sync shutdown poisoned").take() {
            let _ = shutdown.send(());
        }

        if let Some(handle) = self.handle.lock().expect("sync handle poisoned").take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clock::{LamportTimestamp, ReplicaId};

    struct RecordingHook {
        events: Arc<Mutex<Vec<SyncPhase>>>,
    }

    #[async_trait]
    impl SyncHook for RecordingHook {
        async fn run(&self, ctx: &SyncContext) -> Result<()> {
            let mut events = self.events.lock().expect("events lock poisoned");
            events.push(ctx.phase);
            Ok(())
        }
    }

    #[tokio::test(flavor = "current_thread")]
    async fn registry_invokes_hooks_in_order() {
        let registry = SyncHookRegistry::new();
        let events = Arc::new(Mutex::new(Vec::new()));
        let first: Arc<dyn SyncHook> = Arc::new(RecordingHook {
            events: Arc::clone(&events),
        });
        let second: Arc<dyn SyncHook> = Arc::new(RecordingHook {
            events: Arc::clone(&events),
        });

        registry.register(SyncPhase::Prepare, first);
        registry.register(SyncPhase::Prepare, second);

        let ctx = SyncContext::new("/tmp/repo", SyncPhase::Prepare, None);
        registry.dispatch(&ctx).await.expect("dispatch hooks");

        let recorded = events.lock().expect("events lock poisoned");
        assert_eq!(
            recorded.as_slice(),
            &[SyncPhase::Prepare, SyncPhase::Prepare]
        );
    }

    fn sample_operation(replica: &str) -> OperationId {
        let timestamp = LamportTimestamp::new(1, ReplicaId::new(replica));
        OperationId::new(timestamp)
    }

    #[test]
    fn background_worker_processes_deltas() {
        let worker = BackgroundSyncWorker::spawn(16).expect("spawn worker");
        let delta = IndexDelta::new(
            CacheNamespace::Issues,
            EntityId::new(),
            1,
            vec![sample_operation("worker")],
        );

        let rx = worker.enqueue_delta(delta).expect("enqueue delta");

        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async { rx.await.expect("receive result").expect("delta applied") });

        assert_eq!(worker.queue_depth(), 0);
        let statuses = worker.status_snapshot();
        assert!(
            statuses
                .values()
                .all(|status| matches!(status, SyncTaskStatus::Applied))
        );
    }

    #[test]
    fn background_worker_marks_failures() {
        let worker = BackgroundSyncWorker::spawn(4).expect("spawn worker");
        let delta = IndexDelta::new(CacheNamespace::Milestones, EntityId::new(), 2, Vec::new());

        let rx = worker.enqueue_delta(delta).expect("enqueue delta");
        let runtime = Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let result = rx.await.expect("receive result");
            assert!(result.is_err());
        });

        let statuses = worker.status_snapshot();
        assert!(
            statuses
                .values()
                .any(|status| matches!(status, SyncTaskStatus::Failed))
        );
    }
}
