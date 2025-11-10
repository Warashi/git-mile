use super::app::*;
use super::clipboard::*;
use super::editor::*;
use super::ui::*;
use super::*;
use crate::config::{StateKind, WorkflowState};
use crate::task_cache::TaskView;
use crate::task_writer::TaskStore;
use anyhow::{Result, anyhow};
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::{EventId, TaskId};
use git_mile_core::{TaskFilter, TaskSnapshot};
use git2::Oid;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::Display;
use std::rc::Rc;
use std::result::Result as StdResult;
use std::str::FromStr;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

fn expect_ok<T, E: Display>(result: StdResult<T, E>, ctx: &str) -> T {
    match result {
        Ok(value) => value,
        Err(err) => panic!("{ctx}: {err}"),
    }
}

fn expect_err<T, E: Display>(result: StdResult<T, E>, ctx: &str) -> E {
    match result {
        Ok(_) => panic!("{ctx}"),
        Err(err) => err,
    }
}

fn expect_some<T>(value: Option<T>, ctx: &str) -> T {
    value.map_or_else(|| panic!("{ctx}"), |inner| inner)
}

fn app_selected_task<S: TaskStore>(app: &App<S>) -> Option<&TaskView> {
    app.visibility().selected_task(&app.tasks)
}

fn app_selected_task_id<S: TaskStore>(app: &App<S>) -> Option<TaskId> {
    app.visibility().selected_task_id(&app.tasks)
}

fn app_select_next<S: TaskStore>(app: &mut App<S>) {
    app.visibility_mut().select_next();
}

fn app_selection_index<S: TaskStore>(app: &App<S>) -> usize {
    app.visibility().selected_index()
}

fn apply_app_filter<S: TaskStore>(app: &mut App<S>, filter: TaskFilter) {
    let keep_id = app.visibility().selected_task_id(&app.tasks);
    {
        let visibility = app.visibility_mut();
        visibility.set_filter(filter);
    }
    app.rebuild_visibility(keep_id);
}

struct MockStore {
    tasks: RefCell<Vec<TaskId>>,
    events: RefCell<HashMap<TaskId, Vec<Event>>>,
    next_oid: RefCell<u8>,
}

impl MockStore {
    fn new() -> Self {
        Self {
            tasks: RefCell::new(Vec::new()),
            events: RefCell::new(HashMap::new()),
            next_oid: RefCell::new(0),
        }
    }

    fn with_task(self, id: TaskId, events: Vec<Event>) -> Self {
        self.tasks.borrow_mut().push(id);
        self.events.borrow_mut().insert(id, events);
        self
    }

    fn from_tasks(entries: Vec<(TaskId, Vec<Event>)>) -> Self {
        let store = Self::new();
        {
            let mut tasks = store.tasks.borrow_mut();
            let mut map = store.events.borrow_mut();
            for (id, events) in entries {
                tasks.push(id);
                map.insert(id, events);
            }
        }
        store
    }
}

#[test]
fn truncate_with_ellipsis_returns_borrowed_when_short() {
    let title = "Short title";
    assert!(matches!(
        truncate_with_ellipsis(title, 20),
        Cow::Borrowed(result) if result == title
    ));
}

#[test]
fn truncate_with_ellipsis_handles_multibyte_titles() {
    let title = "あいうえおかきくけこ";
    assert_eq!(truncate_with_ellipsis(title, 5), "あい...");
}

#[test]
fn truncate_with_ellipsis_keeps_grapheme_clusters_intact() {
    let title = "a\u{0301}bcdef";
    assert_eq!(truncate_with_ellipsis(title, 4), "a\u{0301}...");
}

impl TaskStore for MockStore {
    type Error = anyhow::Error;

    fn list_tasks(&self) -> Result<Vec<TaskId>, Self::Error> {
        Ok(self.tasks.borrow().clone())
    }

    fn load_events(&self, task: TaskId) -> Result<Vec<Event>, Self::Error> {
        Ok(self.events.borrow().get(&task).cloned().unwrap_or_default())
    }

