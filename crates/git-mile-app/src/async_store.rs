//! Async storage abstraction for MCP integration.

use anyhow::{Error, Result, anyhow};
use git_mile_core::event::Event;
use git_mile_core::id::TaskId;
use git_mile_core::{TaskFilter, TaskSnapshot};
use git_mile_store_git::{GitStore, GitStoreError};
use git2::Oid;
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::Mutex;

use crate::task_cache::{TaskCache, TaskView};

/// Async storage trait for use with `tokio::sync::Mutex`.
///
/// This trait mirrors [`crate::task_writer::TaskStore`] but with async methods,
/// allowing integration with async contexts like MCP servers.
#[allow(async_fn_in_trait)]
pub trait AsyncTaskStore: Send + Sync {
    /// Error type bubbled up from the backing store.
    type Error: Into<Error> + Send;

    /// Check if a task exists without loading its events.
    ///
    /// # Errors
    /// Returns a store-specific error when the check fails.
    async fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error>;

    /// Append a single event for the target task.
    ///
    /// # Errors
    /// Returns a store-specific error when persisting the event fails.
    async fn append_event(&self, event: &Event) -> Result<Oid, Self::Error>;

    /// Load every event for the given task.
    ///
    /// # Errors
    /// Returns a store-specific error when the task cannot be read.
    async fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error>;

    /// Enumerate all known task identifiers.
    ///
    /// # Errors
    /// Returns a store-specific error when listing fails.
    async fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error>;

    /// List task IDs that have been modified since the given timestamp.
    ///
    /// # Errors
    /// Returns a store-specific error when the query fails.
    async fn list_tasks_modified_since(
        &self,
        since: time::OffsetDateTime,
    ) -> Result<Vec<TaskId>, Self::Error>;
}

impl AsyncTaskStore for Arc<Mutex<GitStore>> {
    type Error = GitStoreError;

    async fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error> {
        let guard = self.lock().await;
        // Clone the store to avoid holding the lock during blocking I/O
        let store = guard.clone();
        drop(guard);

        tokio::task::spawn_blocking(move || store.task_exists(task))
            .await
            .map_err(|e| GitStoreError::Other(format!("Task join error: {e}")))?
            .map_err(GitStoreError::from)
    }

    async fn append_event(&self, event: &Event) -> Result<Oid, Self::Error> {
        let guard = self.lock().await;
        // Clone the store to avoid holding the lock during blocking I/O
        let store = guard.clone();
        drop(guard);

        let event = event.clone();
        tokio::task::spawn_blocking(move || store.append_event(&event))
            .await
            .map_err(|e| GitStoreError::Other(format!("Task join error: {e}")))?
            .map_err(GitStoreError::from)
    }

    async fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
        let guard = self.lock().await;
        // Clone the store to avoid holding the lock during blocking I/O
        let store = guard.clone();
        drop(guard);

        tokio::task::spawn_blocking(move || store.load_events(task))
            .await
            .map_err(|e| GitStoreError::Other(format!("Task join error: {e}")))?
            .map_err(GitStoreError::from)
    }

    async fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
        let guard = self.lock().await;
        // Clone the store to avoid holding the lock during blocking I/O
        let store = guard.clone();
        drop(guard);

        tokio::task::spawn_blocking(move || store.list_tasks())
            .await
            .map_err(|e| GitStoreError::Other(format!("Task join error: {e}")))?
            .map_err(GitStoreError::from)
    }

    async fn list_tasks_modified_since(
        &self,
        since: time::OffsetDateTime,
    ) -> Result<Vec<TaskId>, Self::Error> {
        let guard = self.lock().await;
        // Clone the store to avoid holding the lock during blocking I/O
        let store = guard.clone();
        drop(guard);

        tokio::task::spawn_blocking(move || store.list_tasks_modified_since(since))
            .await
            .map_err(|e| GitStoreError::Other(format!("Task join error: {e}")))?
            .map_err(GitStoreError::from)
    }
}

/// Async repository that caches task snapshots for MCP integration.
///
/// This is the async counterpart to [`crate::task_repository::TaskRepository`].
pub struct AsyncTaskRepository<S> {
    store: S,
    cache: Arc<Mutex<CacheState>>,
}

struct CacheState {
    cache: TaskCache,
    last_refresh: Option<OffsetDateTime>,
}

impl<S: AsyncTaskStore> AsyncTaskRepository<S> {
    /// Create a new async repository.
    pub fn new(store: S) -> Self {
        Self {
            store,
            cache: Arc::new(Mutex::new(CacheState {
                cache: TaskCache::default(),
                last_refresh: None,
            })),
        }
    }

