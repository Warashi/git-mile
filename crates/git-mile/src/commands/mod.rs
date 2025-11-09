use std::{cmp::Ordering, str::FromStr};

use anyhow::{Context, Result, anyhow};
use git_mile_core::event::Actor;
use git_mile_core::id::TaskId;
use git_mile_core::{TaskFilter, TaskFilterBuilder, TaskSnapshot};
use git2::Oid;

use crate::config::WorkflowConfig;
#[cfg(test)]
use crate::config::WorkflowState;
use crate::task_writer::{CommentRequest, CreateTaskRequest, TaskStore, TaskWriter};
use crate::{Command, LsFormat};

/// Service façade that encapsulates all task-related side effects.
pub struct TaskService<S> {
    writer: TaskWriter<S>,
}

impl<S> TaskService<S> {
    pub const fn new(store: S, workflow: WorkflowConfig) -> Self
    where
        S: TaskStore,
    {
        Self {
            writer: TaskWriter::new(store, workflow),
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

    fn add_comment(&self, input: CommentInput) -> Result<CommentOutput> {
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

    fn materialize(&self, task: TaskId) -> Result<TaskSnapshot> {
        let events = self.store().load_events(task).map_err(Into::into)?;
        Ok(TaskSnapshot::replay(&events))
    }

    fn list_tasks(&self) -> Result<Vec<TaskId>> {
        self.store().list_tasks().map_err(Into::into)
    }

    fn list_snapshots(&self, filter: &TaskFilter) -> Result<Vec<TaskSnapshot>> {
        let mut snapshots = Vec::new();
        for task_id in self.list_tasks()? {
            let events = self.store().load_events(task_id).map_err(Into::into)?;
            snapshots.push(TaskSnapshot::replay(&events));
        }
        snapshots.sort_by(compare_snapshots);
        if filter.is_empty() {
            return Ok(snapshots);
        }
        Ok(snapshots
            .into_iter()
            .filter(|snapshot| filter.matches(snapshot))
            .collect())
    }
}

pub fn run<S: TaskStore>(command: Command, service: &TaskService<S>) -> Result<()> {
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
        Command::Ls {
            states,
            labels,
            assignees,
            text,
            format,
        } => {
            let workflow = service.workflow();
            for state in &states {
                workflow.validate_state(Some(state))?;
            }

            let filter = build_filter(states, labels, assignees, text);
            let filter_empty = filter.is_empty();
            let tasks = service.list_snapshots(&filter)?;

            if tasks.is_empty() {
                if filter_empty {
                    println!("No tasks found");
                } else {
                    println!("No tasks matched the provided filters");
                }
                return Ok(());
            }

            match format {
                LsFormat::Table => render_task_table(&tasks, workflow),
                LsFormat::Json => println!("{}", serde_json::to_string_pretty(&tasks)?),
            }
        }
        _ => unreachable!("Unhandled command routed to TaskService"),
    }

    Ok(())
}

fn build_filter(
    states: Vec<String>,
    labels: Vec<String>,
    assignees: Vec<String>,
    text: Option<String>,
) -> TaskFilter {
    let mut builder = TaskFilterBuilder::new()
        .states(states)
        .labels(labels)
        .assignees(assignees);
    if let Some(text) = text {
        builder = builder.text(text);
    }
    builder.build()
}

fn render_task_table(tasks: &[TaskSnapshot], workflow: &WorkflowConfig) {
    println!("ID | State | Title | Labels | Assignees | Updated");
    println!("-- | ----- | ----- | ------ | --------- | -------");

    for snapshot in tasks {
        let state_display = snapshot.state.as_deref().map_or_else(
            || workflow.display_label(None).to_string(),
            |value| {
                let label = workflow.display_label(Some(value));
                if label == value {
                    label.to_string()
                } else {
                    format!("{label} ({value})")
                }
            },
        );
        let labels = if snapshot.labels.is_empty() {
            "-".to_owned()
        } else {
            snapshot.labels.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        let assignees = if snapshot.assignees.is_empty() {
            "-".to_owned()
        } else {
            snapshot.assignees.iter().cloned().collect::<Vec<_>>().join(", ")
        };
        let updated = snapshot.updated_rfc3339.as_deref().unwrap_or("-").to_string();

        println!(
            "{} | {} | {} | {} | {} | {}",
            snapshot.id, state_display, snapshot.title, labels, assignees, updated
        );
    }
}

fn compare_snapshots(a: &TaskSnapshot, b: &TaskSnapshot) -> Ordering {
    match (a.updated_at(), b.updated_at()) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    }
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
    use crate::{Command, LsFormat};
    use anyhow::{Result, anyhow};
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

        fn list_calls(&self) -> u32 {
            *guard(&self.inner.list_calls)
        }

        fn load_calls(&self) -> Vec<TaskId> {
            guard(&self.inner.load_calls).clone()
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

    fn service_with_store() -> (TaskService<MockStore>, MockStore) {
        let store = MockStore::default();
        let service = TaskService::new(store.clone(), WorkflowConfig::unrestricted());
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
        assert_eq!(events.len(), 3);
        match &events[0].kind {
            EventKind::TaskCreated { title, .. } => assert_eq!(title, "task"),
            other => panic!("unexpected event kind: {other:?}"),
        }
        match &events[1].kind {
            EventKind::ChildLinked { parent: p, child } => {
                assert_eq!(*p, parent);
                assert_eq!(*child, output.task);
                assert_eq!(events[1].task, output.task);
            }
            other => panic!("unexpected event kind: {other:?}"),
        }
        match &events[2].kind {
            EventKind::ChildLinked { parent: p, child } => {
                assert_eq!(*p, parent);
                assert_eq!(*child, output.task);
                assert_eq!(events[2].task, parent);
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
    fn create_task_applies_default_state_when_missing() -> Result<()> {
        let store = MockStore::default();
        let workflow = WorkflowConfig::from_states_with_default(
            vec![WorkflowState::new("state/ready")],
            Some("state/ready"),
        );
        let service = TaskService::new(store.clone(), workflow);

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

    #[test]
    fn parse_task_ids_roundtrip() -> Result<()> {
        let ids = vec![TaskId::new(), TaskId::new()];
        let raw: Vec<_> = ids.iter().map(ToString::to_string).collect();
        let parsed = parse_task_ids(raw)?;
        assert_eq!(parsed, ids);
        Ok(())
    }

    #[test]
    fn parse_task_ids_rejects_invalid_value() {
        let Err(err) = parse_task_ids(vec!["not-a-task-id".into()]) else {
            panic!("expected invalid id error");
        };
        assert!(err.to_string().contains("Invalid task id"));
    }

    #[test]
    fn build_filter_trims_text_input() {
        let filter = build_filter(
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Some("  panic at the disco  ".into()),
        );
        assert_eq!(filter.text.as_deref(), Some("panic at the disco"));
    }

    #[test]
    fn build_filter_discards_blank_text() {
        let filter = build_filter(Vec::new(), Vec::new(), Vec::new(), Some("   ".into()));
        assert!(filter.text.is_none());
    }

    #[test]
    fn run_new_dispatches_to_service() -> Result<()> {
        let (service, store) = service_with_store();
        run(
            Command::New {
                title: "via run".into(),
                state: None,
                labels: vec![],
                assignees: vec![],
                description: None,
                parents: vec![],
                actor_name: "run".into(),
                actor_email: "run@example.invalid".into(),
            },
            &service,
        )?;

        let events = store.appended();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, EventKind::TaskCreated { .. }));
        Ok(())
    }

    #[test]
    fn run_comment_dispatches_to_service() -> Result<()> {
        let (service, store) = service_with_store();
        let task = TaskId::new();
        run(
            Command::Comment {
                task: task.to_string(),
                message: "from run".into(),
                actor_name: "alice".into(),
                actor_email: "alice@example.invalid".into(),
            },
            &service,
        )?;

        let events = store.appended();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, EventKind::CommentAdded { .. }));
        Ok(())
    }

    #[test]
    fn run_ls_lists_all_tasks() -> Result<()> {
        let (service, store) = service_with_store();
        let task = TaskId::new();
        store.set_list(vec![task]);
        run(
            Command::Ls {
                states: vec![],
                labels: vec![],
                assignees: vec![],
                text: None,
                format: LsFormat::Table,
            },
            &service,
        )?;
        assert_eq!(store.list_calls(), 1);
        assert_eq!(store.load_calls(), vec![task]);
        Ok(())
    }

    #[test]
    fn list_snapshots_applies_filters() -> Result<()> {
        let (service, store) = service_with_store();
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

        let snapshots = service.list_snapshots(&filter)?;
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].id, matching);
        Ok(())
    }

    #[test]
    fn run_show_materializes_snapshot() -> Result<()> {
        let (service, store) = service_with_store();
        let task = TaskId::new();
        run(
            Command::Show {
                task: task.to_string(),
            },
            &service,
        )?;

        let calls = store.load_calls();
        assert_eq!(calls, vec![task]);
        Ok(())
    }
}
