//! Task repository with caching for efficient snapshot access.

use anyhow::{Context, Result, anyhow};
use git_mile_core::{TaskFilter, TaskSnapshot, id::TaskId};
use std::sync::{Arc, RwLock};
use time::OffsetDateTime;

use crate::task_cache::{TaskCache, TaskView};
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
        enum RefreshPlan {
            Full,
            Incremental(OffsetDateTime),
        }

        let plan = {
            let state = self.cache.read().map_err(|_| anyhow!("Failed to lock cache"))?;
            state
                .last_refresh
                .map_or(RefreshPlan::Full, RefreshPlan::Incremental)
        };

        match plan {
            RefreshPlan::Full => {
                let cache = TaskCache::load(&*self.store).map_err(Into::into)?;
                let mut state = self.cache.write().map_err(|_| anyhow!("Failed to lock cache"))?;
                let latest_ts = cache
                    .tasks
                    .first()
                    .and_then(|view| view.last_updated)
                    .unwrap_or(OffsetDateTime::UNIX_EPOCH);
                state.cache = cache;
                state.last_refresh = Some(latest_ts);
            }
            RefreshPlan::Incremental(last_refresh) => {
                let modified = self
                    .store
                    .list_tasks_modified_since(last_refresh)
                    .map_err(Into::into)?;
                if modified.is_empty() {
                    return Ok(());
                }
                let updated_views = self.load_task_views(&modified)?;
                let latest_seen = updated_views
                    .iter()
                    .filter_map(|view| view.last_updated)
                    .max()
                    .unwrap_or(last_refresh);
                let mut state = self.cache.write().map_err(|_| anyhow!("Failed to lock cache"))?;
                state.cache.upsert_views(updated_views);
                let previous = state.last_refresh.unwrap_or(last_refresh);
                state.last_refresh = Some(previous.max(latest_seen));
            }
        }
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

    /// Get the full [`TaskView`] for a task.
    ///
    /// # Errors
    /// Returns an error if refreshing fails or the task is missing.
    pub fn get_view(&self, task_id: TaskId) -> Result<TaskView> {
        self.refresh_if_stale()?;

        let state = self.cache.read().map_err(|_| anyhow!("Failed to lock cache"))?;
        state
            .cache
            .view(task_id)
            .ok_or_else(|| anyhow!("Task not found: {task_id}"))
    }

    /// List child task ids for the given parent.
    ///
    /// # Errors
    /// Returns an error if refreshing the cache fails.
    pub fn list_children(&self, task_id: TaskId) -> Result<Vec<TaskId>> {
        self.refresh_if_stale()?;
        let state = self.cache.read().map_err(|_| anyhow!("Failed to lock cache"))?;
        Ok(state.cache.children_of(task_id))
    }

    /// List parent task ids for the given child.
    ///
    /// # Errors
    /// Returns an error if refreshing the cache fails.
    pub fn list_parents(&self, task_id: TaskId) -> Result<Vec<TaskId>> {
        self.refresh_if_stale()?;
        let state = self.cache.read().map_err(|_| anyhow!("Failed to lock cache"))?;
        Ok(state.cache.parents_of(task_id))
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

    fn load_task_views(&self, task_ids: &[TaskId]) -> Result<Vec<TaskView>> {
        let mut views = Vec::with_capacity(task_ids.len());
        for &task_id in task_ids {
            let events = self
                .store
                .load_events(task_id)
                .map_err(Into::into)
                .with_context(|| format!("Failed to load events for task {task_id}"))?;
            views.push(TaskView::from_events(&events));
        }
        Ok(views)
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::arc_with_non_send_sync)]
mod tests {
    use super::*;
    use git_mile_core::event::{Actor, Event, EventKind};
    use git_mile_store_git::GitStore;
    use git2::Oid;
    use std::collections::HashMap;
    use std::sync::Mutex;
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

    #[derive(Default)]
    struct MockStore {
        inner: Mutex<MockStoreInner>,
    }

    #[derive(Default)]
    struct MockStoreInner {
        tasks: Vec<TaskId>,
        events: HashMap<TaskId, Vec<Event>>,
        list_tasks_calls: usize,
        list_modified_calls: usize,
        load_events_calls: HashMap<TaskId, usize>,
    }

