//! Shared task snapshot cache utilities reused by CLI/TUI/MCP.

use std::cmp::Ordering;
use std::collections::HashMap;

use crate::task_writer::TaskStore;
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::TaskId;
use git_mile_core::{OrderedEvents, TaskFilter, TaskSnapshot};
use time::OffsetDateTime;

/// Actor-written comment on a task.
#[derive(Debug, Clone)]
pub struct TaskComment {
    /// Actor who authored the comment.
    pub actor: Actor,
    /// Comment body in Markdown.
    pub body: String,
    /// Event timestamp in UTC.
    pub ts: OffsetDateTime,
}

/// Materialized view combining snapshot + comments.
#[derive(Debug, Clone)]
pub struct TaskView {
    /// Current snapshot derived from the CRDT.
    pub snapshot: TaskSnapshot,
    /// Chronological comment history.
    pub comments: Vec<TaskComment>,
    /// Timestamp of the most recent event.
    pub last_updated: Option<OffsetDateTime>,
}

impl TaskView {
    /// Build a [`TaskView`] from raw event history.
    #[must_use]
    pub fn from_events(events: &[Event]) -> Self {
        let ordered = OrderedEvents::from(events);
        let snapshot = TaskSnapshot::replay_ordered(&ordered);

        let comments = ordered
            .iter()
            .filter_map(|ev| {
                if let EventKind::CommentAdded { body_md, .. } = &ev.kind {
                    Some(TaskComment {
                        actor: ev.actor.clone(),
                        body: body_md.clone(),
                        ts: ev.ts,
                    })
                } else {
                    None
                }
            })
            .collect();

        let last_updated = ordered.latest().map(|ev| ev.ts);

        Self {
            snapshot,
            comments,
            last_updated,
        }
    }
}

/// Cached task snapshots and relation indexes.
#[derive(Debug, Default)]
pub struct TaskCache {
    /// Chronologically sorted task views.
    pub tasks: Vec<TaskView>,
    /// Mapping from task id to index into [`tasks`](Self::tasks).
    pub task_index: HashMap<TaskId, usize>,
    /// Cached parent relationships.
    pub parents_index: HashMap<TaskId, Vec<TaskId>>,
    /// Cached child relationships.
    pub children_index: HashMap<TaskId, Vec<TaskId>>,
}

impl TaskCache {
    /// Load every task snapshot from the store and build indexes.
    ///
    /// # Errors
    ///
    /// Propagates store-specific read failures.
    pub fn load<S>(store: &S) -> Result<Self, S::Error>
    where
        S: TaskStore,
    {
        let mut views = Vec::new();

        for task_id in store.list_tasks()? {
            let events = store.load_events(task_id)?;
            views.push(TaskView::from_events(&events));
        }

        views.sort_by(|a, b| match (a.last_updated, b.last_updated) {
            (Some(a_ts), Some(b_ts)) => b_ts.cmp(&a_ts),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => a.snapshot.id.cmp(&b.snapshot.id),
        });

        Ok(Self::from_views(views))
    }

    fn from_views(views: Vec<TaskView>) -> Self {
        let mut cache = Self {
            tasks: views,
            task_index: HashMap::new(),
            parents_index: HashMap::new(),
            children_index: HashMap::new(),
        };
        cache.rebuild_indexes();
        cache
    }

    fn rebuild_indexes(&mut self) {
        self.task_index.clear();
        self.parents_index.clear();
        self.children_index.clear();

        for (idx, view) in self.tasks.iter().enumerate() {
            self.task_index.insert(view.snapshot.id, idx);
        }

        for view in &self.tasks {
            let parents: Vec<TaskId> = view.snapshot.parents.iter().copied().collect();
            for parent in &parents {
                self.children_index
                    .entry(*parent)
                    .or_default()
                    .push(view.snapshot.id);
            }
            self.children_index.entry(view.snapshot.id).or_default();
            self.parents_index.insert(view.snapshot.id, parents);
        }
    }

    /// Iterate over cached snapshots in last-updated order.
    pub fn snapshots(&self) -> impl Iterator<Item = &TaskSnapshot> {
        self.tasks.iter().map(|view| &view.snapshot)
    }

