use git_mile_core::id::TaskId;
use std::cmp::Ordering;
use std::collections::HashMap;

use super::app::{TaskStore, TaskView};

/// Cached task snapshots and relation indexes used by the TUI.
#[derive(Debug, Default)]
pub(super) struct TaskCache {
    pub tasks: Vec<TaskView>,
    pub task_index: HashMap<TaskId, usize>,
    pub parents_index: HashMap<TaskId, Vec<TaskId>>,
    pub children_index: HashMap<TaskId, Vec<TaskId>>,
}

impl TaskCache {
    /// Load every task snapshot from the store and build indexes.
    pub(super) fn load<S: TaskStore>(store: &S) -> Result<Self, S::Error> {
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_writer::TaskStore as CoreTaskStore;
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

        fn append_event(&self, _event: &Event) -> Result<Oid, Self::Error> {
            unreachable!("append_event is not used in TaskCache tests")
        }

        fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
            Ok(self.events.borrow().get(&task).cloned().unwrap_or_default())
        }

        fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
            Ok(self.tasks.borrow().clone())
        }
    }

    fn fixed_task_id(n: u8) -> TaskId {
        TaskId::from_str(&format!("00000000-0000-0000-0000-0000000000{n:02}")).unwrap()
    }

    fn actor() -> Actor {
        Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        }
    }

    fn ts(secs: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(secs).unwrap()
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

        let cache = TaskCache::load(&store).expect("must load cache");
        let ids: Vec<TaskId> = cache.tasks.iter().map(|view| view.snapshot.id).collect();
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

        let cache = TaskCache::load(&store).expect("must load cache");

        assert_eq!(
            cache.parents_index.get(&child).map(Vec::as_slice),
            Some(&[parent][..])
        );
        assert_eq!(
            cache.children_index.get(&parent).map(Vec::as_slice),
            Some(&[child][..])
        );
        assert!(cache.children_index.get(&child).is_some());
    }
}