    impl MockStore {
        fn with_tasks(tasks: Vec<(TaskId, Vec<Event>)>) -> Self {
            let mut inner = MockStoreInner::default();
            for (task, events) in tasks {
                inner.tasks.push(task);
                inner.events.insert(task, events);
            }
            Self {
                inner: Mutex::new(inner),
            }
        }

        fn replace_events(&self, task: TaskId, events: Vec<Event>) {
            let mut inner = self.inner.lock().expect("lock store");
            if !inner.tasks.contains(&task) {
                inner.tasks.push(task);
            }
            inner.events.insert(task, events);
        }

        fn list_tasks_calls(&self) -> usize {
            self.inner.lock().expect("lock store").list_tasks_calls
        }

        fn list_modified_calls(&self) -> usize {
            self.inner.lock().expect("lock store").list_modified_calls
        }

        fn load_events_calls(&self, task: TaskId) -> usize {
            self.inner
                .lock()
                .expect("lock store")
                .load_events_calls
                .get(&task)
                .copied()
                .unwrap_or(0)
        }
    }

    impl TaskStore for MockStore {
        type Error = anyhow::Error;

        fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error> {
            Ok(self.inner.lock().expect("lock store").events.contains_key(&task))
        }

        fn append_event(&self, _event: &Event) -> Result<Oid, Self::Error> {
            unreachable!("append_event is not used in MockStore tests");
        }

        fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
            let mut inner = self.inner.lock().expect("lock store");
            *inner.load_events_calls.entry(task).or_default() += 1;
            Ok(inner.events.get(&task).cloned().unwrap_or_default())
        }

        fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
            let mut inner = self.inner.lock().expect("lock store");
            inner.list_tasks_calls += 1;
            Ok(inner.tasks.clone())
        }

        fn list_tasks_modified_since(&self, since: OffsetDateTime) -> Result<Vec<TaskId>, Self::Error> {
            let mut inner = self.inner.lock().expect("lock store");
            inner.list_modified_calls += 1;
            let modified = inner
                .events
                .iter()
                .filter_map(|(task, events)| {
                    events
                        .iter()
                        .map(|ev| ev.ts)
                        .max()
                        .filter(|&ts| ts >= since)
                        .map(|_| *task)
                })
                .collect();
            drop(inner);
            Ok(modified)
        }
    }

    fn mock_event(task_id: TaskId, title: &str, ts: i64) -> Event {
        let mut ev = Event::new(
            task_id,
            &Actor {
                name: "tester".into(),
                email: "tester@example.invalid".into(),
            },
            EventKind::TaskCreated {
                title: title.into(),
                labels: vec![],
                assignees: vec![],
                description: None,
                state: None,
                state_kind: None,
            },
        );
        ev.ts = OffsetDateTime::from_unix_timestamp(ts).expect("valid timestamp");
        ev
    }

    #[test]
    fn incremental_refresh_only_fetches_modified_tasks() {
        let first = TaskId::new();
        let second = TaskId::new();
        let store = Arc::new(MockStore::with_tasks(vec![
            (first, vec![mock_event(first, "first", 5)]),
            (second, vec![mock_event(second, "second", 6)]),
        ]));

        let repo = TaskRepository::new(Arc::clone(&store));
        repo.list_snapshots(None).expect("initial load");
        assert_eq!(store.list_tasks_calls(), 1);
        assert_eq!(store.list_modified_calls(), 0);
        assert_eq!(store.load_events_calls(first), 1);
        assert_eq!(store.load_events_calls(second), 1);

        store.replace_events(
            second,
            vec![
                mock_event(second, "second", 6),
                mock_event(second, "second updated", 10),
            ],
        );

        repo.list_snapshots(None).expect("refresh loads diff");
        assert_eq!(store.list_tasks_calls(), 1);
        assert_eq!(store.list_modified_calls(), 1);
        assert_eq!(store.load_events_calls(first), 1);
        assert_eq!(store.load_events_calls(second), 2);
        let view = repo.get_view(second).expect("must load updated task");
        assert_eq!(view.snapshot.title, "second updated");
    }
}
