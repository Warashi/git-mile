//! Git-backed storage implementation for git-mile.

mod error;

pub use error::GitStoreError;

use anyhow::{Context, Result, anyhow};
use git_mile_core::event::Event;
use git_mile_core::id::TaskId;
use git2::{Commit, Oid, Repository, Signature, Sort};
use lru::LruCache;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::{env, num::NonZeroUsize};
use tracing::{debug, info};

/// Default capacity for the event cache when no override is provided.
const DEFAULT_EVENT_CACHE_CAPACITY: usize = 256;
/// Environment variable controlling the `GitStore` event cache capacity.
const EVENT_CACHE_CAPACITY_ENV_VAR: &str = "GIT_MILE_CACHE_CAPACITY";
/// Prefix placed ahead of every git-mile event commit message.
const EVENT_COMMIT_PREFIX: &str = "git-mile-event: ";
/// Canonical OID of Git's empty tree object.
const EMPTY_TREE_OID_HEX: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// Storage based on git refs under `refs/git-mile/tasks/*`.
pub struct GitStore {
    repo: Repository,
    repo_path: PathBuf,
    event_cache: Arc<Mutex<LruCache<TaskId, Vec<Event>>>>,
    empty_tree_oid: Oid,
}

impl GitStore {
    /// Discover and open the repository from `cwd_or_repo`.
    ///
    /// # Errors
    /// Returns an error if a Git repository cannot be discovered from the given path.
    pub fn open(cwd_or_repo: impl AsRef<Path>) -> Result<Self> {
        let repo = Repository::discover(cwd_or_repo).context("Failed to discover .git")?;
        let repo_path = repo.path().to_path_buf();
        let cache = LruCache::new(Self::event_cache_capacity());
        let empty_tree_oid = Self::ensure_empty_tree(&repo)?;
        Ok(Self {
            repo,
            repo_path,
            event_cache: Arc::new(Mutex::new(cache)),
            empty_tree_oid,
        })
    }

    /// Name of the ref for a task.
    fn refname(task: &TaskId) -> String {
        format!("refs/git-mile/tasks/{task}")
    }

    fn task_id_from_refname(name: &str) -> Option<TaskId> {
        name.strip_prefix("refs/git-mile/tasks/")?.parse().ok()
    }

    fn event_cache_capacity() -> NonZeroUsize {
        let env_value = env::var(EVENT_CACHE_CAPACITY_ENV_VAR).ok();
        Self::cache_capacity_from_override(env_value).unwrap_or_else(Self::default_cache_capacity)
    }

    fn default_cache_capacity() -> NonZeroUsize {
        NonZeroUsize::new(DEFAULT_EVENT_CACHE_CAPACITY).map_or_else(
            || unreachable!("DEFAULT_EVENT_CACHE_CAPACITY must be non-zero"),
            |value| value,
        )
    }

    fn cache_capacity_from_override(raw: Option<String>) -> Option<NonZeroUsize> {
        raw?.parse::<usize>().ok().and_then(NonZeroUsize::new)
    }

    fn cached_events(&self, task: TaskId) -> Option<Vec<Event>> {
        self.event_cache
            .lock()
            .ok()
            .and_then(|mut cache| cache.get(&task).cloned())
    }

    fn cache_events(&self, task: TaskId, events: &[Event]) {
        if let Ok(mut cache) = self.event_cache.lock() {
            cache.put(task, events.to_vec());
        }
    }

    fn invalidate_cached_events(&self, task: TaskId) {
        if let Ok(mut cache) = self.event_cache.lock() {
            cache.pop(&task);
        }
    }

    fn ensure_empty_tree(repo: &Repository) -> Result<Oid> {
        let mut idx = repo.index()?;
        idx.clear()?;
        Ok(idx.write_tree()?)
    }

    fn decode_event_from_commit(&self, oid: Oid) -> Result<Option<Event>> {
        let commit = self
            .repo
            .find_commit(oid)
            .with_context(|| format!("Object is not a commit: {oid}"))?;
        Self::event_from_commit(&commit, oid)
    }

    fn event_from_commit(commit: &Commit<'_>, oid: Oid) -> Result<Option<Event>> {
        let Some(message) = commit.message() else {
            return Ok(None);
        };
        let Some((head, body)) = message.split_once("\n\n") else {
            return Ok(None);
        };
        if !head.starts_with(EVENT_COMMIT_PREFIX) {
            return Ok(None);
        }

        let ev: Event = serde_json::from_str(body)
            .with_context(|| format!("Failed to parse event JSON in commit {oid}"))?;
        Ok(Some(ev))
    }