    fn append_event(&self, event: &Event) -> Result<Oid, Self::Error> {
        let mut events = self.events.borrow_mut();
        let entry = events.entry(event.task).or_default();
        entry.push(event.clone());
        let mut tasks = self.tasks.borrow_mut();
        if !tasks.contains(&event.task) {
            tasks.push(event.task);
        }

        let mut counter = self.next_oid.borrow_mut();
        let oid = fake_oid(*counter);
        *counter = counter.wrapping_add(1);
        Ok(oid)
    }
}

fn fake_oid(counter: u8) -> Oid {
    let mut bytes = [0u8; 20];
    bytes[19] = counter;
    match Oid::from_bytes(&bytes) {
        Ok(oid) => oid,
        Err(err) => panic!("failed to construct fake oid: {err}"),
    }
}

fn actor() -> Actor {
    Actor {
        name: "tester".into(),
        email: "tester@example.invalid".into(),
    }
}

fn event(task: TaskId, ts: OffsetDateTime, kind: EventKind) -> Event {
    let mut ev = Event::new(task, &actor(), kind);
    ev.ts = ts;
    ev
}

fn ts(secs: i64) -> OffsetDateTime {
    expect_ok(
        OffsetDateTime::from_unix_timestamp(secs),
        "must create timestamp from unix seconds",
    )
}

fn fixed_task_id(n: u8) -> TaskId {
    expect_ok(
        TaskId::from_str(&format!("00000000-0000-0000-0000-0000000000{n:02}")),
        "must parse task id",
    )
}

fn created(task: TaskId, secs: i64, title: &str) -> Event {
    event(
        task,
        ts(secs),
        EventKind::TaskCreated {
            title: title.into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            state: None,
            state_kind: None,
        },
    )
}

fn child_link(secs: i64, parent: TaskId, child: TaskId) -> Event {
    event(child, ts(secs), EventKind::ChildLinked { parent, child })
}

#[test]
fn status_footer_height_matches_constraints() {
    let constraints = Ui::<MockStore>::status_layout_constraints();
    let total: u16 = constraints.iter().map(min_height_for_constraint).sum();
    assert_eq!(total, Ui::<MockStore>::STATUS_FOOTER_MIN_HEIGHT);
}

const fn min_height_for_constraint(constraint: &Constraint) -> u16 {
    match *constraint {
        Constraint::Length(value) | Constraint::Min(value) => value,
        _ => 0,
    }
}

#[derive(Default)]
struct NoopClipboard;

impl ClipboardSink for NoopClipboard {
    fn set_text(&mut self, _text: &str) -> Result<()> {
        Ok(())
    }
}

struct RecordingClipboard {
    writes: Rc<RefCell<Vec<String>>>,
}

impl RecordingClipboard {
    fn new(writes: Rc<RefCell<Vec<String>>>) -> Self {
        Self { writes }
    }
}

impl ClipboardSink for RecordingClipboard {
    fn set_text(&mut self, text: &str) -> Result<()> {
        self.writes.borrow_mut().push(text.to_string());
        Ok(())
    }
}

struct FailingClipboard {
    message: String,
}

