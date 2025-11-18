//! Integration tests for HookExecutor.

use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::StateKind;
use git_mile_hooks::{HookContext, HookError, HookExecutor, HookKind, HooksConfig};
use std::path::PathBuf;

fn test_hooks_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn create_test_event() -> Event {
    let task_id = "019a9440-2270-72f1-8306-0bf4ea84d34e".parse().unwrap();
    let actor = Actor {
        name: "test".to_string(),
        email: "test@example.com".to_string(),
    };
    let kind = EventKind::TaskCreated {
        title: "Test Task".to_string(),
        labels: vec![],
        assignees: vec![],
        description: None,
        state: Some("state/todo".to_string()),
        state_kind: Some(StateKind::Todo),
    };
    Event::new(task_id, &actor, kind)
}

#[test]
#[ignore] // Requires filesystem setup and executable scripts
fn test_execute_success() {
    let config = HooksConfig {
        enabled: true,
        disabled: vec![],
        timeout: 5,
        async_post_hooks: false,
        hooks_dir: test_hooks_dir(),
    };

    let executor = HookExecutor::new(config, test_hooks_dir());
    let context = HookContext::new(&create_test_event());

    // Create a hook script by copying success.sh
    std::fs::create_dir_all(test_hooks_dir()).unwrap();
    let hook_path = test_hooks_dir().join("pre-task-create");
    std::fs::copy(test_hooks_dir().join("success.sh"), &hook_path).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook_path, perms).ok();
    }

    let result = executor.execute(HookKind::PreTaskCreate, &context);

    // Clean up
    std::fs::remove_file(&hook_path).ok();

    assert!(result.is_ok());
    let hook_result = result.unwrap();
    assert_eq!(hook_result.exit_code, 0);
    assert!(hook_result.stdout.contains("Hook executed successfully"));
}

#[test]
#[ignore] // Requires filesystem setup and executable scripts
fn test_execute_failure() {
    let config = HooksConfig {
        enabled: true,
        disabled: vec![],
        timeout: 5,
        async_post_hooks: false,
        hooks_dir: test_hooks_dir(),
    };

    let executor = HookExecutor::new(config, test_hooks_dir());
    let context = HookContext::new(&create_test_event());

    // Create a hook script by copying failure.sh
    let hook_path = test_hooks_dir().join("pre-task-update");
    std::fs::copy(test_hooks_dir().join("failure.sh"), &hook_path).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook_path, perms).ok();
    }

    let result = executor.execute(HookKind::PreTaskUpdate, &context);

    // Clean up
    std::fs::remove_file(&hook_path).ok();

    match result {
        Err(HookError::Rejected { code, stderr }) => {
            assert_eq!(code, 1);
            assert!(stderr.contains("Hook validation failed"));
        }
        _ => panic!("Expected HookError::Rejected"),
    }
}

#[test]
#[ignore] // Requires filesystem setup and executable scripts
fn test_execute_timeout() {
    let config = HooksConfig {
        enabled: true,
        disabled: vec![],
        timeout: 1, // 1 second timeout
        async_post_hooks: false,
        hooks_dir: test_hooks_dir(),
    };

    let executor = HookExecutor::new(config, test_hooks_dir());
    let context = HookContext::new(&create_test_event());

    // Create a hook script by copying timeout.sh
    let hook_path = test_hooks_dir().join("post-task-create");
    std::fs::copy(test_hooks_dir().join("timeout.sh"), &hook_path).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&hook_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&hook_path, perms).ok();
    }

    let result = executor.execute(HookKind::PostTaskCreate, &context);

    // Clean up
    std::fs::remove_file(&hook_path).ok();

    match result {
        Err(HookError::Timeout(_)) => (),
        _ => panic!("Expected HookError::Timeout, got {:?}", result),
    }
}

#[test]
fn test_execute_not_found() {
    let config = HooksConfig {
        enabled: true,
        disabled: vec![],
        timeout: 5,
        async_post_hooks: false,
        hooks_dir: test_hooks_dir(),
    };

    let executor = HookExecutor::new(config, test_hooks_dir());
    let context = HookContext::new(&create_test_event());

    // Don't create any hook script
    let result = executor.execute(HookKind::PreStateChange, &context);

    match result {
        Err(HookError::NotFound(_)) => (),
        _ => panic!("Expected HookError::NotFound, got {:?}", result),
    }
}

#[test]
fn test_execute_disabled_hook() {
    let config = HooksConfig {
        enabled: true,
        disabled: vec!["pre-comment-add".to_string()],
        timeout: 5,
        async_post_hooks: false,
        hooks_dir: test_hooks_dir(),
    };

    let executor = HookExecutor::new(config, test_hooks_dir());
    let context = HookContext::new(&create_test_event());

    // Even if the hook script exists, it should not be executed
    let hook_path = test_hooks_dir().join("pre-comment-add");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("success.sh", &hook_path).ok();
    }

    let result = executor.execute(HookKind::PreCommentAdd, &context);

    // Clean up
    std::fs::remove_file(&hook_path).ok();

    // Disabled hooks return success without executing
    assert!(result.is_ok());
    let hook_result = result.unwrap();
    assert_eq!(hook_result.exit_code, 0);
    assert!(hook_result.stdout.is_empty());
}

#[test]
fn test_execute_hooks_disabled() {
    let config = HooksConfig {
        enabled: false, // Hooks globally disabled
        disabled: vec![],
        timeout: 5,
        async_post_hooks: false,
        hooks_dir: test_hooks_dir(),
    };

    let executor = HookExecutor::new(config, test_hooks_dir());
    let context = HookContext::new(&create_test_event());

    let hook_path = test_hooks_dir().join("post-comment-add");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("success.sh", &hook_path).ok();
    }

    let result = executor.execute(HookKind::PostCommentAdd, &context);

    // Clean up
    std::fs::remove_file(&hook_path).ok();

    // When hooks are globally disabled, they return success without executing
    assert!(result.is_ok());
    let hook_result = result.unwrap();
    assert_eq!(hook_result.exit_code, 0);
    assert!(hook_result.stdout.is_empty());
}

#[test]
fn test_json_context_serialization() {
    let event = create_test_event();
    let context = HookContext::new(&event);

    let json = serde_json::to_string(&context).unwrap();
    // EventKind is serialized with "type" tag
    assert!(json.contains("taskCreated") || json.contains("TaskCreated"));
    assert!(json.contains("Test Task"));
}