    /// Append an event as a single commit with empty tree.
    ///
    /// # Errors
    /// Returns an error if any Git object manipulation fails.
    pub fn append_event(&self, ev: &Event) -> Result<Oid> {
        let refname = Self::refname(&ev.task);

        // Author/committer signature from event actor.
        let sig = Signature::now(&ev.actor.name, &ev.actor.email)
            .with_context(|| format!("Invalid signature: {} <{}>", ev.actor.name, ev.actor.email))?;

        debug_assert_eq!(
            self.empty_tree_oid.to_string(),
            EMPTY_TREE_OID_HEX,
            "empty tree OID should remain stable"
        );
        let tree = self.repo.find_tree(self.empty_tree_oid)?;

        // Parent (if ref exists)
        let parents: Vec<Commit<'_>> = match self.repo.find_reference(&refname) {
            Ok(r) => {
                let target = r.target().ok_or_else(|| anyhow!("Ref {refname} has no target"))?;
                let parent = self.repo.find_commit(target)?;
                vec![parent]
            }
            Err(_) => Vec::new(),
        };

        // Commit message: first line + blank + pretty JSON
        let body = serde_json::to_string_pretty(ev)?;
        let msg = format!("{EVENT_COMMIT_PREFIX}{}\n\n{}", ev.id, body);

        let parent_refs: Vec<&Commit<'_>> = parents.iter().collect();
        let oid = self
            .repo
            .commit(Some(&refname), &sig, &sig, &msg, &tree, &parent_refs)?;