impl FailingClipboard {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl ClipboardSink for FailingClipboard {
    fn set_text(&mut self, _text: &str) -> Result<()> {
        Err(anyhow!(self.message.clone()))
    }
}

fn ui_with_clipboard(app: App<MockStore>, clipboard: Box<dyn ClipboardSink>) -> Ui<MockStore> {
    Ui::with_clipboard(app, actor(), clipboard)
}

#[test]
fn osc52_sequence_encodes_text() {
    let seq = osc52_sequence("Task-ID");
    assert_eq!(seq, "\x1b]52;c;VGFzay1JRA==\x07");
}

#[test]
fn task_view_sorts_comments_chronologically() {
    let task = TaskId::new();
    let events = vec![
        event(
            task,
            ts(2000),
            EventKind::CommentAdded {
                comment_id: EventId::new(),
                body_md: "Second".into(),
            },
        ),
        event(
            task,
            ts(0),
            EventKind::TaskCreated {
                title: "Title".into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: None,
                state_kind: None,
            },
        ),
        event(
            task,
            ts(1000),
            EventKind::CommentAdded {
                comment_id: EventId::new(),
                body_md: "First".into(),
            },
        ),
    ];

    let view = TaskView::from_events(&events);
    assert_eq!(view.comments.len(), 2);
    assert_eq!(view.comments[0].body, "First");
    assert_eq!(view.comments[1].body, "Second");
    assert_eq!(view.last_updated, Some(ts(2000)));
}

#[test]
fn app_refreshes_tasks_sorted_by_last_update() -> Result<()> {
    let task_a = TaskId::new();
    let task_b = TaskId::new();

    let events_a = vec![
        event(
            task_a,
            ts(0),
            EventKind::TaskCreated {
                title: "A".into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: None,
                state_kind: None,
            },
        ),
        event(
            task_a,
            ts(2000),
            EventKind::CommentAdded {
                comment_id: EventId::new(),
                body_md: "Update".into(),
            },
        ),
    ];

    let events_b = vec![event(
        task_b,
        ts(1000),
        EventKind::TaskCreated {
            title: "B".into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            state: None,
            state_kind: None,
        },
    )];

    let store = MockStore::new()
        .with_task(task_a, events_a)
        .with_task(task_b, events_b);

    let app = App::new(store, WorkflowConfig::unrestricted())?;
    assert_eq!(app.tasks.len(), 2);
    let titles: Vec<_> = app
        .tasks
        .iter()
        .map(|view| view.snapshot.title.as_str())
        .collect();
    assert_eq!(titles, vec!["A", "B"]);
    Ok(())
}

#[test]
fn app_get_children_uses_parent_links() -> Result<()> {
    let parent = TaskId::new();
    let child = TaskId::new();

    let parent_events = vec![created(parent, 0, "Parent")];
    let child_events = vec![created(child, 10, "Child"), child_link(20, parent, child)];

    let store = MockStore::new()
        .with_task(parent, parent_events)
        .with_task(child, child_events);
    let app = App::new(store, WorkflowConfig::unrestricted())?;

    let children = app.get_children(parent);
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].snapshot.id, child);
    Ok(())
}

#[test]
fn app_get_children_preserves_task_order() -> Result<()> {
    let parent = TaskId::new();
    let recent_child = TaskId::new();
    let older_child = TaskId::new();

    let store = MockStore::new()
        .with_task(parent, vec![created(parent, 0, "Parent")])
        .with_task(
            older_child,
            vec![
                created(older_child, 10, "Older"),
                child_link(11, parent, older_child),
            ],
        )
        .with_task(
            recent_child,
            vec![
                created(recent_child, 20, "Recent"),
                child_link(21, parent, recent_child),
            ],
        );
    let app = App::new(store, WorkflowConfig::unrestricted())?;

    let children = app.get_children(parent);
    let ids: Vec<TaskId> = children.iter().map(|view| view.snapshot.id).collect();
    assert_eq!(ids, vec![recent_child, older_child]);
    Ok(())
}

#[test]
fn app_get_root_handles_cyclic_parent_graph() -> Result<()> {
    let root = fixed_task_id(1);
    let loop_a = fixed_task_id(2);
    let loop_b = fixed_task_id(3);
    let leaf = fixed_task_id(4);

    let store = MockStore::from_tasks(vec![
        (root, vec![created(root, 0, "Root")]),
        (
            loop_a,
            vec![
                created(loop_a, 1, "LoopA"),
                child_link(2, root, loop_a),
                child_link(3, loop_b, loop_a),
            ],
        ),
        (
            loop_b,
            vec![created(loop_b, 4, "LoopB"), child_link(5, loop_a, loop_b)],
        ),
        (
            leaf,
            vec![
                created(leaf, 6, "Leaf"),
                child_link(7, loop_a, leaf),
                child_link(8, loop_b, leaf),
            ],
        ),
    ]);
    let app = App::new(store, WorkflowConfig::unrestricted())?;

    let root_view = expect_some(app.get_root(leaf), "must locate a root despite cycle");
    assert_eq!(root_view.snapshot.id, root);
    Ok(())
}

