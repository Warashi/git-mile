use std::str::FromStr;

use anyhow::{Context, Result};
use git_mile_core::event::Actor;
use git_mile_core::id::TaskId;
use git_mile_core::TaskFilter;

use crate::config::WorkflowConfig;
use crate::filter_util::TaskFilterBuilder;
use crate::{Command, LsFormat};

use super::service::{CommentInput, CreateTaskInput, TaskService};
use crate::task_repository::TaskRepository;
use crate::task_writer::TaskStore;

pub fn run<S: TaskStore, R: TaskStore>(
    command: Command,
    service: &TaskService<S>,
    repository: &TaskRepository<R>,
) -> Result<()> {
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
        } => handle_new(
            service,
            title,
            state,
            labels,
            assignees,
            description,
            parents,
            actor_name,
            actor_email,
        ),
        Command::Comment {
            task,
            message,
            actor_name,
            actor_email,
        } => handle_comment(service, &task, message, actor_name, actor_email),
        Command::Show { task } => handle_show(service, &task),
        Command::Ls {
            states,
            labels,
            assignees,
            state_kinds,
            exclude_state_kinds,
            parents,
            children,
            updated_since,
            updated_until,
            text,
            format,
        } => handle_ls(
            service,
            repository,
            states,
            labels,
            assignees,
            state_kinds,
            exclude_state_kinds,
            parents,
            children,
            updated_since,
            updated_until,
            text,
            format,
        ),
        _ => unreachable!("Unhandled command routed to TaskService"),
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_new<S: TaskStore>(
    service: &TaskService<S>,
    title: String,
    state: Option<String>,
    labels: Vec<String>,
    assignees: Vec<String>,
    description: Option<String>,
    parents: Vec<String>,
    actor_name: String,
    actor_email: String,
) -> Result<()> {
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
    Ok(())
}

fn handle_comment<S: TaskStore>(
    service: &TaskService<S>,
    task: &str,
    message: String,
    actor_name: String,
    actor_email: String,
) -> Result<()> {
    let task = parse_task_id(task)?;
    let actor = Actor {
        name: actor_name,
        email: actor_email,
    };
    let output = service.add_comment(CommentInput { task, message, actor })?;
    println!("commented: {} ({})", output.task, output.oid);
    Ok(())
}

