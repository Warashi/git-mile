//! Git-backed storage implementation for git-mile.

use anyhow::{Context, Result, anyhow};
use git_mile_core::event::Event;
use git_mile_core::id::TaskId;
use git2::{Commit, Oid, Repository, Signature, Sort};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::Mutex;
use tracing::{debug, info};

const EVENT_CACHE_CAPACITY: usize = 256;

/// Storage based on git refs under `refs/git-mile/tasks/*`.
pub struct GitStore {
    repo: Repository,
    event_cache: Mutex<LruCache<Oid, Event>>,
}

impl GitStore {
    /// Discover and open the repository from `cwd_or_repo`.
    ///
    /// # Errors
    /// Returns an error if a Git repository cannot be discovered from the given path.
    pub fn open(cwd_or_repo: impl AsRef<Path>) -> Result<Self> {
        let repo = Repository::discover(cwd_or_repo).context("Failed to discover .git")?;
        let capacity = NonZeroUsize::new(EVENT_CACHE_CAPACITY)
            .ok_or_else(|| anyhow!("cache capacity must be non-zero"))?;
        let cache = LruCache::new(capacity);
        Ok(Self {
            repo,
            event_cache: Mutex::new(cache),
        })
    }

    /// Name of the ref for a task.
    fn refname(task: &TaskId) -> String {
        format!("refs/git-mile/tasks/{task}")
    }

    fn cached_event(&self, oid: Oid) -> Option<Event> {
        self.event_cache
            .lock()
            .ok()
            .and_then(|mut cache| cache.get(&oid).cloned())
    }

    fn cache_event(&self, oid: Oid, event: Event) {
        if let Ok(mut cache) = self.event_cache.lock() {
            cache.put(oid, event);
        }
    }

    fn cached_or_decode_event(&self, oid: Oid) -> Result<Option<Event>> {
        if let Some(ev) = self.cached_event(oid) {
            return Ok(Some(ev));
        }
        let Some(ev) = self.decode_event_from_commit(oid)? else {
            return Ok(None);
        };
        self.cache_event(oid, ev.clone());
        Ok(Some(ev))
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
        if !head.starts_with("git-mile-event: ") {
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

        // Empty tree
        let tree_oid = {
            let mut idx = self.repo.index()?;
            idx.clear()?;
            idx.write_tree()?
        };
        let tree = self.repo.find_tree(tree_oid)?;

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
        let msg = format!("git-mile-event: {}\n\n{}", ev.id, body);

        let parent_refs: Vec<&Commit<'_>> = parents.iter().collect();
        let oid = self
            .repo
            .commit(Some(&refname), &sig, &sig, &msg, &tree, &parent_refs)?;

        info!(%oid, %refname, "Appended event");
        Ok(oid)
    }

    /// Load events by walking commits reachable from `refs/git-mile/tasks/<id>`.
    ///
    /// # Errors
    /// Returns an error if the task ref is missing or commit history cannot be traversed.
    pub fn load_events(&self, task: TaskId) -> Result<Vec<Event>> {
        let refname = Self::refname(&task);
        let reference = self
            .repo
            .find_reference(&refname)
            .with_context(|| format!("Task not found: {refname}"))?;
        let tip = reference.target().ok_or_else(|| anyhow!("Ref has no target"))?;

        let mut rev = self.repo.revwalk()?;
        rev.set_sorting(Sort::TIME | Sort::REVERSE)?;
        rev.push(tip)?;

        let mut out = Vec::new();
        for oid in rev {
            let oid = oid?;
            if let Some(ev) = self.cached_or_decode_event(oid)? {
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
            if let Some(id_str) = name.strip_prefix("refs/git-mile/tasks/")
                && let Ok(id) = id_str.parse()
            {
                ids.push(id);
            }
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::event::{Actor, Event, EventKind};
    use git_mile_core::StateKind;
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
        let msg = format!("git-mile-event: {}\n\n{}", legacy_event.id, body);
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

    fn temp_repo_path() -> Result<PathBuf> {
        let path = std::env::temp_dir().join(format!("git-mile-test-{}", TaskId::new()));
        if path.exists() {
            fs::remove_dir_all(&path)?;
        }
        fs::create_dir(&path)?;
        Ok(path)
    }
}
