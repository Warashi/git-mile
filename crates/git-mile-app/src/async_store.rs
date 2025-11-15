//! Async storage abstraction for MCP integration.

use anyhow::{anyhow, Error, Result};
use git2::Oid;
use git_mile_core::event::Event;
use git_mile_core::id::TaskId;
use git_mile_core::{TaskFilter, TaskSnapshot};
use git_mile_store_git::{GitStore, GitStoreError};
use std::sync::Arc;
use time::OffsetDateTime;
use tokio::sync::Mutex;

use crate::task_cache::TaskCache;

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
        let mut state = self.cache.lock().await;

        // For now, always do a full reload
        let loaded_cache = self.load_cache().await?;

        state.cache = loaded_cache;
        state.last_refresh = Some(OffsetDateTime::now_utc());
        drop(state);
        Ok(())
    }

    /// Load all tasks and build a `TaskCache`.
    async fn load_cache(&self) -> Result<TaskCache> {
        use crate::task_cache::TaskView;
        use std::cmp::Ordering;

        // List all task IDs
        let task_ids = self
            .store
            .list_tasks()
            .await
            .map_err(|e| anyhow!("Failed to list tasks: {}", e.into()))?;

        // Load events for each task and build TaskView
        let mut views = Vec::new();
        for task_id in task_ids {
            let events = self
                .store
                .load_events(task_id)
                .await
                .map_err(|e| anyhow!("Failed to load events for task {}: {}", task_id, e.into()))?;
            views.push(TaskView::from_events(&events));
        }

        // Sort by last_updated descending
        views.sort_by(|a, b| match (a.last_updated, b.last_updated) {
            (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.snapshot.id.cmp(&b.snapshot.id),
        });

        // Build cache with indexes
        Ok(TaskCache::from_views(views))
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
}