fn handle_show<S: TaskStore>(service: &TaskService<S>, task: &str) -> Result<()> {
    let task = parse_task_id(task)?;
    let snapshot = service.materialize(task)?;
    println!("{}", serde_json::to_string_pretty(&snapshot)?);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn handle_ls<S: TaskStore, R: TaskStore>(
    service: &TaskService<S>,
    repository: &TaskRepository<R>,
    states: Vec<String>,
    labels: Vec<String>,
    assignees: Vec<String>,
    state_kinds: Vec<String>,
    exclude_state_kinds: Vec<String>,
    parents: Vec<String>,
    children: Vec<String>,
    updated_since: Option<String>,
    updated_until: Option<String>,
    text: Option<String>,
    format: LsFormat,
) -> Result<()> {
    let workflow = service.workflow();
    for state in &states {
        workflow.validate_state(Some(state))?;
    }

    let filter = build_filter(CliFilterArgs {
        states,
        labels,
        assignees,
        include_state_kinds: state_kinds,
        exclude_state_kinds,
        parents,
        children,
        updated_since,
        updated_until,
        text,
    })?;
    let filter_empty = filter.is_empty();
    let tasks = repository.list_snapshots(Some(&filter))?;

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
    Ok(())
}

struct CliFilterArgs {
    states: Vec<String>,
    labels: Vec<String>,
    assignees: Vec<String>,
    include_state_kinds: Vec<String>,
    exclude_state_kinds: Vec<String>,
    parents: Vec<String>,
    children: Vec<String>,
    updated_since: Option<String>,
    updated_until: Option<String>,
    text: Option<String>,
}

fn build_filter(args: CliFilterArgs) -> Result<TaskFilter> {
    let CliFilterArgs {
        states,
        labels,
        assignees,
        include_state_kinds,
        exclude_state_kinds,
        parents,
        children,
        updated_since,
        updated_until,
        text,
    } = args;

    let parent_ids = parse_task_ids(parents)?;
    let child_ids = parse_task_ids(children)?;

    let mut builder = TaskFilterBuilder::new()
        .with_states(&states)
        .with_labels(&labels)
        .with_assignees(&assignees)
        .with_parents(&parent_ids)
        .with_children(&child_ids);

    builder = builder.with_state_kinds(&include_state_kinds, &exclude_state_kinds)?;
    builder = builder.with_text(text);
    builder = builder.with_time_range(updated_since, updated_until)?;

    Ok(builder.build())
}

fn render_task_table(tasks: &[git_mile_core::TaskSnapshot], workflow: &WorkflowConfig) {
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

fn parse_task_ids(inputs: Vec<String>) -> Result<Vec<TaskId>> {
    inputs.into_iter().map(|raw| parse_task_id(&raw)).collect()
}

fn parse_task_id(raw: &str) -> Result<TaskId> {
    TaskId::from_str(raw).with_context(|| format!("Invalid task id: {raw}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, LsFormat};
    use anyhow::{Result, anyhow};
    use git_mile_core::event::{Event, EventKind};
    use git_mile_core::StateKind;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use crate::config::WorkflowConfig;
    use crate::task_writer::TaskStore;

    use super::super::service::TaskService;

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

        fn append_event(&self, event: &Event) -> Result<git2::Oid, Self::Error> {
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

        fn set_list(&self, ids: Vec<TaskId>) {
            *guard(&self.inner.list) = ids;
        }

        fn list_calls(&self) -> u32 {
            *guard(&self.inner.list_calls)
        }

        fn load_calls(&self) -> Vec<TaskId> {
            guard(&self.inner.load_calls).clone()
        }
    }

    fn fake_oid(counter: u8) -> git2::Oid {
        let mut bytes = [0u8; 20];
        bytes[19] = counter;
        git2::Oid::from_bytes(&bytes).unwrap_or_else(|_| unreachable!("fixed-length byte array"))
    }

    fn guard<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
        mutex.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn service_with_store() -> (TaskService<std::sync::Arc<MockStore>>, TaskRepository<std::sync::Arc<MockStore>>, MockStore) {
        let store = MockStore::default();
        let store_arc = std::sync::Arc::new(store.clone());
        let store_arc_arc = std::sync::Arc::new(std::sync::Arc::clone(&store_arc));
        let repository = TaskRepository::new(store_arc_arc);
        let service = TaskService::new(store_arc, WorkflowConfig::unrestricted());
        (service, repository, store)
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
    fn build_filter_trims_text_input() -> Result<()> {
        let filter = build_filter(CliFilterArgs {
            states: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            include_state_kinds: Vec::new(),
            exclude_state_kinds: Vec::new(),
            parents: Vec::new(),
            children: Vec::new(),
            updated_since: None,
            updated_until: None,
            text: Some("  panic at the disco  ".into()),
        })?;
        assert_eq!(filter.text.as_deref(), Some("panic at the disco"));
        Ok(())
    }

    #[test]
    fn build_filter_discards_blank_text() -> Result<()> {
        let filter = build_filter(CliFilterArgs {
            states: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            include_state_kinds: Vec::new(),
            exclude_state_kinds: Vec::new(),
            parents: Vec::new(),
            children: Vec::new(),
            updated_since: None,
            updated_until: None,
            text: Some("   ".into()),
        })?;
        assert!(filter.text.is_none());
        Ok(())
    }

    #[test]
    fn build_filter_applies_state_kinds_and_parents() -> Result<()> {
        let parent = TaskId::new();
        let child = TaskId::new();
        let filter = build_filter(CliFilterArgs {
            states: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            include_state_kinds: vec!["todo".into()],
            exclude_state_kinds: vec!["done".into()],
            parents: vec![parent.to_string()],
            children: vec![child.to_string()],
            updated_since: Some("2024-01-01T00:00:00Z".into()),
            updated_until: None,
            text: None,
        })?;

        assert!(filter.parents.contains(&parent));
        assert!(filter.children.contains(&child));
        assert!(filter.state_kinds.include.contains(&StateKind::Todo));
        assert!(filter.state_kinds.exclude.contains(&StateKind::Done));
        assert!(filter.updated.is_some());
        Ok(())
    }

    #[test]
    fn build_filter_rejects_invalid_state_kind() {
        let Err(err) = build_filter(CliFilterArgs {
            states: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            include_state_kinds: vec!["unknown".into()],
            exclude_state_kinds: Vec::new(),
            parents: Vec::new(),
            children: Vec::new(),
            updated_since: None,
            updated_until: None,
            text: None,
        }) else {
            panic!("filter should reject invalid state kind");
        };
        assert!(err.to_string().contains("invalid state kind"));
    }

    #[test]
    fn build_filter_rejects_invalid_timestamp() {
        let Err(err) = build_filter(CliFilterArgs {
            states: Vec::new(),
            labels: Vec::new(),
            assignees: Vec::new(),
            include_state_kinds: Vec::new(),
            exclude_state_kinds: Vec::new(),
            parents: Vec::new(),
            children: Vec::new(),
            updated_since: Some("not-a-timestamp".into()),
            updated_until: None,
            text: None,
        }) else {
            panic!("filter should reject timestamp");
        };
        assert!(err.to_string().contains("invalid updated_since timestamp"));
    }

    #[test]
    fn run_new_dispatches_to_service() -> Result<()> {
        let (service, repository, store) = service_with_store();
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
            &repository,
        )?;

        let events = store.appended();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0].kind, EventKind::TaskCreated { .. }));
        Ok(())
    }

    #[test]
    fn run_comment_dispatches_to_service() -> Result<()> {
        let (service, repository, store) = service_with_store();

        // First create a task
        run(
            Command::New {
                title: "task for comment".into(),
                state: None,
                labels: vec![],
                assignees: vec![],
                description: None,
                parents: vec![],
                actor_name: "alice".into(),
                actor_email: "alice@example.invalid".into(),
            },
            &service,
            &repository,
        )?;

        // Get the created task ID
        let created_events = store.appended();
        let task = created_events[0].task;

        // Now add a comment
        run(
            Command::Comment {
                task: task.to_string(),
                message: "from run".into(),
                actor_name: "alice".into(),
                actor_email: "alice@example.invalid".into(),
            },
            &service,
            &repository,
        )?;

        let events = store.appended();
        assert_eq!(events.len(), 2); // 1 for task creation, 1 for comment
        assert!(matches!(events[0].kind, EventKind::TaskCreated { .. }));
        assert!(matches!(events[1].kind, EventKind::CommentAdded { .. }));
        Ok(())
    }

    #[test]
    fn run_ls_lists_all_tasks() -> Result<()> {
        let (service, repository, store) = service_with_store();
        let task = TaskId::new();
        store.set_list(vec![task]);
        run(
            Command::Ls {
                states: vec![],
                labels: vec![],
                assignees: vec![],
                state_kinds: vec![],
                exclude_state_kinds: vec![],
                parents: vec![],
                children: vec![],
                updated_since: None,
                updated_until: None,
                text: None,
                format: LsFormat::Table,
            },
            &service,
            &repository,
        )?;
        assert_eq!(store.list_calls(), 1);
        assert_eq!(store.load_calls(), vec![task]);
        Ok(())
    }

    #[test]
    fn run_show_materializes_snapshot() -> Result<()> {
        let (service, repository, store) = service_with_store();
        let task = TaskId::new();
        run(
            Command::Show {
                task: task.to_string(),
            },
            &service,
            &repository,
        )?;

        let calls = store.load_calls();
        assert_eq!(calls, vec![task]);
        Ok(())
    }
}