#[test]
fn tree_view_includes_parent_and_child() -> Result<()> {
    let parent = TaskId::new();
    let child = TaskId::new();

    let parent_events = vec![event(
        parent,
        ts(0),
        EventKind::TaskCreated {
            title: "Parent".into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            state: None,
            state_kind: None,
        },
    )];
    let child_events = vec![
        event(
            child,
            ts(10),
            EventKind::TaskCreated {
                title: "Child".into(),
                labels: Vec::new(),
                assignees: Vec::new(),
                description: None,
                state: None,
                state_kind: None,
            },
        ),
        event(child, ts(20), EventKind::ChildLinked { parent, child }),
    ];

    let store = MockStore::new()
        .with_task(parent, parent_events)
        .with_task(child, child_events);
    let app = App::new(store, WorkflowConfig::unrestricted())?;
    let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));
    ui.open_tree_view();

    assert_eq!(ui.detail_focus, DetailFocus::TreeView);
    let visible: Vec<TaskId> = ui
        .tree_state
        .visible_nodes
        .iter()
        .map(|(_, task_id)| *task_id)
        .collect();
    assert_eq!(visible, vec![parent, child]);
    Ok(())
}

#[test]
fn tree_view_expands_path_to_selected_grandchild() -> Result<()> {
    let parent = TaskId::new();
    let child = TaskId::new();
    let grandchild = TaskId::new();

    let store = MockStore::new()
        .with_task(parent, vec![created(parent, 0, "Parent")])
        .with_task(
            child,
            vec![created(child, 10, "Child"), child_link(11, parent, child)],
        )
        .with_task(
            grandchild,
            vec![
                created(grandchild, 20, "Grandchild"),
                child_link(21, child, grandchild),
            ],
        );
    let mut app = App::new(store, WorkflowConfig::unrestricted())?;
    app.visibility_mut().jump_to_task(grandchild);
    let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));

    ui.open_tree_view();

    let visible: Vec<TaskId> = ui
        .tree_state
        .visible_nodes
        .iter()
        .map(|(_, task_id)| *task_id)
        .collect();
    assert_eq!(visible, vec![parent, child, grandchild]);
    assert_eq!(ui.tree_state.selected_task_id(), Some(grandchild));
    Ok(())
}

#[test]
fn copy_selected_task_id_writes_to_clipboard() -> Result<()> {
    let task = TaskId::new();
    let store = MockStore::new().with_task(task, vec![created(task, 0, "Task")]);
    let app = App::new(store, WorkflowConfig::unrestricted())?;
    let writes = Rc::new(RefCell::new(Vec::new()));
    let clipboard = RecordingClipboard::new(Rc::clone(&writes));
    let mut ui = ui_with_clipboard(app, Box::new(clipboard));

    ui.copy_selected_task_id();

    let recorded = writes.borrow().last().cloned();
    assert_eq!(recorded, Some(task.to_string()));
    let message = expect_some(ui.message.take(), "info message must be set");
    assert!(matches!(message.level, MessageLevel::Info));
    assert!(message.text.contains("コピー"));
    Ok(())
}

#[test]
fn copy_selected_task_id_reports_clipboard_failure() -> Result<()> {
    let task = TaskId::new();
    let store = MockStore::new().with_task(task, vec![created(task, 0, "Task")]);
    let app = App::new(store, WorkflowConfig::unrestricted())?;
    let mut ui = ui_with_clipboard(app, Box::new(FailingClipboard::new("broken clipboard")));

    ui.copy_selected_task_id();

    let message = expect_some(ui.message.take(), "error message must be set");
    assert!(matches!(message.level, MessageLevel::Error));
    assert!(
        message.text.contains("broken clipboard"),
        "actual text: {}",
        message.text
    );
    Ok(())
}

#[test]
fn copy_selected_task_id_without_selection_shows_error() -> Result<()> {
    let store = MockStore::new();
    let app = App::new(store, WorkflowConfig::unrestricted())?;
    let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));

    ui.copy_selected_task_id();

    let message = expect_some(ui.message.take(), "error message must be set");
    assert!(matches!(message.level, MessageLevel::Error));
    assert!(message.text.contains("コピー対象"));
    Ok(())
}