    /// Refresh the cache if stale.
    async fn refresh_if_stale(&self) -> Result<()> {
        enum RefreshPlan {
            Full,
            Incremental(OffsetDateTime),
        }

        let plan = {
            let state = self.cache.lock().await;
            state
                .last_refresh
                .map_or(RefreshPlan::Full, RefreshPlan::Incremental)
        };

        match plan {
            RefreshPlan::Full => {
                let cache = self.load_cache().await?;
                let mut state = self.cache.lock().await;
                let latest_ts = cache
                    .tasks
                    .first()
                    .and_then(|view| view.last_updated)
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH);
                state.cache = cache;
                state.last_refresh = Some(latest_ts);
            }
            RefreshPlan::Incremental(last_refresh) => {
                let modified = self.list_modified_since(last_refresh).await?;
                if modified.is_empty() {
                    return Ok(());
                }
                let updated_views = self.load_task_views(&modified).await?;
                let latest_seen = updated_views
                    .iter()
                    .filter_map(|view| view.last_updated)
                    .max()
                    .unwrap_or(last_refresh);
                let mut state = self.cache.lock().await;
                state.cache.upsert_views(updated_views);
                let previous = state.last_refresh.unwrap_or(last_refresh);
                state.last_refresh = Some(previous.max(latest_seen));
            }
        }

        Ok(())
    }

    async fn list_modified_since(&self, since: OffsetDateTime) -> Result<Vec<TaskId>> {
        self.store
            .list_tasks_modified_since(since)
            .await
            .map_err(|e| anyhow!("Failed to list modified tasks: {}", e.into()))
    }

    /// Load all tasks and build a `TaskCache`.
    async fn load_cache(&self) -> Result<TaskCache> {
        let task_ids = self
            .store
            .list_tasks()
            .await
            .map_err(|e| anyhow!("Failed to list tasks: {}", e.into()))?;

        let views = self.load_task_views(&task_ids).await?;
        Ok(TaskCache::from_views(views))
    }

    async fn load_task_views(&self, task_ids: &[TaskId]) -> Result<Vec<TaskView>> {
        let mut views = Vec::with_capacity(task_ids.len());
        for &task_id in task_ids {
            let events = self
                .store
                .load_events(task_id)
                .await
                .map_err(|e| anyhow!("Failed to load events for task {}: {}", task_id, e.into()))?;
            views.push(TaskView::from_events(&events));
        }
        Ok(views)
    }

    /// List task snapshots with optional filter.
    ///
    /// # Errors
    /// Returns an error if the cache cannot be refreshed or task loading fails.
    pub async fn list_snapshots(&self, filter: Option<&TaskFilter>) -> Result<Vec<TaskSnapshot>> {
        self.refresh_if_stale().await?;
        let state = self.cache.lock().await;
        Ok(filter.map_or_else(
            || state.cache.snapshots().cloned().collect(),
            |f| state.cache.filtered_snapshots(f),
        ))
    }

    /// Get a clone of the current cache.
    ///
    /// # Errors
    /// Returns an error if the cache cannot be refreshed or task loading fails.
    pub async fn get_cache(&self) -> Result<TaskCache> {
        self.refresh_if_stale().await?;
        let state = self.cache.lock().await;
        Ok(state.cache.clone())
    }

    /// Fetch a snapshot for a specific task.
    ///
    /// # Errors
    /// Returns an error if refreshing the cache fails or the task is missing.
    pub async fn get_snapshot(&self, task_id: TaskId) -> Result<TaskSnapshot> {
        self.get_view(task_id).await.map(|view| view.snapshot)
    }

    /// Fetch a [`TaskView`] (snapshot + comments) for a specific task.
    ///
    /// # Errors
    /// Returns an error if refreshing the cache fails or the task is missing.
    pub async fn get_view(&self, task_id: TaskId) -> Result<TaskView> {
        self.refresh_if_stale().await?;
        let state = self.cache.lock().await;
        state
            .cache
            .view(task_id)
            .ok_or_else(|| anyhow!("Task not found: {task_id}"))
    }

    /// List children for a task.
    ///
    /// # Errors
    /// Returns an error if refreshing the cache fails.
    pub async fn list_children(&self, task_id: TaskId) -> Result<Vec<TaskId>> {
        self.refresh_if_stale().await?;
        let state = self.cache.lock().await;
        if !state.cache.task_index.contains_key(&task_id) {
            return Err(anyhow!("Task not found: {task_id}"));
        }
        Ok(state.cache.children_of(task_id))
    }

    /// List parents for a task.
    ///
    /// # Errors
    /// Returns an error if refreshing the cache fails.
    pub async fn list_parents(&self, task_id: TaskId) -> Result<Vec<TaskId>> {
        self.refresh_if_stale().await?;
        let state = self.cache.lock().await;
        if !state.cache.task_index.contains_key(&task_id) {
            return Err(anyhow!("Task not found: {task_id}"));
        }
        Ok(state.cache.parents_of(task_id))
    }
}
