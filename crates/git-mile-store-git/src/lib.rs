//! Git-backed storage implementation for git-mile.

use anyhow::{anyhow, Context, Result};
use git2::{Commit, ObjectType, Oid, Repository, Signature, Sort};
use git_mile_core::event::Event;
use git_mile_core::id::TaskId;
use std::path::Path;
use tracing::{debug, info};

/// Storage based on git refs under `refs/git-mile/tasks/*`.
pub struct GitStore {
    repo: Repository,
}

impl GitStore {
    /// Discover and open the repository from `cwd_or_repo`.
    ///
    /// # Errors
    /// Returns an error if a Git repository cannot be discovered from the given path.
    pub fn open(cwd_or_repo: impl AsRef<Path>) -> Result<Self> {
        let repo = Repository::discover(cwd_or_repo).context("Failed to discover .git")?;
        Ok(Self { repo })
    }

    /// Name of the ref for a task.
    fn refname(task: &TaskId) -> String {
        format!("refs/git-mile/tasks/{task}")
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
        rev.set_sorting(Sort::TOPOLOGICAL)?;
        rev.push(tip)?;

        let mut out = Vec::new();
        for oid in rev {
            let oid = oid?;
            let obj = self.repo.find_object(oid, Some(ObjectType::Commit))?;
            let commit = obj
                .into_commit()
                .map_err(|_| anyhow!("Object is not a commit: {oid}"))?;

            if let Some(msg) = commit.message() {
                if let Some((head, body)) = msg.split_once("\n\n") {
                    if head.starts_with("git-mile-event: ") {
                        let ev: Event = serde_json::from_str(body)
                            .with_context(|| format!("Failed to parse event JSON in commit {oid}"))?;
                        if ev.task == task {
                            out.push(ev);
                        } else {
                            debug!("Ignoring event for different task in {oid}");
                        }
                    }
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
            if let Some(id_str) = name.strip_prefix("refs/git-mile/tasks/") {
                if let Ok(id) = id_str.parse() {
                    ids.push(id);
                }
            }
        }
        Ok(ids)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::event::{Actor, Event, EventKind};
    use std::fs;
    use std::path::PathBuf;

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

        let event = Event::new(
            task,
            &actor,
            EventKind::TaskCreated {
                title: "Add docs".into(),
                labels: vec![],
                assignees: vec![],
                description: None,
                state: None,
            },
        );

        let oid = store.append_event(&event)?;
        assert_ne!(oid, Oid::zero());

        let tasks = store.list_tasks()?;
        assert_eq!(tasks, vec![task]);

        let events = store.load_events(task)?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].task, task);
        if let EventKind::TaskCreated { title, .. } = &events[0].kind {
            assert_eq!(title, "Add docs");
        } else {
            panic!("unexpected event kind");
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