#[test]
fn add_comment_keeps_selection_and_updates_comments() -> Result<()> {
    let task_a = TaskId::new();
    let task_b = TaskId::new();

    let store = MockStore::new()
        .with_task(
            task_a,
            vec![event(
                task_a,
                ts(0),
                EventKind::TaskCreated {
                    title: "A".into(),
                    labels: Vec::new(),
                    assignees: Vec::new(),
                    description: None,
                    state: None,
                    state_kind: None,
                },
            )],
        )
        .with_task(
            task_b,
            vec![event(
                task_b,
                ts(1),
                EventKind::TaskCreated {
                    title: "B".into(),
                    labels: Vec::new(),
                    assignees: Vec::new(),
                    description: None,
                    state: None,
                    state_kind: None,
                },
            )],
        );

    let mut app = App::new(store, WorkflowConfig::unrestricted())?;
    app_select_next(&mut app);
    let target = expect_some(app_selected_task_id(&app), "selected task id");
    app.add_comment(target, "hello".into(), &actor())?;

    assert_eq!(app_selected_task_id(&app), Some(target));
    assert_eq!(
        expect_some(app_selected_task(&app), "selected task")
            .comments
            .last()
            .map(|c| c.body.as_str()),
        Some("hello")
    );
    // Commented task should move to the top.
    assert_eq!(app_selection_index(&app), 0);
    Ok(())
}

#[test]
fn create_task_registers_and_selects_new_entry() -> Result<()> {
    let store = MockStore::new();
    let mut app = App::new(store, WorkflowConfig::unrestricted())?;

    let data = NewTaskData {
        title: "Title".into(),
        state: Some("todo".into()),
        labels: vec!["type/docs".into()],
        assignees: Vec::new(),
        description: Some("Write documentation".into()),
        parent: None,
    };

    let id = app.create_task(data, &actor())?;
    assert_eq!(app.tasks.len(), 1);
    assert_eq!(app_selected_task_id(&app), Some(id));

    let snap = &expect_some(app_selected_task(&app), "selected task").snapshot;
    assert_eq!(snap.title, "Title");
    assert_eq!(snap.state.as_deref(), Some("todo"));
    assert_eq!(snap.description, "Write documentation");
    let labels: Vec<&str> = snap.labels.iter().map(String::as_str).collect();
    assert_eq!(labels, vec!["type/docs"]);
    Ok(())
}

#[test]
fn create_task_rejects_unknown_state() -> Result<()> {
    let store = MockStore::new();
    let workflow = WorkflowConfig::from_states(vec![WorkflowState::new("state/todo")]);
    let mut app = App::new(store, workflow)?;

    let data = NewTaskData {
        title: "Title".into(),
        state: Some("state/done".into()),
        labels: Vec::new(),
        assignees: Vec::new(),
        description: None,
        parent: None,
    };

    let err = expect_err(app.create_task(data, &actor()), "create_task should fail");
    assert!(err.to_string().contains("タスクの作成に失敗しました"));
    Ok(())
}

#[test]
fn create_task_applies_default_state() -> Result<()> {
    let store = MockStore::new();
    let workflow =
        WorkflowConfig::from_states_with_default(vec![WorkflowState::new("state/todo")], Some("state/todo"));
    let mut app = App::new(store, workflow)?;

    let data = NewTaskData {
        title: "Title".into(),
        state: None,
        labels: Vec::new(),
        assignees: Vec::new(),
        description: None,
        parent: None,
    };

    app.create_task(data, &actor())?;
    let snap = &expect_some(app_selected_task(&app), "selected task").snapshot;
    assert_eq!(snap.state.as_deref(), Some("state/todo"));
    Ok(())
}

#[test]
fn comment_editor_output_strips_comments_and_trims() {
    let input = "# comment\nline1\n\nline2  \n# ignored";
    let parsed = parse_comment_editor_output(input);
    assert_eq!(parsed.as_deref(), Some("line1\n\nline2"));
}

#[test]
fn comment_editor_output_none_when_empty() {
    let input = "# comment\n\n   \n# another comment";
    assert!(parse_comment_editor_output(input).is_none());
}