    /// Return snapshots filtered by the provided filter.
    #[must_use]
    pub fn filtered_snapshots(&self, filter: &TaskFilter) -> Vec<TaskSnapshot> {
        self.tasks
            .iter()
            .filter(|view| filter.matches(&view.snapshot))
            .map(|view| view.snapshot.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_writer::TaskStore as CoreTaskStore;
    use git_mile_core::TaskFilter;
    use git_mile_core::event::{Actor, Event, EventKind};
    use git_mile_core::id::TaskId;
    use git2::Oid;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::str::FromStr;
    use time::OffsetDateTime;

    #[derive(Default)]
    struct MockStore {
        tasks: RefCell<Vec<TaskId>>,
        events: RefCell<HashMap<TaskId, Vec<Event>>>,
    }

    impl MockStore {
        fn with_task(self, id: TaskId, events: Vec<Event>) -> Self {
            self.tasks.borrow_mut().push(id);
            self.events.borrow_mut().insert(id, events);
            self
        }
    }

    impl CoreTaskStore for MockStore {
        type Error = anyhow::Error;

        fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error> {
            Ok(self.events.borrow().contains_key(&task))
        }

        fn append_event(&self, _event: &Event) -> Result<Oid, Self::Error> {
            unreachable!("append_event is not used in TaskCache tests")
        }

        fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
            Ok(self.events.borrow().get(&task).cloned().unwrap_or_default())
        }

        fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
            Ok(self.tasks.borrow().clone())
        }

        fn list_tasks_modified_since(
            &self,
            _since: time::OffsetDateTime,
        ) -> Result<Vec<TaskId>, Self::Error> {
            // For testing, return all tasks
            self.list_tasks()
        }
    }

    fn fixed_task_id(n: u8) -> TaskId {
        TaskId::from_str(&format!("00000000-0000-0000-0000-0000000000{n:02}"))
            .unwrap_or_else(|err| panic!("must parse task id: {err}"))
    }

    fn actor() -> Actor {
        Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        }
    }

    fn ts(secs: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(secs)
            .unwrap_or_else(|err| panic!("must convert unix timestamp: {err}"))
    }

    fn created(task: TaskId, secs: i64, title: &str) -> Event {
        let mut ev = Event::new(
            task,
            &actor(),
            EventKind::TaskCreated {
                title: title.into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: None,
                state_kind: None,
            },
        );
        ev.ts = ts(secs);
        ev
    }

    fn child_link(target: TaskId, secs: i64, parent: TaskId, child: TaskId) -> Event {
        let mut ev = Event::new(target, &actor(), EventKind::ChildLinked { parent, child });
        ev.ts = ts(secs);
        ev
    }

    #[test]
    fn load_sorts_tasks_by_latest_update() {
        let first = fixed_task_id(1);
        let second = fixed_task_id(2);
        let third = fixed_task_id(3);

        let store = MockStore::default()
            .with_task(first, vec![created(first, 10, "first")])
            .with_task(second, vec![created(second, 30, "second")])
            .with_task(third, vec![created(third, 20, "third")]);

        let cache = TaskCache::load(&store).unwrap_or_else(|err| panic!("must load cache: {err}"));
        let ids: Vec<TaskId> = cache.snapshots().map(|view| view.id).collect();
        assert_eq!(ids, vec![second, third, first]);
    }

    #[test]
    fn indexes_track_parent_child_relationships() {
        let parent = fixed_task_id(10);
        let child = fixed_task_id(20);

        let parent_events = vec![
            created(parent, 5, "parent"),
            child_link(parent, 15, parent, child),
        ];
        let mut child_events = vec![created(child, 6, "child")];
        child_events.push(child_link(child, 15, parent, child));

        let store = MockStore::default()
            .with_task(parent, parent_events)
            .with_task(child, child_events);

        let cache = TaskCache::load(&store).unwrap_or_else(|err| panic!("must load cache: {err}"));

        assert_eq!(
            cache.parents_index.get(&child).map(Vec::as_slice),
            Some(&[parent][..])
        );
        assert_eq!(
            cache.children_index.get(&parent).map(Vec::as_slice),
            Some(&[child][..])
        );
        assert!(cache.children_index.contains_key(&child));
    }

    #[test]
    fn filtered_snapshots_apply_task_filter() {
        let todo = fixed_task_id(30);
        let done = fixed_task_id(31);

        let store = MockStore::default()
            .with_task(todo, vec![created(todo, 5, "todo task")])
            .with_task(done, vec![created(done, 10, "done task")]);

        let filter = TaskFilter {
            text: Some("done".into()),
            ..TaskFilter::default()
        };

        let cache = TaskCache::load(&store).unwrap_or_else(|err| panic!("must load cache: {err}"));
        let filtered = cache.filtered_snapshots(&filter);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, done);
    }
}
