use std::str::FromStr;

use anyhow::{Context, Result};
use git2::Oid;
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_core::TaskSnapshot;
use git_mile_store_git::GitStore;

use crate::config::WorkflowConfig;
#[cfg(test)]
use crate::config::WorkflowState;
use crate::Command;

/// Minimal abstraction over the backing event store so command handlers can be unit-tested.
pub trait TaskRepository {
    fn append_event(&self, event: &Event) -> Result<Oid>;
    fn load_events(&self, task: TaskId) -> Result<Vec<Event>>;
    fn list_tasks(&self) -> Result<Vec<TaskId>>;
}

impl TaskRepository for GitStore {
    fn append_event(&self, event: &Event) -> Result<Oid> {
        Self::append_event(self, event)
    }

    fn load_events(&self, task: TaskId) -> Result<Vec<Event>> {
        Self::load_events(self, task)
    }

    fn list_tasks(&self) -> Result<Vec<TaskId>> {
        Self::list_tasks(self)
    }
}

/// Service façade that encapsulates all task-related side effects.
pub struct TaskService<S> {
    store: S,
    workflow: WorkflowConfig,
}

impl<S> TaskService<S> {
    pub const fn new(store: S, workflow: WorkflowConfig) -> Self {
        Self { store, workflow }
    }
}

impl<S: TaskRepository> TaskService<S> {
    fn create_with_parents(&self, input: CreateTaskInput) -> Result<CreateTaskOutput> {
        let CreateTaskInput {
            title,
            state,
            labels,
            assignees,
            description,
            parents,
            actor,
        } = input;

        self.workflow.validate_state(state.as_deref())?;

        let task = TaskId::new();
        let created_event = Event::new(
            task,
            &actor,
            EventKind::TaskCreated {
                title,
                labels,
                assignees,
                description,
                state,
            },
        );
        let created_event_oid = self.store.append_event(&created_event)?;

        let mut parent_links = Vec::new();
        for parent in parents {
            self.store
                .load_events(parent)
                .with_context(|| format!("Parent task not found: {parent}"))?;

            let link_event = Event::new(task, &actor, EventKind::ChildLinked { parent, child: task });
            let oid = self.store.append_event(&link_event)?;
            parent_links.push(ParentLink { parent, oid });
        }

        Ok(CreateTaskOutput {
            task,
            created_event_oid,
            parent_links,
        })
    }

    fn add_comment(&self, input: CommentInput) -> Result<CommentOutput> {
        let CommentInput {
            task, message, actor, ..
        } = input;
        let comment_event = Event::new(
            task,
            &actor,
            EventKind::CommentAdded {
                comment_id: EventId::new(),
                body_md: message,
            },
        );
        let oid = self.store.append_event(&comment_event)?;
        Ok(CommentOutput { task, oid })
    }

    fn materialize(&self, task: TaskId) -> Result<TaskSnapshot> {
        let events = self.store.load_events(task)?;
        Ok(TaskSnapshot::replay(&events))
    }

    fn list_tasks(&self) -> Result<Vec<TaskId>> {
        self.store.list_tasks()
    }
}

pub fn run<S: TaskRepository>(command: Command, service: &TaskService<S>) -> Result<()> {
    match command {
        Command::New {
            title,
            state,
            labels,
            assignees,
            description,
            parents,
            actor_name,
            actor_email,
        } => {
            let parent_ids = parse_task_ids(parents)?;
            let actor = Actor {
                name: actor_name,
                email: actor_email,
            };
            let output = service.create_with_parents(CreateTaskInput {
                title,
                state,
                labels,
                assignees,
                description,
                parents: parent_ids,
                actor,
            })?;

            println!("created task: {} ({})", output.task, output.created_event_oid);
            for link in output.parent_links {
                println!("linked to parent: {} ({})", link.parent, link.oid);
            }
        }
        Command::Comment {
            task,
            message,
            actor_name,
            actor_email,
        } => {
            let task = parse_task_id(&task)?;
            let actor = Actor {
                name: actor_name,
                email: actor_email,
            };
            let output = service.add_comment(CommentInput { task, message, actor })?;
            println!("commented: {} ({})", output.task, output.oid);
        }
        Command::Show { task } => {
            let task = parse_task_id(&task)?;
            let snapshot = service.materialize(task)?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
        }
        Command::Ls => {
            for id in service.list_tasks()? {
                println!("{id}");
            }
        }
        _ => unreachable!("Unhandled command routed to TaskService"),
    }

    Ok(())
}