#[test]
fn new_task_editor_output_parses_fields() {
    let raw = "\
# heading
title: Sample Task
state: state/todo
labels: type/docs, area/cli
assignees: alice, bob
---
This is description.
";
    let parsed = expect_ok(parse_new_task_editor_output(raw), "parse succeeds");
    let data = expect_some(parsed, "should create task");
    assert_eq!(data.title, "Sample Task");
    assert_eq!(data.state.as_deref(), Some("state/todo"));
    assert_eq!(data.labels, vec!["type/docs".to_string(), "area/cli".to_string()]);
    assert_eq!(data.assignees, vec!["alice".to_string(), "bob".to_string()]);
    assert_eq!(data.description.as_deref(), Some("This is description."));
}

#[test]
fn new_task_editor_output_none_when_all_empty() {
    let raw = "\
# heading
title:
state:
labels:
assignees:
---
# no description
";
    let parsed = expect_ok(parse_new_task_editor_output(raw), "parse succeeds");
    assert!(parsed.is_none());
}

#[test]
fn new_task_editor_output_requires_title() {
    let raw = "\
title:
state: state/todo
labels: foo
assignees:
---
";
    let err = expect_err(parse_new_task_editor_output(raw), "should error");
    assert_eq!(err, "タイトルを入力してください");
}

#[test]
fn new_task_editor_template_prefills_default_state() {
    let template = new_task_editor_template(None, None, Some("state/todo"));
    assert!(template.contains("state: state/todo"));
}

#[test]
fn filter_editor_output_parses_all_fields() {
    let parent = fixed_task_id(1);
    let child = fixed_task_id(2);
    let raw = format!(
        "\
states: state/todo,state/done
state_kinds: in_progress,!done
labels: type/bug
assignees: alice
parents: {parent}
children: {child}
text: panic
updated_since: 2025-01-01T00:00:00Z
updated_until: 2025-01-02T00:00:00Z
"
    );
    let filter = expect_ok(parse_filter_editor_output(&raw), "parse succeeds");
    assert!(filter.states.contains("state/todo"));
    assert!(filter.labels.contains("type/bug"));
    assert!(filter.assignees.contains("alice"));
    assert!(filter.parents.contains(&parent));
    assert!(filter.children.contains(&child));
    assert_eq!(filter.text.as_deref(), Some("panic"));
    assert!(
        filter
            .state_kinds
            .include
            .contains(&git_mile_core::StateKind::InProgress)
    );
    assert!(filter.state_kinds.exclude.contains(&StateKind::Done));
    let updated = expect_some(filter.updated, "updated filter");
    let expected_since = expect_ok(OffsetDateTime::parse("2025-01-01T00:00:00Z", &Rfc3339), "ts");
    let expected_until = expect_ok(OffsetDateTime::parse("2025-01-02T00:00:00Z", &Rfc3339), "ts");
    assert_eq!(updated.since, Some(expected_since));
    assert_eq!(updated.until, Some(expected_until));
}

#[test]
fn filter_editor_output_rejects_invalid_timestamp() {
    let err = expect_err(
        parse_filter_editor_output("updated_since: invalid"),
        "should error",
    );
    assert!(err.contains("時刻"));
}

#[test]
fn summarize_task_filter_lists_active_fields() {
    let mut filter = TaskFilter::default();
    filter.states.insert("state/todo".into());
    filter.text = Some("panic".into());
    let summary = summarize_task_filter(&filter);
    assert!(summary.contains("state=state/todo"));
    assert!(summary.contains("text=\"panic\""));
}

#[test]
fn summarize_task_filter_includes_state_kind_clause() {
    let mut filter = TaskFilter::default();
    filter.state_kinds.exclude.insert(StateKind::Done);
    let summary = summarize_task_filter(&filter);
    assert!(summary.contains("state-kind!=done"));
}

#[test]
fn status_layout_allocates_space_for_filter_and_status_blocks() {
    let area = Rect::new(0, 0, 80, 12);
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(Ui::<MockStore>::status_layout_constraints())
        .split(area);
    assert_eq!(rows.len(), 3);
    assert!(rows[1].height >= 3, "フィルタ欄の高さが不足しています");
    assert!(rows[2].height >= 3, "ステータス欄の高さが不足しています");
}

