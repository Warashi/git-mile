use std::cmp::Ordering;
use std::io::Write;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, anyhow};
use git_mile_core::TaskFilter;
use git_mile_core::event::Event;
use git_mile_core::id::{EventId, TaskId};
use git_mile_core::{StateKind, TaskSnapshot};

use crate::event_log::{
    entries_from_events, format_actor, format_timestamp, single_line_detail, truncate_detail,
};
use crate::mcp::{TaskCommentEntry, WorkflowStateEntry, WorkflowStatesResponse};
use crate::{Command, LogFormat, LsFormat, OutputFormat};
use git_mile_app::actor_from_params_or_default;
use git_mile_app::{
    CommentInput, CreateTaskInput, DescriptionPatch, SetDiff, StatePatch, TaskFilterBuilder, TaskRepository,
    TaskService, TaskStore, TaskUpdate, WorkflowConfig,
};

pub fn run<S: TaskStore, R: TaskStore>(
    command: Command,
    service: &TaskService<S>,
    repository: &TaskRepository<R>,
    repo_root: &Path,
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
            actor_name.as_deref(),
            actor_email.as_deref(),
            repo_root,
        ),
        Command::Comment {
            task,
            message,
            actor_name,
            actor_email,
        } => handle_comment(
            service,
            &task,
            message,
            actor_name.as_deref(),
            actor_email.as_deref(),
            repo_root,
        ),
        Command::Log { task, format } => handle_log(service, &task, format, &mut std::io::stdout()),
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
        Command::ListComments { task, format } => {
            handle_list_comments(repository, &task, format, &mut std::io::stdout())
        }
        Command::ListSubtasks { parent_task, format } => {
            handle_list_subtasks(service, repository, &parent_task, format, &mut std::io::stdout())
        }
        Command::ListWorkflowStates { format } => {
            handle_list_workflow_states(service, format, &mut std::io::stdout())
        }
        Command::UpdateTask {
            task,
            title,
            description,
            state,
            clear_state,
            add_labels,
            remove_labels,
            add_assignees,
            remove_assignees,
            link_parents,
            unlink_parents,
            actor_name,
            actor_email,
            format,
        } => handle_update_task(
            service,
            task,
            title,
            description,
            state,
            clear_state,
            add_labels,
            remove_labels,
            add_assignees,
            remove_assignees,
            link_parents,
            unlink_parents,
            actor_name.as_deref(),
            actor_email.as_deref(),
            repo_root,
            format,
            &mut std::io::stdout(),
        ),
        Command::UpdateComment {
            task,
            comment,
            body,
            actor_name,
            actor_email,
            format,
        } => handle_update_comment(
            service,
            &task,
            &comment,
            body,
            actor_name.as_deref(),
            actor_email.as_deref(),
            repo_root,
            format,
            &mut std::io::stdout(),
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
    actor_name: Option<&str>,
    actor_email: Option<&str>,
    repo_root: &Path,
) -> Result<()> {
    let parent_ids = parse_task_ids(parents)?;
    let actor = actor_from_params_or_default(actor_name, actor_email, repo_root);
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
    actor_name: Option<&str>,
    actor_email: Option<&str>,
    repo_root: &Path,
) -> Result<()> {
    let task = parse_task_id(task)?;
    let actor = actor_from_params_or_default(actor_name, actor_email, repo_root);
    let output = service.add_comment(CommentInput { task, message, actor })?;
    println!("commented: {} ({})", output.task, output.oid);
    Ok(())
}

fn handle_log<S: TaskStore>(
    service: &TaskService<S>,
    task: &str,
    format: LogFormat,
    writer: &mut dyn Write,
) -> Result<()> {
    let task = parse_task_id(task)?;
    let events = service.event_log(task)?;
    match format {
        LogFormat::Table => render_log_table(&events, writer),
        LogFormat::Json => {
            let json = serde_json::to_string_pretty(&events)?;
            writeln!(writer, "{json}")?;
            Ok(())
        }
    }
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
        LsFormat::Table => render_task_table(&tasks, workflow, &mut std::io::stdout())?,
        LsFormat::Json => println!("{}", serde_json::to_string_pretty(&tasks)?),
    }
    Ok(())
}

