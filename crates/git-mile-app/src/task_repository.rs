//! Task repository with caching for efficient snapshot access.

use anyhow::{Result, anyhow};
use git_mile_core::{TaskFilter, TaskSnapshot, id::TaskId};
use std::sync::{Arc, RwLock};
use time::OffsetDateTime;

use crate::task_cache::TaskCache;
use crate::task_writer::TaskStore;

/// Repository that caches task snapshots and provides efficient access.
pub struct TaskRepository<S> {
    store: Arc<S>,
    cache: Arc<RwLock<CacheState>>,
}

struct CacheState {
    cache: TaskCache,
    last_refresh: Option<OffsetDateTime>,
}

impl<S: TaskStore> TaskRepository<S> {
    /// Create a new repository wrapping the given store.
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            cache: Arc::new(RwLock::new(CacheState {
                cache: TaskCache::default(),
                last_refresh: None,
            })),
        }
    }

    /// Refresh the cache if it's stale.
    ///
    /// # Errors
    /// Returns an error if loading tasks from the store fails.
    pub fn refresh_if_stale(&self) -> Result<()> {
        let mut state = self.cache.write().map_err(|_| anyhow!("Failed to lock cache"))?;

        // For now, always do a full reload using TaskCache::load
        // TODO: Implement differential updates in the future
        let loaded_cache = TaskCache::load(&*self.store).map_err(Into::into)?;
        state.cache = loaded_cache;
        state.last_refresh = Some(OffsetDateTime::now_utc());
        drop(state);
        Ok(())
    }

    /// List all task snapshots, optionally filtered.
    ///
    /// # Errors
    /// Returns an error if refreshing the cache fails.
    pub fn list_snapshots(&self, filter: Option<&TaskFilter>) -> Result<Vec<TaskSnapshot>> {
        self.refresh_if_stale()?;

        let state = self.cache.read().map_err(|_| anyhow!("Failed to lock cache"))?;

        Ok(filter.map_or_else(
            || state.cache.snapshots().cloned().collect(),
            |f| state.cache.filtered_snapshots(f),
        ))
    }

    /// Get a single task snapshot by ID.
    ///
    /// # Errors
    /// Returns an error if the task is not found or refreshing fails.
    pub fn get_snapshot(&self, task_id: TaskId) -> Result<TaskSnapshot> {
        self.refresh_if_stale()?;

        let state = self.cache.read().map_err(|_| anyhow!("Failed to lock cache"))?;

        state
            .cache
            .task_index
            .get(&task_id)
            .and_then(|&idx| state.cache.tasks.get(idx))
            .map(|view| view.snapshot.clone())
            .ok_or_else(|| anyhow!("Task not found: {task_id}"))
    }

    /// Clear the cache, forcing a full reload on next access.
    ///
    /// # Errors
    /// Returns an error if the cache lock cannot be acquired.
    pub fn clear_cache(&self) -> Result<()> {
        let mut state = self.cache.write().map_err(|_| anyhow!("Failed to lock cache"))?;

        state.cache = TaskCache::default();
        state.last_refresh = None;
        drop(state);
        Ok(())
    }

    /// Get a clone of the current task cache.
    ///
    /// # Errors
    /// Returns an error if refreshing the cache fails or the lock cannot be acquired.
    pub fn get_cache(&self) -> Result<TaskCache> {
        self.refresh_if_stale()?;

        let state = self.cache.read().map_err(|_| anyhow!("Failed to lock cache"))?;

        Ok(state.cache.clone())
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use git_mile_core::event::{Actor, Event, EventKind};
    use git_mile_store_git::GitStore;
    use tempfile::TempDir;

    fn setup_test_repo() -> (TempDir, TaskRepository<GitStore>) {
        let temp_dir = TempDir::new().expect("create temp dir");
        let repo_path = temp_dir.path();
        git2::Repository::init(repo_path).expect("init git repo");

        let store = Arc::new(GitStore::open(repo_path).expect("open git store"));
        let repo = TaskRepository::new(store);

        (temp_dir, repo)
    }

    #[test]
    fn repository_initial_load_empty() {
        let (_dir, repo) = setup_test_repo();
        let snapshots = repo.list_snapshots(None).expect("list snapshots");
        assert_eq!(snapshots.len(), 0);
    }

    #[test]
    fn repository_loads_tasks() {
        let (_dir, repo) = setup_test_repo();

        // Create a task
        let task_id = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };
        let event = Event::new(
            task_id,
            &actor,
            EventKind::TaskCreated {
                title: "Test Task".into(),
                labels: vec![],
                assignees: vec![],
                description: None,
                state: None,
                state_kind: None,
            },
        );

        repo.store.append_event(&event).expect("append event");

        // List snapshots
        let snapshots = repo.list_snapshots(None).expect("list snapshots");
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].title, "Test Task");
    }

    #[test]
    fn repository_cache_reuse() {
        let (_dir, repo) = setup_test_repo();

        // First load
        let snapshots1 = repo.list_snapshots(None).expect("list snapshots");
        assert_eq!(snapshots1.len(), 0);

        // Second load (from cache)
        let snapshots2 = repo.list_snapshots(None).expect("list snapshots");
        assert_eq!(snapshots2.len(), 0);

        // Cache should be reused (last_refresh is set)
        assert!(repo.cache.read().expect("read cache").last_refresh.is_some());
    }
}