#[test]
fn instructions_include_filter_shortcut() -> Result<()> {
    let task = TaskId::new();
    let store = MockStore::new().with_task(task, vec![created(task, 0, "Task")]);
    let app = App::new(store, WorkflowConfig::unrestricted())?;
    let ui = ui_with_clipboard(app, Box::new(NoopClipboard));
    assert!(
        ui.instructions().contains("f:フィルタ"),
        "instructions must mention filter shortcut"
    );
    Ok(())
}

#[test]
fn parse_list_trims_entries() {
    assert_eq!(
        parse_list("one, two , , three"),
        vec!["one".to_owned(), "two".to_owned(), "three".to_owned()]
    );
}

#[test]
fn update_task_applies_field_changes() -> Result<()> {
    let task = TaskId::new();
    let created = Event::new(
        task,
        &actor(),
        EventKind::TaskCreated {
            title: "Initial".into(),
            labels: vec!["type/bug".into(), "area/cli".into()],
            assignees: vec!["alice".into(), "carol".into()],
            description: Some("old description".into()),
            state: Some("state/in-progress".into()),
            state_kind: Some(StateKind::InProgress),
        },
    );

    let store = MockStore::new().with_task(task, vec![created]);
    let mut app = App::new(store, WorkflowConfig::unrestricted())?;

    let updated = app.update_task(
        task,
        NewTaskData {
            title: "Updated".into(),
            state: None,
            labels: vec!["type/docs".into()],
            assignees: vec!["bob".into()],
            description: Some("new description".into()),
            parent: None,
        },
        &actor(),
    )?;
    assert!(updated);

    let view = expect_some(
        app.tasks.iter().find(|view| view.snapshot.id == task),
        "task should exist",
    );
    assert_eq!(view.snapshot.title, "Updated");
    assert_eq!(view.snapshot.state, None);
    let labels: Vec<&str> = view.snapshot.labels.iter().map(String::as_str).collect();
    assert_eq!(labels, vec!["type/docs"]);
    let assignees: Vec<&str> = view.snapshot.assignees.iter().map(String::as_str).collect();
    assert_eq!(assignees, vec!["bob"]);
    assert_eq!(view.snapshot.description, "new description");

    let events = app.store().events.borrow();
    let stored = expect_some(events.get(&task), "events for task");
    assert_eq!(stored.len(), 8);
    assert!(
        stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::TaskTitleSet { .. }))
    );
    assert!(
        stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::TaskStateCleared))
    );
    assert!(
        stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::TaskDescriptionSet { .. }))
    );
    assert!(
        stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::LabelsAdded { .. }))
    );
    assert!(
        stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::LabelsRemoved { .. }))
    );
    assert!(
        stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::AssigneesAdded { .. }))
    );
    assert!(
        stored
            .iter()
            .any(|ev| matches!(ev.kind, EventKind::AssigneesRemoved { .. }))
    );
    Ok(())
}

#[test]
fn update_task_returns_false_when_no_diff() -> Result<()> {
    let task = TaskId::new();
    let created = Event::new(
        task,
        &actor(),
        EventKind::TaskCreated {
            title: "Initial".into(),
            labels: vec!["type/bug".into()],
            assignees: vec!["alice".into()],
            description: Some("desc".into()),
            state: Some("state/todo".into()),
            state_kind: Some(StateKind::Todo),
        },
    );

    let store = MockStore::new().with_task(task, vec![created]);
    let mut app = App::new(store, WorkflowConfig::unrestricted())?;
    let snapshot = {
        let events = app.store().events.borrow();
        let stored = expect_some(events.get(&task), "events for task");
        TaskSnapshot::replay(stored)
    };

    let updated = app.update_task(
        task,
        NewTaskData {
            title: snapshot.title.clone(),
            state: snapshot.state.clone(),
            labels: snapshot.labels.iter().cloned().collect(),
            assignees: snapshot.assignees.iter().cloned().collect(),
            description: if snapshot.description.is_empty() {
                None
            } else {
                Some(snapshot.description)
            },
            parent: None,
        },
        &actor(),
    )?;
    assert!(!updated);

    let events = app.store().events.borrow();
    let stored = expect_some(events.get(&task), "events for task");
    assert_eq!(stored.len(), 1);
    Ok(())
}