fn handle_list_comments<R: TaskStore>(
    repository: &TaskRepository<R>,
    task: &str,
    format: OutputFormat,
    writer: &mut dyn Write,
) -> Result<()> {
    let task_id = parse_task_id(task)?;
    let view = repository.get_view(task_id)?;
    let entries = view
        .comments
        .iter()
        .map(|comment| TaskCommentEntry {
            comment_id: comment.id.to_string(),
            actor: comment.actor.clone(),
            body_md: comment.body.clone(),
            created_at: format_timestamp(comment.created_at),
            updated_at: comment.updated_at.map(format_timestamp),
        })
        .collect::<Vec<_>>();

    match format {
        OutputFormat::Table => {
            writeln!(writer, "CommentId | Actor | Created | Updated | Body")?;
            writeln!(writer, "--------- | ----- | ------- | ------- | ----")?;
            for entry in &entries {
                let actor = format_actor(&entry.actor);
                let updated = entry.updated_at.as_deref().unwrap_or("-");
                let body = truncate_detail(&single_line_detail(&entry.body_md), 80);
                writeln!(
                    writer,
                    "{} | {} | {} | {} | {}",
                    entry.comment_id, actor, entry.created_at, updated, body
                )?;
            }
            Ok(())
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&entries)?;
            writeln!(writer, "{json}")?;
            Ok(())
        }
    }
}

fn handle_list_subtasks<S: TaskStore, R: TaskStore>(
    service: &TaskService<S>,
    repository: &TaskRepository<R>,
    parent_task: &str,
    format: OutputFormat,
    writer: &mut dyn Write,
) -> Result<()> {
    let parent_task_id = parse_task_id(parent_task)?;
    repository.get_snapshot(parent_task_id)?;
    let child_ids = repository.list_children(parent_task_id)?;

    let mut subtasks = Vec::with_capacity(child_ids.len());
    for child in child_ids {
        subtasks.push(repository.get_snapshot(child)?);
    }
    subtasks.sort_by(compare_snapshots);

    match format {
        OutputFormat::Table => render_task_table(&subtasks, service.workflow(), writer),
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&subtasks)?;
            writeln!(writer, "{json}")?;
            Ok(())
        }
    }
}

