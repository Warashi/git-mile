use anyhow::{Result, anyhow};
use git_mile_core::TaskSnapshot;
use git_mile_core::event::Actor;
use git_mile_core::id::TaskId;
use git2::Oid;

use std::path::PathBuf;

use crate::config::{HooksConfig, WorkflowConfig};
use crate::task_writer::{CommentRequest, CreateTaskRequest, TaskStore, TaskWriter};

/// Service façade that encapsulates all task-related side effects.
pub struct TaskService<S> {
    writer: TaskWriter<S>,
}

impl<S> TaskService<S> {
    pub const fn new(store: S, workflow: WorkflowConfig, hooks_config: HooksConfig, base_dir: PathBuf) -> Self
    where
        S: TaskStore,
    {
        Self {
            writer: TaskWriter::new(store, workflow, hooks_config, base_dir),
        }
    }

    pub const fn workflow(&self) -> &WorkflowConfig {
        self.writer.workflow()
    }

    const fn store(&self) -> &S {
        self.writer.store()
    }
}

impl<S: TaskStore> TaskService<S> {
    /// Create a task and optionally link parents.
    ///
    /// # Errors
    /// Returns an error if validation fails or emitting events is unsuccessful.
    pub fn create_with_parents(&self, input: CreateTaskInput) -> Result<CreateTaskOutput> {
        let CreateTaskInput {
            title,
            state,
            labels,
            assignees,
            description,
            parents,
            actor,
        } = input;

        let request = CreateTaskRequest {
            title,
            state,
            labels,
            assignees,
            description,
            parents,
            actor,
        };
        let result = self.writer.create_task(request)?;
        let created_event_oid = *result
            .events
            .first()
            .ok_or_else(|| anyhow!("TaskWriter returned no events for create_task"))?;
        let parent_links = result
            .parent_links
            .into_iter()
            .map(|link| ParentLink {
                parent: link.parent,
                oid: link.oid,
            })
            .collect();

        Ok(CreateTaskOutput {
            task: result.task,
            created_event_oid,
            parent_links,
        })
    }

    /// Append a comment to the specified task.
    ///
    /// # Errors
    /// Returns an error if the task cannot be loaded or the event append fails.
    pub fn add_comment(&self, input: CommentInput) -> Result<CommentOutput> {
        let CommentInput {
            task, message, actor, ..
        } = input;
        let result = self.writer.add_comment(
            task,
            CommentRequest {
                body_md: message,
                actor,
            },
        )?;
        let oid = *result
            .events
            .first()
            .ok_or_else(|| anyhow!("TaskWriter returned no events for add_comment"))?;
        Ok(CommentOutput { task, oid })
    }

    /// Build a [`TaskSnapshot`] for the given task by replaying events.
    ///
    /// # Errors
    /// Returns an error if event loading fails.
    pub fn materialize(&self, task: TaskId) -> Result<TaskSnapshot> {
        let events = self.store().load_events(task).map_err(Into::into)?;
        Ok(TaskSnapshot::replay(&events))
    }
}

pub struct CreateTaskInput {
    pub title: String,
    pub state: Option<String>,
    pub labels: Vec<String>,
    pub assignees: Vec<String>,
    pub description: Option<String>,
    pub parents: Vec<TaskId>,
    pub actor: Actor,
}

pub struct CreateTaskOutput {
    pub task: TaskId,
    pub created_event_oid: Oid,
    pub parent_links: Vec<ParentLink>,
}

pub struct ParentLink {
    pub parent: TaskId,
    pub oid: Oid,
}

pub struct CommentInput {
    pub task: TaskId,
    pub message: String,
    pub actor: Actor,
}

pub struct CommentOutput {
    pub task: TaskId,
    pub oid: Oid,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{WorkflowConfig, WorkflowState};
    use git_mile_core::TaskFilter;
    use git_mile_core::event::{Event, EventKind};
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    #[derive(Clone, Default)]
    struct MockStore {
        inner: Arc<MockStoreInner>,
    }

    #[derive(Default)]
    struct MockStoreInner {
        appended: Mutex<Vec<Event>>,
        load_calls: Mutex<Vec<TaskId>>,
        fail_on_load: Mutex<HashSet<TaskId>>,
        list: Mutex<Vec<TaskId>>,
        list_calls: Mutex<u32>,
        events: Mutex<HashMap<TaskId, Vec<Event>>>,
        next_oid: Mutex<u8>,
    }

    impl TaskStore for MockStore {
        type Error = anyhow::Error;