fn parse_task_ids(inputs: Vec<String>) -> Result<Vec<TaskId>> {
    inputs.into_iter().map(|raw| parse_task_id(&raw)).collect()
}

fn parse_task_id(raw: &str) -> Result<TaskId> {
    TaskId::from_str(raw).with_context(|| format!("Invalid task id: {raw}"))
}

struct CreateTaskInput {
    title: String,
    state: Option<String>,
    labels: Vec<String>,
    assignees: Vec<String>,
    description: Option<String>,
    parents: Vec<TaskId>,
    actor: Actor,
}

struct CreateTaskOutput {
    task: TaskId,
    created_event_oid: Oid,
    parent_links: Vec<ParentLink>,
}

struct ParentLink {
    parent: TaskId,
    oid: Oid,
}

struct CommentInput {
    task: TaskId,
    message: String,
    actor: Actor,
}

struct CommentOutput {
    task: TaskId,
    oid: Oid,
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use git_mile_core::event::EventKind;
    use std::collections::HashSet;
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
        next_oid: Mutex<u8>,
    }

    impl TaskRepository for MockStore {
        fn append_event(&self, event: &Event) -> Result<Oid> {
            guard(&self.inner.appended).push(event.clone());
            let oid = {
                let mut counter = guard(&self.inner.next_oid);
                let oid = fake_oid(*counter);
                *counter = counter.wrapping_add(1);
                oid
            };
            Ok(oid)
        }

        fn load_events(&self, task: TaskId) -> Result<Vec<Event>> {
            guard(&self.inner.load_calls).push(task);
            if guard(&self.inner.fail_on_load).contains(&task) {
                return Err(anyhow!("missing task {task}"));
            }
            Ok(Vec::new())
        }

        fn list_tasks(&self) -> Result<Vec<TaskId>> {
            Ok(guard(&self.inner.list).clone())
        }
    }

    impl MockStore {
        fn appended(&self) -> Vec<Event> {
            guard(&self.inner.appended).clone()
        }

        fn fail_on_load(&self, task: TaskId) {
            guard(&self.inner.fail_on_load).insert(task);
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

    fn service_with_store() -> (TaskService<MockStore>, MockStore) {
        let store = MockStore::default();
        let service = TaskService::new(store.clone(), WorkflowConfig::default());
        (service, store)
    }

    #[test]
    fn create_task_links_parents_and_validates_existence() -> Result<()> {
        let (service, store) = service_with_store();
        let parent = TaskId::new();

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
        assert_eq!(events.len(), 2);
        match &events[0].kind {
            EventKind::TaskCreated { title, .. } => assert_eq!(title, "task"),
            other => panic!("unexpected event kind: {other:?}"),
        }
        match &events[1].kind {
            EventKind::ChildLinked { parent: p, child } => {
                assert_eq!(*p, parent);
                assert_eq!(*child, output.task);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn create_task_rejects_unknown_state_when_restricted() {
        let store = MockStore::default();
        let workflow = WorkflowConfig::from_states(vec![WorkflowState::new("state/ready")]);
        let service = TaskService::new(store, workflow);

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
    fn create_task_errors_when_parent_missing() {
        let (service, store) = service_with_store();
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
        let (service, store) = service_with_store();
        let task = TaskId::new();

        let output = service.add_comment(CommentInput {
            task,
            message: "hello".into(),
            actor: sample_actor(),
        })?;

        assert_eq!(output.task, task);
        let events = store.appended();
        assert_eq!(events.len(), 1);
        match &events[0].kind {
            EventKind::CommentAdded { body_md, .. } => assert_eq!(body_md, "hello"),
            other => panic!("unexpected event kind: {other:?}"),
        }
        Ok(())
    }
}
