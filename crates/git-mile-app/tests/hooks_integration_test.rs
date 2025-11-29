//! Integration tests for Phase 2 hook support
//!
//! These tests verify that all hook types (`PreEvent`, `PostEvent`, `PreTaskUpdate`, etc.)
//! are executed correctly across `TaskWriter` operations.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::redundant_clone,
    clippy::field_reassign_with_default
)]

use git_mile_app::config::ProjectConfig;
use git_mile_app::task_writer::{CommentRequest, CreateTaskRequest, TaskUpdate, TaskWriter};
use git_mile_core::event::Actor;
use git_mile_hooks::HooksConfig;
use git_mile_store_git::GitStore;
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

/// Test helper: Setup a temporary git repository with hooks directory
fn setup_test_repo() -> (TempDir, GitStore, PathBuf) {
    let temp_dir = TempDir::with_prefix("git-mile-hooks-test-").expect("create temp dir");
    let repo_path = temp_dir.path();

    // Initialize git repository
    git2::Repository::init(repo_path).expect("init git repo");

    // Create .git-mile directory structure
    let git_mile_dir = repo_path.join(".git-mile");
    fs::create_dir(&git_mile_dir).expect("create .git-mile dir");

    // Create hooks directory
    let hooks_dir = git_mile_dir.join("hooks");
    fs::create_dir(&hooks_dir).expect("create hooks dir");

    let store = GitStore::open(repo_path).expect("open git store");

    (temp_dir, store, hooks_dir)
}

/// Test helper: Create a hook script that writes execution info to a log file
fn create_logging_hook(hooks_dir: &std::path::Path, hook_name: &str, log_path: &std::path::Path) {
    let hook_path = hooks_dir.join(hook_name);
    let script = format!(
        r#"#!/bin/sh
echo "{}" >> "{}"
exit 0
"#,
        hook_name,
        log_path.display()
    );

    fs::write(&hook_path, script).expect("write hook script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path).expect("get metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms).expect("set executable");
    }
}

/// Test helper: Create a hook that always fails
fn create_failing_hook(hooks_dir: &std::path::Path, hook_name: &str) {
    let hook_path = hooks_dir.join(hook_name);
    let script = r#"#!/bin/sh
echo "Hook rejected" >&2
exit 1
"#;

    fs::write(&hook_path, script).expect("write hook script");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&hook_path).expect("get metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&hook_path, perms).expect("set executable");
    }
}

fn test_actor() -> Actor {
    Actor {
        name: "Test User".to_owned(),
        email: "test@example.com".to_owned(),
    }
}