#[test]
fn set_task_state_applies_new_value() -> Result<()> {
    let task = TaskId::new();
    let created = Event::new(
        task,
        &actor(),
        EventKind::TaskCreated {
            title: "Initial".into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            state: Some("state/todo".into()),
            state_kind: Some(StateKind::Todo),
        },
    );
    let workflow = WorkflowConfig::from_states(vec![
        WorkflowState::new("state/todo"),
        WorkflowState::new("state/done"),
    ]);
    let store = MockStore::new().with_task(task, vec![created]);
    let mut app = App::new(store, workflow)?;

    let changed = app.set_task_state(task, Some("state/done".into()), &actor())?;
    assert!(changed);

    let view = expect_some(
        app.tasks.iter().find(|view| view.snapshot.id == task),
        "task should exist",
    );
    assert_eq!(view.snapshot.state.as_deref(), Some("state/done"));

    let events = app.store().events.borrow();
    let stored = expect_some(events.get(&task), "events for task");
    assert_eq!(stored.len(), 2);
    assert!(
        stored
            .iter()
            .any(|ev| matches!(&ev.kind, EventKind::TaskStateSet { state, .. } if state == "state/done"))
    );
    Ok(())
}

#[test]
fn set_task_state_returns_false_when_unchanged() -> Result<()> {
    let task = TaskId::new();
    let created = Event::new(
        task,
        &actor(),
        EventKind::TaskCreated {
            title: "Initial".into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            state: Some("state/todo".into()),
            state_kind: Some(StateKind::Todo),
        },
    );
    let workflow = WorkflowConfig::from_states(vec![WorkflowState::new("state/todo")]);
    let store = MockStore::new().with_task(task, vec![created]);
    let mut app = App::new(store, workflow)?;

    let changed = app.set_task_state(task, Some("state/todo".into()), &actor())?;
    assert!(!changed);

    let events = app.store().events.borrow();
    let stored = expect_some(events.get(&task), "events for task");
    assert_eq!(stored.len(), 1);
    Ok(())
}

#[test]
fn open_state_picker_prefills_current_state() -> Result<()> {
    let task = TaskId::new();
    let created = Event::new(
        task,
        &actor(),
        EventKind::TaskCreated {
            title: "Initial".into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            state: Some("state/done".into()),
            state_kind: Some(StateKind::Done),
        },
    );
    let workflow = WorkflowConfig::from_states(vec![
        WorkflowState::new("state/todo"),
        WorkflowState::new("state/done"),
    ]);
    let store = MockStore::new().with_task(task, vec![created]);
    let app = App::new(store, workflow)?;
    let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));
    apply_app_filter(&mut ui.app, TaskFilter::default());
    ui.open_state_picker();

    assert_eq!(ui.detail_focus, DetailFocus::StatePicker);
    let picker = expect_some(ui.state_picker.as_ref(), "state picker");
    assert_eq!(
        picker.options[picker.selected].value.as_deref(),
        Some("state/done")
    );
    Ok(())
}

#[test]
fn apply_state_picker_selection_updates_state() -> Result<()> {
    let task = TaskId::new();
    let created = Event::new(
        task,
        &actor(),
        EventKind::TaskCreated {
            title: "Initial".into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            state: Some("state/todo".into()),
            state_kind: Some(StateKind::Todo),
        },
    );
    let workflow = WorkflowConfig::from_states(vec![
        WorkflowState::new("state/todo"),
        WorkflowState::new("state/done"),
    ]);
    let store = MockStore::new().with_task(task, vec![created]);
    let app = App::new(store, workflow)?;
    let mut ui = ui_with_clipboard(app, Box::new(NoopClipboard));

    ui.open_state_picker();
    ui.state_picker_down();
    ui.apply_state_picker_selection();

    let view = expect_some(
        ui.app.tasks.iter().find(|view| view.snapshot.id == task),
        "task exists",
    );
    assert_eq!(view.snapshot.state.as_deref(), Some("state/done"));
    assert!(ui.state_picker.is_none());
    assert_eq!(ui.detail_focus, DetailFocus::None);
    Ok(())
}