fn handle_list_workflow_states<S: TaskStore>(
    service: &TaskService<S>,
    format: OutputFormat,
    writer: &mut dyn Write,
) -> Result<()> {
    let workflow = service.workflow();
    let response = WorkflowStatesResponse {
        restricted: workflow.is_restricted(),
        default_state: workflow.default_state().map(str::to_owned),
        states: workflow
            .states()
            .iter()
            .map(|state| WorkflowStateEntry {
                value: state.value().to_owned(),
                label: state.label().map(str::to_owned),
                kind: state.kind(),
            })
            .collect(),
    };

    match format {
        OutputFormat::Table => {
            writeln!(writer, "Restricted | {}", response.restricted)?;
            writeln!(
                writer,
                "DefaultState | {}",
                response.default_state.as_deref().unwrap_or("-")
            )?;
            writeln!(writer, "Value | Label | Kind | Default")?;
            writeln!(writer, "----- | ----- | ---- | -------")?;
            for state in &response.states {
                let label = state.label.as_deref().unwrap_or("-");
                let kind = format_state_kind(state.kind.as_ref());
                let is_default = if response.default_state.as_deref() == Some(state.value.as_str()) {
                    "yes"
                } else {
                    "no"
                };
                writeln!(writer, "{} | {} | {} | {}", state.value, label, kind, is_default)?;
            }
            Ok(())
        }
        OutputFormat::Json => {
            let json = serde_json::to_string_pretty(&response)?;
            writeln!(writer, "{json}")?;
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_update_task<S: TaskStore>(
    service: &TaskService<S>,
    task: String,
    title: Option<String>,
    description: Option<String>,
    state: Option<String>,
    clear_state: bool,
    add_labels: Vec<String>,
    remove_labels: Vec<String>,
    add_assignees: Vec<String>,
    remove_assignees: Vec<String>,
    link_parents: Vec<String>,
    unlink_parents: Vec<String>,
    actor_name: Option<&str>,
    actor_email: Option<&str>,
    repo_root: &Path,
    format: OutputFormat,
    writer: &mut dyn Write,
) -> Result<()> {
    if let Some(state_value) = state.as_deref() {
        service.workflow().validate_state(Some(state_value))?;
    }

    let task_id = parse_task_id(&task)?;
    let link_parent_ids = parse_task_ids(link_parents)?;
    let unlink_parent_ids = parse_task_ids(unlink_parents)?;
    let actor = actor_from_params_or_default(actor_name, actor_email, repo_root);

    let patch = TaskUpdate {
        title,
        state: state.map_or_else(
            || {
                if clear_state {
                    Some(StatePatch::Clear)
                } else {
                    None
                }
            },
            |value| Some(StatePatch::Set { state: value }),
        ),
        description: description.map(|body| DescriptionPatch::Set { description: body }),
        labels: SetDiff {
            added: add_labels,
            removed: remove_labels,
        },
        assignees: SetDiff {
            added: add_assignees,
            removed: remove_assignees,
        },
    };

    service.update_task(task_id, patch, &link_parent_ids, &unlink_parent_ids, &actor)?;
    let snapshot = service.materialize(task_id)?;
    match format {
        OutputFormat::Table => render_task_table(&[snapshot], service.workflow(), writer),
        OutputFormat::Json => {
            writeln!(writer, "{}", serde_json::to_string_pretty(&snapshot)?)?;
            Ok(())
        }
    }
}

fn handle_update_comment<S: TaskStore>(
    service: &TaskService<S>,
    task: &str,
    comment: &str,
    body: String,
    actor_name: Option<&str>,
    actor_email: Option<&str>,
    repo_root: &Path,
    format: OutputFormat,
    writer: &mut dyn Write,
) -> Result<()> {
    let task_id = parse_task_id(task)?;
    let comment_id = parse_event_id(comment)?;
    let actor = actor_from_params_or_default(actor_name, actor_email, repo_root);

    service.update_comment(task_id, comment_id, body, &actor)?;

    match format {
        OutputFormat::Table => {
            writeln!(writer, "TaskId | CommentId | Status")?;
            writeln!(writer, "------ | --------- | ------")?;
            writeln!(writer, "{task_id} | {comment_id} | updated")?;
            Ok(())
        }
        OutputFormat::Json => {
            let payload = serde_json::json!({
                "task_id": task_id.to_string(),
                "comment_id": comment_id.to_string(),
                "status": "updated"
            });
            writeln!(writer, "{}", serde_json::to_string_pretty(&payload)?)?;
            Ok(())
        }
    }
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

    builder.build().map_err(|err| anyhow!(err))
}

fn render_task_table(
    tasks: &[TaskSnapshot],
    workflow: &WorkflowConfig,
    writer: &mut dyn Write,
) -> Result<()> {
    writeln!(writer, "ID | State | Title | Labels | Assignees | Updated")?;
    writeln!(writer, "-- | ----- | ----- | ------ | --------- | -------")?;

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

        writeln!(
            writer,
            "{} | {} | {} | {} | {} | {}",
            snapshot.id, state_display, snapshot.title, labels, assignees, updated
        )?;
    }
    Ok(())
}

fn render_log_table(events: &[Event], writer: &mut dyn Write) -> Result<()> {
    writeln!(writer, "Timestamp | Actor | Event | Detail | EventId")?;
    writeln!(writer, "--------- | ----- | ----- | ------ | -------")?;

    for entry in entries_from_events(events) {
        let ts = format_timestamp(entry.ts);
        let actor = format_actor(&entry.actor);
        let detail = entry.detail.as_deref().map_or_else(
            || "-".to_owned(),
            |text| truncate_detail(&single_line_detail(text), 80),
        );
        writeln!(
            writer,
            "{ts} | {actor} | {} | {detail} | {}",
            entry.action, entry.id
        )?;
    }
    Ok(())
}

fn compare_snapshots(a: &TaskSnapshot, b: &TaskSnapshot) -> Ordering {
    match (a.updated_at(), b.updated_at()) {
        (Some(left), Some(right)) => right.cmp(&left),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.id.cmp(&b.id),
    }
}

fn format_state_kind(kind: Option<&StateKind>) -> String {
    kind.map_or_else(|| "-".to_owned(), |state_kind| state_kind.as_str().to_owned())
}

fn parse_task_ids(inputs: Vec<String>) -> Result<Vec<TaskId>> {
    inputs.into_iter().map(|raw| parse_task_id(&raw)).collect()
}

fn parse_task_id(raw: &str) -> Result<TaskId> {
    TaskId::from_str(raw).with_context(|| format!("Invalid task id: {raw}"))
}

fn parse_event_id(raw: &str) -> Result<EventId> {
    EventId::from_str(raw).with_context(|| format!("Invalid comment id: {raw}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Command, LogFormat, LsFormat};
    use anyhow::{Context, Result, anyhow};
    use git_mile_core::StateKind;
    use git_mile_core::event::{Actor, Event, EventKind};
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

    use git_mile_app::{TaskService, TaskStore, WorkflowConfig};

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
            {
                let mut list = guard(&self.inner.list);
                if !list.contains(&event.task) {
                    list.push(event.task);
                }
            }
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

        fn set_events(&self, task: TaskId, events: Vec<Event>) {
            guard(&self.inner.events).insert(task, events);
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

    fn sample_actor() -> Actor {
        Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        }
    }

    fn service_with_store() -> (
        TaskService<std::sync::Arc<MockStore>>,
        TaskRepository<std::sync::Arc<MockStore>>,
        MockStore,
    ) {
        let store = MockStore::default();
        let store_arc = std::sync::Arc::new(store.clone());
        let store_arc_arc = std::sync::Arc::new(std::sync::Arc::clone(&store_arc));
        let repository = TaskRepository::new(store_arc_arc);
        let service = TaskService::new(
            store_arc,
            WorkflowConfig::unrestricted(),
            git_mile_app::HooksConfig::default(),
            std::path::PathBuf::from("/tmp/.git-mile"),
        );
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
    fn parse_event_id_rejects_invalid_value() {
        let Err(err) = parse_event_id("not-an-event-id") else {
            panic!("expected invalid id error");
        };
        assert!(err.to_string().contains("Invalid comment id"));
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
    fn handle_log_outputs_ordered_table() -> Result<()> {
        let (service, _repository, store) = service_with_store();
        let task = TaskId::new();
        let actor = sample_actor();

        let mut later = Event::new(
            task,
            &actor,
            EventKind::TaskTitleSet {
                title: "later".into(),
            },
        );
        later.lamport = 3;

        let mut earlier = Event::new(task, &actor, EventKind::TaskStateCleared);
        earlier.lamport = 2;

        store.set_events(task, vec![later.clone(), earlier.clone()]);

        let mut output = Vec::new();
        super::handle_log(&service, &task.to_string(), LogFormat::Table, &mut output)?;
        let text = String::from_utf8(output).context("log output must be utf8")?;
        let earlier_idx = text.find(&earlier.id.to_string()).context("earlier event id")?;
        let later_idx = text.find(&later.id.to_string()).context("later event id")?;

        assert!(earlier_idx < later_idx, "events must appear in lamport order");
        assert!(text.contains("Timestamp | Actor | Event | Detail | EventId"));
        assert_eq!(store.load_calls(), vec![task]);
        Ok(())
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
                actor_name: Some("run".into()),
                actor_email: Some("run@example.invalid".into()),
            },
            &service,
            &repository,
            Path::new("."),
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
                actor_name: Some("alice".into()),
                actor_email: Some("alice@example.invalid".into()),
            },
            &service,
            &repository,
            Path::new("."),
        )?;

        // Get the created task ID
        let created_events = store.appended();
        let task = created_events[0].task;

        // Now add a comment
        run(
            Command::Comment {
                task: task.to_string(),
                message: "from run".into(),
                actor_name: Some("alice".into()),
                actor_email: Some("alice@example.invalid".into()),
            },
            &service,
            &repository,
            Path::new("."),
        )?;

        let events = store.appended();
        assert_eq!(events.len(), 2); // 1 for task creation, 1 for comment
        assert!(matches!(events[0].kind, EventKind::TaskCreated { .. }));
        assert!(matches!(events[1].kind, EventKind::CommentAdded { .. }));
        Ok(())
    }

    #[test]
    fn run_update_task_dispatches_to_service() -> Result<()> {
        let (service, repository, store) = service_with_store();

        let created = service.create_with_parents(git_mile_app::CreateTaskInput {
            title: "before".into(),
            state: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            parents: Vec::new(),
            actor: sample_actor(),
        })?;

        run(
            Command::UpdateTask {
                task: created.task.to_string(),
                title: Some("after".into()),
                description: None,
                state: None,
                clear_state: false,
                add_labels: Vec::new(),
                remove_labels: Vec::new(),
                add_assignees: Vec::new(),
                remove_assignees: Vec::new(),
                link_parents: Vec::new(),
                unlink_parents: Vec::new(),
                actor_name: Some("alice".into()),
                actor_email: Some("alice@example.invalid".into()),
                format: OutputFormat::Table,
            },
            &service,
            &repository,
            Path::new("."),
        )?;

        let events = store.appended();
        assert!(
            events
                .iter()
                .any(|event| matches!(event.kind, EventKind::TaskTitleSet { ref title } if title == "after"))
        );
        Ok(())
    }

    #[test]
    fn run_update_comment_dispatches_to_service() -> Result<()> {
        let (service, repository, store) = service_with_store();
        let created = service.create_with_parents(git_mile_app::CreateTaskInput {
            title: "task".into(),
            state: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            parents: Vec::new(),
            actor: sample_actor(),
        })?;
        service.add_comment(git_mile_app::CommentInput {
            task: created.task,
            message: "before".into(),
            actor: sample_actor(),
        })?;
        let comment_id = store
            .appended()
            .iter()
            .find_map(|event| match &event.kind {
                EventKind::CommentAdded { comment_id, .. } => Some(*comment_id),
                _ => None,
            })
            .ok_or_else(|| anyhow!("comment id not found"))?;

        run(
            Command::UpdateComment {
                task: created.task.to_string(),
                comment: comment_id.to_string(),
                body: "after".into(),
                actor_name: Some("alice".into()),
                actor_email: Some("alice@example.invalid".into()),
                format: OutputFormat::Json,
            },
            &service,
            &repository,
            Path::new("."),
        )?;

        let events = store.appended();
        assert!(events.iter().any(
            |event| matches!(event.kind, EventKind::CommentUpdated { ref body_md, .. } if body_md == "after")
        ));
        Ok(())
    }

    #[test]
    fn run_update_comment_rejects_missing_comment() -> Result<()> {
        let (service, repository, _store) = service_with_store();
        let created = service.create_with_parents(git_mile_app::CreateTaskInput {
            title: "task".into(),
            state: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            parents: Vec::new(),
            actor: sample_actor(),
        })?;

        let err = run(
            Command::UpdateComment {
                task: created.task.to_string(),
                comment: EventId::new().to_string(),
                body: "after".into(),
                actor_name: Some("alice".into()),
                actor_email: Some("alice@example.invalid".into()),
                format: OutputFormat::Json,
            },
            &service,
            &repository,
            Path::new("."),
        )
        .expect_err("must reject missing comment");
        assert!(err.to_string().contains("not found"));
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
            Path::new("."),
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
            Path::new("."),
        )?;

        let calls = store.load_calls();
        assert_eq!(calls, vec![task]);
        Ok(())
    }

    #[test]
    fn handle_list_comments_outputs_json_entries() -> Result<()> {
        let (service, repository, _store) = service_with_store();
        let actor = sample_actor();
        let created = service.create_with_parents(git_mile_app::CreateTaskInput {
            title: "task with comments".into(),
            state: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            parents: Vec::new(),
            actor: actor.clone(),
        })?;
        let task = created.task;
        service.add_comment(git_mile_app::CommentInput {
            task,
            message: "first comment".into(),
            actor,
        })?;

        let mut output = Vec::new();
        super::handle_list_comments(&repository, &task.to_string(), OutputFormat::Json, &mut output)?;

        let text = String::from_utf8(output).context("comment output must be utf8")?;
        let comments: Vec<TaskCommentEntry> = serde_json::from_str(&text)?;
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].body_md, "first comment");
        Ok(())
    }

    #[test]
    fn handle_list_subtasks_orders_by_updated_desc() -> Result<()> {
        let (service, repository, _store) = service_with_store();
        let actor = sample_actor();
        let parent = service.create_with_parents(git_mile_app::CreateTaskInput {
            title: "parent".into(),
            state: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            parents: Vec::new(),
            actor: actor.clone(),
        })?;
        let child_one = service.create_with_parents(git_mile_app::CreateTaskInput {
            title: "child one".into(),
            state: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            parents: vec![parent.task],
            actor: actor.clone(),
        })?;
        let child_two = service.create_with_parents(git_mile_app::CreateTaskInput {
            title: "child two".into(),
            state: None,
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            parents: vec![parent.task],
            actor,
        })?;

        let mut output = Vec::new();
        super::handle_list_subtasks(
            &service,
            &repository,
            &parent.task.to_string(),
            OutputFormat::Json,
            &mut output,
        )?;
        let text = String::from_utf8(output).context("subtask output must be utf8")?;
        let subtasks: Vec<TaskSnapshot> = serde_json::from_str(&text)?;
        assert_eq!(subtasks.len(), 2);
        assert_eq!(subtasks[0].id, child_two.task);
        assert_eq!(subtasks[1].id, child_one.task);
        Ok(())
    }

    #[test]
    fn handle_list_workflow_states_outputs_json_shape() -> Result<()> {
        let (service, _repository, _store) = service_with_store();
        let mut output = Vec::new();

        super::handle_list_workflow_states(&service, OutputFormat::Json, &mut output)?;

        let text = String::from_utf8(output).context("workflow output must be utf8")?;
        let response: WorkflowStatesResponse = serde_json::from_str(&text)?;
        assert!(!response.restricted);
        assert!(response.default_state.is_none());
        Ok(())
    }
}