#[test]
#[cfg(unix)] // Requires executable scripts
fn test_task_create_executes_pre_and_post_hooks() {
    let (_temp, store, hooks_dir) = setup_test_repo();
    let log_file = hooks_dir.join("execution.log");

    // Create hooks that log their execution
    create_logging_hook(&hooks_dir, "pre-event", &log_file);
    create_logging_hook(&hooks_dir, "pre-task-create", &log_file);
    create_logging_hook(&hooks_dir, "post-task-create", &log_file);
    create_logging_hook(&hooks_dir, "post-event", &log_file);

    let hooks_config = HooksConfig::default();
    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow,
        hooks_config,
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    // Create a task
    let request = CreateTaskRequest {
        title: "Test Task".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    let result = writer.create_task(request);
    assert!(result.is_ok(), "Task creation should succeed");

    // Verify hook execution order
    let log_content = fs::read_to_string(&log_file).expect("read log file");
    let lines: Vec<&str> = log_content.lines().collect();

    assert_eq!(lines.len(), 4, "Should execute 4 hooks");
    assert_eq!(lines[0], "pre-event");
    assert_eq!(lines[1], "pre-task-create");
    assert_eq!(lines[2], "post-task-create");
    assert_eq!(lines[3], "post-event");
}

#[test]
#[cfg(unix)]
fn test_pre_task_create_hook_can_reject_operation() {
    let (_temp, store, hooks_dir) = setup_test_repo();

    // Create a failing pre-hook
    create_failing_hook(&hooks_dir, "pre-task-create");

    let hooks_config = HooksConfig::default();
    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow,
        hooks_config,
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    let request = CreateTaskRequest {
        title: "Test Task".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    let result = writer.create_task(request);
    assert!(result.is_err(), "Task creation should fail when pre-hook rejects");
}

#[test]
#[cfg(unix)]
fn test_task_update_hooks_execute_for_title_change() {
    let (_temp, store, hooks_dir) = setup_test_repo();
    let log_file = hooks_dir.join("execution.log");

    // First create a task
    let hooks_config = HooksConfig::default();
    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow.clone(),
        hooks_config.clone(),
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    let request = CreateTaskRequest {
        title: "Original Title".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    let create_result = writer.create_task(request).expect("create task");
    let task_id = create_result.task;

    // Create hooks for update
    create_logging_hook(&hooks_dir, "pre-event", &log_file);
    create_logging_hook(&hooks_dir, "pre-task-update", &log_file);
    create_logging_hook(&hooks_dir, "post-task-update", &log_file);
    create_logging_hook(&hooks_dir, "post-event", &log_file);

    // Update task title
    let patch = TaskUpdate {
        title: Some("Updated Title".to_owned()),
        ..TaskUpdate::default()
    };

    let result = writer.update_task(task_id, patch, &test_actor());
    assert!(result.is_ok(), "Task update should succeed");

    // Verify hooks executed
    let log_content = fs::read_to_string(&log_file).expect("read log file");
    let lines: Vec<&str> = log_content.lines().collect();

    assert!(lines.contains(&"pre-event"), "PreEvent should execute");
    assert!(lines.contains(&"pre-task-update"), "PreTaskUpdate should execute");
    assert!(
        lines.contains(&"post-task-update"),
        "PostTaskUpdate should execute"
    );
    assert!(lines.contains(&"post-event"), "PostEvent should execute");
}

#[test]
#[cfg(unix)]
fn test_state_change_hooks_execute() {
    let (_temp, store, hooks_dir) = setup_test_repo();
    let log_file = hooks_dir.join("execution.log");

    // Create a task with a state
    let hooks_config = HooksConfig::default();
    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow.clone(),
        hooks_config.clone(),
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    let request = CreateTaskRequest {
        title: "Test Task".to_owned(),
        state: Some("state/todo".to_owned()),
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    let create_result = writer.create_task(request).expect("create task");
    let task_id = create_result.task;

    // Create state change hooks
    create_logging_hook(&hooks_dir, "pre-event", &log_file);
    create_logging_hook(&hooks_dir, "pre-state-change", &log_file);
    create_logging_hook(&hooks_dir, "post-state-change", &log_file);
    create_logging_hook(&hooks_dir, "post-event", &log_file);

    // Change state
    let result = writer.set_state(task_id, Some("state/done".to_owned()), &test_actor());
    assert!(result.is_ok(), "State change should succeed");

    // Verify state-specific hooks executed
    let log_content = fs::read_to_string(&log_file).expect("read log file");
    let lines: Vec<&str> = log_content.lines().collect();

    assert!(lines.contains(&"pre-event"));
    assert!(lines.contains(&"pre-state-change"));
    assert!(lines.contains(&"post-state-change"));
    assert!(lines.contains(&"post-event"));
}

#[test]
#[cfg(unix)]
fn test_comment_hooks_execute() {
    let (_temp, store, hooks_dir) = setup_test_repo();
    let log_file = hooks_dir.join("execution.log");

    // Create a task
    let hooks_config = HooksConfig::default();
    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow.clone(),
        hooks_config.clone(),
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    let request = CreateTaskRequest {
        title: "Test Task".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    let create_result = writer.create_task(request).expect("create task");
    let task_id = create_result.task;

    // Create comment hooks
    create_logging_hook(&hooks_dir, "pre-event", &log_file);
    create_logging_hook(&hooks_dir, "pre-comment-add", &log_file);
    create_logging_hook(&hooks_dir, "post-comment-add", &log_file);
    create_logging_hook(&hooks_dir, "post-event", &log_file);

    // Add comment
    let comment = CommentRequest {
        body_md: "Test comment".to_owned(),
        actor: test_actor(),
    };

    let result = writer.add_comment(task_id, comment);
    assert!(result.is_ok(), "Comment addition should succeed");

    // Verify hooks executed
    let log_content = fs::read_to_string(&log_file).expect("read log file");
    let lines: Vec<&str> = log_content.lines().collect();

    assert!(lines.contains(&"pre-event"));
    assert!(lines.contains(&"pre-comment-add"));
    assert!(lines.contains(&"post-comment-add"));
    assert!(lines.contains(&"post-event"));
}

#[test]
#[cfg(unix)]
fn test_relation_hooks_execute_on_parent_link() {
    let (_temp, store, hooks_dir) = setup_test_repo();
    let log_file = hooks_dir.join("execution.log");

    // Create parent and child tasks
    let hooks_config = HooksConfig::default();
    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow.clone(),
        hooks_config.clone(),
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    let parent_request = CreateTaskRequest {
        title: "Parent Task".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    let parent_result = writer.create_task(parent_request).expect("create parent");
    let parent_id = parent_result.task;

    let child_request = CreateTaskRequest {
        title: "Child Task".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    let child_result = writer.create_task(child_request).expect("create child");
    let child_id = child_result.task;

    // Create relation hooks
    create_logging_hook(&hooks_dir, "pre-event", &log_file);
    create_logging_hook(&hooks_dir, "pre-relation-change", &log_file);
    create_logging_hook(&hooks_dir, "post-relation-change", &log_file);
    create_logging_hook(&hooks_dir, "post-event", &log_file);

    // Link parent
    let result = writer.link_parents(child_id, &[parent_id], &test_actor());
    assert!(result.is_ok(), "Parent linking should succeed");

    // Verify hooks executed (should be 2 sets: one for child event, one for parent event)
    let log_content = fs::read_to_string(&log_file).expect("read log file");
    let lines: Vec<&str> = log_content.lines().collect();

    // Count hook executions
    let pre_event_count = lines.iter().filter(|&&l| l == "pre-event").count();
    let pre_relation_count = lines.iter().filter(|&&l| l == "pre-relation-change").count();
    let post_relation_count = lines.iter().filter(|&&l| l == "post-relation-change").count();
    let post_event_count = lines.iter().filter(|&&l| l == "post-event").count();

    assert_eq!(
        pre_event_count, 2,
        "PreEvent should execute twice (child + parent)"
    );
    assert_eq!(pre_relation_count, 2, "PreRelationChange should execute twice");
    assert_eq!(post_relation_count, 2, "PostRelationChange should execute twice");
    assert_eq!(post_event_count, 2, "PostEvent should execute twice");
}

#[test]
#[cfg(unix)]
fn test_pre_event_can_reject_any_operation() {
    let (_temp, store, hooks_dir) = setup_test_repo();

    // Create a failing pre-event hook
    create_failing_hook(&hooks_dir, "pre-event");

    let hooks_config = HooksConfig::default();
    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow,
        hooks_config,
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    let request = CreateTaskRequest {
        title: "Test Task".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    let result = writer.create_task(request);
    assert!(result.is_err(), "PreEvent should be able to reject any operation");
}

#[test]
#[cfg(unix)]
fn test_hook_execution_order_is_correct() {
    let (_temp, store, hooks_dir) = setup_test_repo();
    let log_file = hooks_dir.join("execution.log");

    // Create all hooks
    create_logging_hook(&hooks_dir, "pre-event", &log_file);
    create_logging_hook(&hooks_dir, "pre-task-create", &log_file);
    create_logging_hook(&hooks_dir, "post-task-create", &log_file);
    create_logging_hook(&hooks_dir, "post-event", &log_file);

    let hooks_config = HooksConfig::default();
    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow,
        hooks_config,
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    let request = CreateTaskRequest {
        title: "Test Task".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    writer.create_task(request).expect("create task");

    // Verify exact execution order
    let log_content = fs::read_to_string(&log_file).expect("read log file");
    let lines: Vec<&str> = log_content.lines().collect();

    assert_eq!(
        lines,
        vec!["pre-event", "pre-task-create", "post-task-create", "post-event"],
        "Hook execution order should be: PreEvent -> Specific Pre -> Specific Post -> PostEvent"
    );
}

#[test]
fn test_hooks_skip_when_script_not_found() {
    let (_temp, store, hooks_dir) = setup_test_repo();

    // Don't create any hook scripts
    let hooks_config = HooksConfig::default();
    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow,
        hooks_config,
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    let request = CreateTaskRequest {
        title: "Test Task".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    // Should succeed even without hook scripts
    let result = writer.create_task(request);
    assert!(
        result.is_ok(),
        "Operations should succeed when hooks are not found"
    );
}

#[test]
fn test_hooks_skip_when_disabled() {
    let (_temp, store, hooks_dir) = setup_test_repo();

    // Hooks are disabled
    let mut hooks_config = HooksConfig::default();
    hooks_config.enabled = false;

    let workflow = ProjectConfig::default().workflow;
    let writer = TaskWriter::new(
        store,
        workflow,
        hooks_config,
        hooks_dir.parent().unwrap().to_path_buf(),
    );

    let request = CreateTaskRequest {
        title: "Test Task".to_owned(),
        state: None,
        labels: vec![],
        assignees: vec![],
        description: None,
        parents: vec![],
        actor: test_actor(),
    };

    let result = writer.create_task(request);
    assert!(
        result.is_ok(),
        "Operations should succeed when hooks are disabled"
    );
}