        fn task_exists(&self, task: TaskId) -> Result<bool, Self::Error> {
            if guard(&self.inner.fail_on_load).contains(&task) {
                return Err(anyhow!("missing task {task}"));
            }
            Ok(guard(&self.inner.events).contains_key(&task))
        }

        fn append_event(&self, event: &Event) -> Result<Oid, Self::Error> {
            guard(&self.inner.appended).push(event.clone());
            guard(&self.inner.events)
                .entry(event.task)
                .or_default()
                .push(event.clone());
            let oid = {
                let mut counter = guard(&self.inner.next_oid);
                let oid = fake_oid(*counter);
                *counter = counter.wrapping_add(1);
                oid
            };
            Ok(oid)
        }

        fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
            guard(&self.inner.load_calls).push(task);
            if guard(&self.inner.fail_on_load).contains(&task) {
                return Err(anyhow!("missing task {task}"));
            }
            Ok(guard(&self.inner.events).get(&task).cloned().unwrap_or_default())
        }

        fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
            *guard(&self.inner.list_calls) += 1;
            Ok(guard(&self.inner.list).clone())
        }

        fn list_tasks_modified_since(
            &self,
            _since: time::OffsetDateTime,
        ) -> Result<Vec<TaskId>, Self::Error> {
            // For testing, return all tasks
            self.list_tasks()
        }
    }

    impl MockStore {
        fn appended(&self) -> Vec<Event> {
            guard(&self.inner.appended).clone()
        }

        fn fail_on_load(&self, task: TaskId) {
            guard(&self.inner.fail_on_load).insert(task);
        }

        fn set_list(&self, ids: Vec<TaskId>) {
            *guard(&self.inner.list) = ids;
        }

        fn set_events(&self, task: TaskId, events: Vec<Event>) {
            guard(&self.inner.events).insert(task, events);
        }
    }

    fn fake_oid(counter: u8) -> Oid {
        let mut bytes = [0u8; 20];
        bytes[19] = counter;
        Oid::from_bytes(&bytes).unwrap_or_else(|_| unreachable!("fixed-length byte array"))
    }

    fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sample_actor() -> Actor {
        Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        }
    }

    fn service_with_store() -> (
        TaskService<std::sync::Arc<MockStore>>,
        crate::task_repository::TaskRepository<std::sync::Arc<MockStore>>,
        MockStore,
    ) {
        let store = MockStore::default();
        let store_arc = std::sync::Arc::new(store.clone());
        let store_arc_arc = std::sync::Arc::new(std::sync::Arc::clone(&store_arc));
        let repository = crate::task_repository::TaskRepository::new(store_arc_arc);
        let service = TaskService::new(
            store_arc,
            WorkflowConfig::unrestricted(),
            HooksConfig::default(),
            PathBuf::from("/tmp/.git-mile"),
        );
        (service, repository, store)
    }

    #[test]
    fn create_task_links_parents_and_validates_existence() -> Result<()> {
        let (service, _repository, store) = service_with_store();

        // First create a parent task
        let parent_output = service.create_with_parents(CreateTaskInput {
            title: "parent".into(),
            state: None,
            labels: vec![],
            assignees: vec![],
            description: None,
            parents: vec![],
            actor: sample_actor(),
        })?;
        let parent = parent_output.task;

        // Now create a child task with the parent
        let output = service.create_with_parents(CreateTaskInput {
            title: "task".into(),
            state: Some("doing".into()),
            labels: vec!["a".into()],
            assignees: vec!["dev".into()],
            description: Some("desc".into()),
            parents: vec![parent],
            actor: sample_actor(),
        })?;

        assert_eq!(output.parent_links.len(), 1);
        assert_eq!(output.parent_links[0].parent, parent);

        let events = store.appended();
        assert_eq!(events.len(), 4); // 1 for parent creation + 3 for child creation and linking
        match &events[0].kind {
            EventKind::TaskCreated { title, .. } => assert_eq!(title, "parent"),
            other => panic!("unexpected event kind: {other:?}"),
        }
        match &events[1].kind {
            EventKind::TaskCreated { title, .. } => assert_eq!(title, "task"),
            other => panic!("unexpected event kind: {other:?}"),
        }
        match &events[2].kind {
            EventKind::ChildLinked { parent: p, child } => {
                assert_eq!(*p, parent);
                assert_eq!(*child, output.task);
                assert_eq!(events[2].task, output.task);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
        match &events[3].kind {
            EventKind::ChildLinked { parent: p, child } => {
                assert_eq!(*p, parent);
                assert_eq!(*child, output.task);
                assert_eq!(events[3].task, parent);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn create_task_rejects_unknown_state_when_restricted() {
        let store = MockStore::default();
        let workflow = WorkflowConfig::from_states(vec![WorkflowState::new("state/ready")]);
        let service = TaskService::new(
            store,
            workflow,
            HooksConfig::default(),
            PathBuf::from("/tmp/.git-mile"),
        );

        let Err(err) = service.create_with_parents(CreateTaskInput {
            title: "task".into(),
            state: Some("state/done".into()),
            labels: vec![],
            assignees: vec![],
            description: None,
            parents: vec![],
            actor: sample_actor(),
        }) else {
            panic!("expected state validation error");
        };

        assert!(err.to_string().contains("state 'state/done'"));
    }

    #[test]
    fn create_task_applies_default_state_when_missing() -> Result<()> {
        let store = MockStore::default();
        let workflow = WorkflowConfig::from_states_with_default(
            vec![WorkflowState::new("state/ready")],
            Some("state/ready"),
        );
        let service = TaskService::new(
            store.clone(),
            workflow,
            HooksConfig::default(),
            PathBuf::from("/tmp/.git-mile"),
        );

        let output = service.create_with_parents(CreateTaskInput {
            title: "task".into(),
            state: None,
            labels: vec![],
            assignees: vec![],
            description: None,
            parents: vec![],
            actor: sample_actor(),
        })?;

        let events = store.appended();
        assert_eq!(events.len(), 1);
        match &events[0].kind {
            EventKind::TaskCreated { state, .. } => {
                assert_eq!(state.as_deref(), Some("state/ready"));
            }
            other => panic!("unexpected event: {other:?}"),
        }

        assert_eq!(output.parent_links.len(), 0);
        Ok(())
    }

    #[test]
    fn create_task_errors_when_parent_missing() {
        let (service, _repository, store) = service_with_store();
        let parent = TaskId::new();
        store.fail_on_load(parent);

        let result = service.create_with_parents(CreateTaskInput {
            title: "task".into(),
            state: None,
            labels: vec![],
            assignees: vec![],
            description: None,
            parents: vec![parent],
            actor: sample_actor(),
        });

        assert!(result.is_err());
    }

    #[test]
    fn add_comment_appends_event() -> Result<()> {
        let (service, _repository, store) = service_with_store();

        // First create a task
        let create_output = service.create_with_parents(CreateTaskInput {
            title: "task for comment".into(),
            state: None,
            labels: vec![],
            assignees: vec![],
            description: None,
            parents: vec![],
            actor: sample_actor(),
        })?;
        let task = create_output.task;

        // Now add a comment
        let output = service.add_comment(CommentInput {
            task,
            message: "hello".into(),
            actor: sample_actor(),
        })?;

        assert_eq!(output.task, task);
        let events = store.appended();
        assert_eq!(events.len(), 2); // 1 for task creation, 1 for comment
        match &events[0].kind {
            EventKind::TaskCreated { .. } => {}
            other => panic!("unexpected event kind: {other:?}"),
        }
        match &events[1].kind {
            EventKind::CommentAdded { body_md, .. } => assert_eq!(body_md, "hello"),
            other => panic!("unexpected event kind: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn list_snapshots_applies_filters() -> Result<()> {
        let (_service, repository, store) = service_with_store();
        let matching = TaskId::new();
        let skipped = TaskId::new();
        store.set_list(vec![matching, skipped]);

        let actor = sample_actor();
        let matching_event = Event::new(
            matching,
            &actor,
            EventKind::TaskCreated {
                title: "Docs".into(),
                labels: vec!["label/docs".into()],
                assignees: vec!["alice".into()],
                description: None,
                state: Some("state/todo".into()),
                state_kind: None,
            },
        );
        let skipped_event = Event::new(
            skipped,
            &actor,
            EventKind::TaskCreated {
                title: "Feature".into(),
                labels: vec!["label/feature".into()],
                assignees: vec!["bob".into()],
                description: None,
                state: Some("state/done".into()),
                state_kind: None,
            },
        );
        store.set_events(matching, vec![matching_event]);
        store.set_events(skipped, vec![skipped_event]);

        let mut filter = TaskFilter::default();
        filter.states.insert("state/todo".into());
        filter.labels.insert("label/docs".into());

        let snapshots = repository.list_snapshots(Some(&filter))?;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, matching);
        Ok(())
    }
}