        info!(%oid, %refname, "Appended event");
        self.invalidate_cached_events(ev.task);
        Ok(oid)
    }

    /// Load events by walking commits reachable from `refs/git-mile/tasks/<id>`.
    ///
    /// # Errors
    /// Returns an error if the task ref is missing or commit history cannot be traversed.
    pub fn load_events(&self, task: TaskId) -> Result<Vec<Event>> {
        if let Some(events) = self.cached_events(task) {
            return Ok(events);
        }
        let refname = Self::refname(&task);
        let reference = self
            .repo
            .find_reference(&refname)
            .with_context(|| format!("Task not found: {refname}"))?;
        let events = self.load_events_from_reference(task, &reference)?;
        self.cache_events(task, &events);
        Ok(events)
    }

    /// Load events for every known task reference.
    ///
    /// # Errors
    /// Returns an error if reference enumeration fails.
    pub fn load_all_task_events(&self) -> Result<Vec<(TaskId, Vec<Event>)>> {
        let mut results = Vec::new();
        let references = self.repo.references_glob("refs/git-mile/tasks/*")?;
        for reference in references {
            let reference = reference?;
            let Some(name) = reference.name() else {
                continue;
            };
            let Some(task_id) = Self::task_id_from_refname(name) else {
                continue;
            };
            let events = self.load_events_from_reference(task_id, &reference)?;
            self.cache_events(task_id, &events);
            results.push((task_id, events));
        }
        Ok(results)
    }

    fn load_events_from_reference(
        &self,
        task: TaskId,
        reference: &git2::Reference<'_>,
    ) -> Result<Vec<Event>> {
        let tip = reference.target().ok_or_else(|| anyhow!("Ref has no target"))?;
        self.load_events_from_tip(task, tip)
    }

    fn load_events_from_tip(&self, task: TaskId, tip: Oid) -> Result<Vec<Event>> {
        let mut rev = self.repo.revwalk()?;
        rev.set_sorting(Sort::TIME | Sort::REVERSE)?;
        rev.push(tip)?;

        let mut out = Vec::new();
        for oid in rev {
            let oid = oid?;
            if let Some(ev) = self.decode_event_from_commit(oid)? {
                if ev.task == task {
                    out.push(ev);
                } else {
                    debug!(event_task = %ev.task, requested = %task, %oid, "Ignoring event for different task");
                }
            }
        }

        Ok(out)
    }

    /// List task ids by scanning `refs/git-mile/tasks/*`.
    ///
    /// # Errors
    /// Returns an error if reference enumeration fails.
    pub fn list_tasks(&self) -> Result<Vec<TaskId>> {
        let mut ids = Vec::new();
        for r in self.repo.references_glob("refs/git-mile/tasks/*")? {
            let r = r?;
            let name = r.name().ok_or_else(|| anyhow!("Invalid ref name"))?;
            if let Some(id) = Self::task_id_from_refname(name) {
                ids.push(id);
            }
        }
        Ok(ids)
    }

    /// Check if a task exists without loading its events.
    ///
    /// # Errors
    /// Returns an error if the reference check fails.
    pub fn task_exists(&self, task: TaskId) -> Result<bool> {
        let refname = Self::refname(&task);
        match self.repo.find_reference(&refname) {
            Ok(_) => Ok(true),
            Err(e) if e.code() == git2::ErrorCode::NotFound => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// List task IDs that have been modified since the given timestamp.
    ///
    /// # Errors
    /// Returns an error if reference enumeration or commit access fails.
    pub fn list_tasks_modified_since(&self, since: time::OffsetDateTime) -> Result<Vec<TaskId>> {
        let mut modified_tasks = Vec::new();
        let since_unix = since.unix_timestamp();

        for reference in self.repo.references_glob("refs/git-mile/tasks/*")? {
            let reference = reference?;
            let Some(name) = reference.name() else {
                continue;
            };
            let Some(task_id) = Self::task_id_from_refname(name) else {
                continue;
            };

            // Check the latest commit timestamp
            if let Ok(commit) = reference.peel_to_commit() {
                let commit_time = commit.time().seconds();
                if commit_time >= since_unix {
                    modified_tasks.push(task_id);
                }
            }
        }

        Ok(modified_tasks)
    }
}

impl Clone for GitStore {
    /// Clone the `GitStore` by reopening the same repository.
    ///
    /// Note: The event cache is **shared** between clones via `Arc`.
    /// All clones benefit from the same LRU cache.
    ///
    /// # Panics
    /// Panics if the repository cannot be reopened at the saved path.
    fn clone(&self) -> Self {
        // Reopen the repository from the saved path
        let repo = Repository::open(&self.repo_path)
            .unwrap_or_else(|_| panic!("Failed to reopen repository at {}", self.repo_path.display()));

        Self {
            repo,
            repo_path: self.repo_path.clone(),
            event_cache: Arc::clone(&self.event_cache),
            empty_tree_oid: self.empty_tree_oid,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::StateKind;
    use git_mile_core::event::{Actor, Event, EventKind};
    use git2::Signature;
    use serde_json::Value;
    use std::{fs, path::PathBuf, thread, time::Duration as StdDuration};

    #[test]
    fn append_and_load_roundtrip() -> Result<()> {
        let base = temp_repo_path()?;
        Repository::init(&base)?;

        let store = GitStore::open(&base)?;
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let created = Event::new(
            task,
            &actor,
            EventKind::TaskCreated {
                title: "Add docs".into(),
                labels: vec![],
                assignees: vec![],
                description: None,
                state: None,
                state_kind: None,
            },
        );

        let second = Event::new(
            task,
            &actor,
            EventKind::TaskTitleSet {
                title: "Polish docs".into(),
            },
        );

        let oid = store.append_event(&created)?;
        assert_ne!(oid, Oid::zero());
        thread::sleep(StdDuration::from_millis(10));
        store.append_event(&second)?;

        let tasks = store.list_tasks()?;
        assert_eq!(tasks, vec![task]);

        let events = store.load_events(task)?;
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].task, task);
        assert!(matches!(events[0].kind, EventKind::TaskCreated { .. }));
        assert!(matches!(events[1].kind, EventKind::TaskTitleSet { .. }));

        let titles: Vec<_> = events
            .iter()
            .map(|ev| match &ev.kind {
                EventKind::TaskCreated { title, .. } | EventKind::TaskTitleSet { title } => title.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(titles, vec!["Add docs", "Polish docs"]);

        let cached_events = store.load_events(task)?;
        let cached_titles: Vec<_> = cached_events
            .iter()
            .map(|ev| match &ev.kind {
                EventKind::TaskCreated { title, .. } | EventKind::TaskTitleSet { title } => title.as_str(),
                _ => "",
            })
            .collect();
        assert_eq!(titles, cached_titles);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn append_event_serializes_state_kind_into_commit_body() -> Result<()> {
        let base = temp_repo_path()?;
        Repository::init(&base)?;

        let store = GitStore::open(&base)?;
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let event = Event::new(
            task,
            &actor,
            EventKind::TaskStateSet {
                state: "state/done".into(),
                state_kind: Some(StateKind::Done),
            },
        );

        let oid = store.append_event(&event)?;
        let commit = store.repo.find_commit(oid)?;
        let message = commit.message().context("Commit must have a message")?;
        let (_, body) = message
            .split_once("\n\n")
            .context("Commit message must contain JSON body")?;
        let json: Value = serde_json::from_str(body)?;
        let serialized_kind = json
            .get("kind")
            .and_then(|kind| kind.get("state_kind"))
            .and_then(Value::as_str)
            .context("state_kind must be serialized as string")?;
        assert_eq!(serialized_kind, "done");

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn load_events_accepts_commits_without_state_kind_field() -> Result<()> {
        let base = temp_repo_path()?;
        let repo = Repository::init(&base)?;

        let store = GitStore::open(&base)?;
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let legacy_event = Event::new(
            task,
            &actor,
            EventKind::TaskStateSet {
                state: "state/in-progress".into(),
                state_kind: Some(StateKind::InProgress),
            },
        );
        let mut legacy_value = serde_json::to_value(&legacy_event)?;
        if let Some(kind) = legacy_value.get_mut("kind")
            && let Some(obj) = kind.as_object_mut()
        {
            obj.remove("state_kind");
        }
        let body = serde_json::to_string_pretty(&legacy_value)?;
        let refname = GitStore::refname(&task);
        let msg = format!("{EVENT_COMMIT_PREFIX}{}\n\n{}", legacy_event.id, body);
        let sig = Signature::now(&actor.name, &actor.email)?;
        let mut idx = repo.index()?;
        idx.clear()?;
        let tree = {
            let tree_oid = idx.write_tree()?;
            repo.find_tree(tree_oid)?
        };
        repo.commit(Some(&refname), &sig, &sig, &msg, &tree, &[])?;

        let events = store.load_events(task)?;
        assert_eq!(events.len(), 1);
        match &events[0].kind {
            EventKind::TaskStateSet { state, state_kind } => {
                assert_eq!(state, "state/in-progress");
                assert!(state_kind.is_none(), "state_kind defaults to None");
            }
            other => panic!("unexpected event kind: {other:?}"),
        }

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn load_events_cache_is_invalidated_after_append() -> Result<()> {
        let base = temp_repo_path()?;
        Repository::init(&base)?;

        let store = GitStore::open(&base)?;
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let created = Event::new(
            task,
            &actor,
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: vec![],
                assignees: vec![],
                description: None,
                state: None,
                state_kind: None,
            },
        );
        store.append_event(&created)?;
        assert_eq!(store.load_events(task)?.len(), 1);

        let updated = Event::new(
            task,
            &actor,
            EventKind::TaskTitleSet {
                title: "Updated".into(),
            },
        );
        store.append_event(&updated)?;

        let events = store.load_events(task)?;
        assert_eq!(events.len(), 2, "cache must be invalidated after append");

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn load_all_events_returns_every_task() -> Result<()> {
        let base = temp_repo_path()?;
        Repository::init(&base)?;

        let store = GitStore::open(&base)?;
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let first = TaskId::new();
        let second = TaskId::new();

        for (task, title) in [(first, "First"), (second, "Second")] {
            store.append_event(&Event::new(
                task,
                &actor,
                EventKind::TaskCreated {
                    title: title.into(),
                    labels: vec![],
                    assignees: vec![],
                    description: None,
                    state: None,
                    state_kind: None,
                },
            ))?;
        }

        let mut all = store.load_all_task_events()?;
        all.sort_by_key(|(task, _)| *task);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].0, first);
        assert_eq!(all[1].0, second);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn task_exists_returns_true_for_existing_task() -> Result<()> {
        let base = temp_repo_path()?;
        Repository::init(&base)?;

        let store = GitStore::open(&base)?;
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        // Task doesn't exist yet
        assert!(!store.task_exists(task)?);

        // Create task
        let event = Event::new(
            task,
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
        store.append_event(&event)?;

        // Task now exists
        assert!(store.task_exists(task)?);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn task_exists_returns_false_for_nonexistent_task() -> Result<()> {
        let base = temp_repo_path()?;
        Repository::init(&base)?;

        let store = GitStore::open(&base)?;
        let task = TaskId::new();

        assert!(!store.task_exists(task)?);

        fs::remove_dir_all(&base)?;
        Ok(())
    }

    #[test]
    fn capacity_override_accepts_valid_numbers() {
        if let Some(override_value) = GitStore::cache_capacity_from_override(Some("512".into())) {
            assert_eq!(override_value.get(), 512);
        } else {
            panic!("override must accept positive numbers");
        }
    }

    #[test]
    fn capacity_override_rejects_invalid_values() {
        assert!(GitStore::cache_capacity_from_override(Some("abc".into())).is_none());
        assert!(GitStore::cache_capacity_from_override(Some("0".into())).is_none());
        assert!(GitStore::cache_capacity_from_override(None).is_none());
    }

    fn temp_repo_path() -> Result<PathBuf> {
        let path = std::env::temp_dir().join(format!("git-mile-test-{}", TaskId::new()));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir(&path)?;
        Ok(path)
    }
}
