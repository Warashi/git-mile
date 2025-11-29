#![allow(missing_docs)]

use anyhow::Result;
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::TaskId;
use git_mile_store_git::GitStore;
use git2::Repository;
use std::fs;
use std::path::{Path, PathBuf};

fn temp_repo_path() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("git-mile-push-pull-test-{}", TaskId::new()));
    if path.exists() {
        fs::remove_dir_all(&path)?;
    }
    fs::create_dir(&path)?;
    Ok(path)
}

fn setup_remote_repo() -> Result<(PathBuf, Repository)> {
    let remote_path = temp_repo_path()?;
    let repo = Repository::init_bare(&remote_path)?;
    Ok((remote_path, repo))
}

fn setup_local_repo_with_remote(remote_path: &Path) -> Result<(PathBuf, GitStore)> {
    let local_path = temp_repo_path()?;
    let repo = Repository::init(&local_path)?;

    // Add remote
    repo.remote("origin", &format!("file://{}", remote_path.display()))?;

    let store = GitStore::open(&local_path)?;
    Ok((local_path, store))
}

#[test]
fn test_push_to_empty_remote() -> Result<()> {
    let (remote_path, _remote_repo) = setup_remote_repo()?;
    let (local_path, local_store) = setup_local_repo_with_remote(&remote_path)?;

    // Create a task
    let task = TaskId::new();
    let actor = Actor {
        name: "tester".into(),
        email: "tester@example.invalid".into(),
    };

    let event = Event::new(
        task,
        &actor,
        EventKind::TaskCreated {
            title: "Test task".into(),
            labels: vec![],
            assignees: vec![],
            description: None,
            state: None,
            state_kind: None,
        },
    );

    local_store.append_event(&event)?;

    // Push to remote
    local_store.push_refs("origin", false)?;

    // Verify the ref exists in remote
    let remote_repo = Repository::open(&remote_path)?;
    let refname = format!("refs/git-mile/tasks/{task}");
    assert!(remote_repo.find_reference(&refname).is_ok());

    // Cleanup
    fs::remove_dir_all(&local_path)?;
    fs::remove_dir_all(&remote_path)?;
    Ok(())
}

#[test]
fn test_pull_from_remote_with_new_tasks() -> Result<()> {
    let (remote_path, _remote_repo) = setup_remote_repo()?;
    let (local1_path, local1_store) = setup_local_repo_with_remote(&remote_path)?;

    // Create a task in local1
    let task = TaskId::new();
    let actor = Actor {
        name: "tester".into(),
        email: "tester@example.invalid".into(),
    };

    let event = Event::new(
        task,
        &actor,
        EventKind::TaskCreated {
            title: "Test task".into(),
            labels: vec![],
            assignees: vec![],
            description: None,
            state: None,
            state_kind: None,
        },
    );

    local1_store.append_event(&event)?;
    local1_store.push_refs("origin", false)?;

    // Create local2 and pull
    let (local2_path, local2_store) = setup_local_repo_with_remote(&remote_path)?;
    local2_store.pull_refs("origin")?;

    // Verify task exists in local2
    let tasks = local2_store.list_tasks()?;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0], task);

    let events = local2_store.load_events(task)?;
    assert_eq!(events.len(), 1);
    assert!(matches!(events[0].kind, EventKind::TaskCreated { .. }));

    // Cleanup
    fs::remove_dir_all(&local1_path)?;
    fs::remove_dir_all(&local2_path)?;
    fs::remove_dir_all(&remote_path)?;
    Ok(())
}

#[test]
fn test_push_pull_concurrent_modifications() -> Result<()> {
    let (remote_path, _remote_repo) = setup_remote_repo()?;
    let (local1_path, local1_store) = setup_local_repo_with_remote(&remote_path)?;
    let (local2_path, local2_store) = setup_local_repo_with_remote(&remote_path)?;

    let task = TaskId::new();
    let actor = Actor {
        name: "tester".into(),
        email: "tester@example.invalid".into(),
    };

    // Create task in local1
    let event1 = Event::new(
        task,
        &actor,
        EventKind::TaskCreated {
            title: "Test task".into(),
            labels: vec![],
            assignees: vec![],
            description: None,
            state: None,
            state_kind: None,
        },
    );
    local1_store.append_event(&event1)?;
    local1_store.push_refs("origin", false)?;

    // Pull to local2
    local2_store.pull_refs("origin")?;

    // Make concurrent modifications
    let event2 = Event::new(
        task,
        &actor,
        EventKind::LabelsAdded {
            labels: vec!["label1".into()],
        },
    );
    local1_store.append_event(&event2)?;

    let event3 = Event::new(
        task,
        &actor,
        EventKind::LabelsAdded {
            labels: vec!["label2".into()],
        },
    );
    local2_store.append_event(&event3)?;

    // Push from local1
    local1_store.push_refs("origin", false)?;

    // Push from local2 (should fail without force)
    assert!(local2_store.push_refs("origin", false).is_err());

    // Pull in local2 to merge
    local2_store.pull_refs("origin")?;

    // Now push should succeed
    local2_store.push_refs("origin", false)?;

    // Pull back to local1
    local1_store.pull_refs("origin")?;

    // Both should have all events (CRDT convergence)
    let local1_events = local1_store.load_events(task)?;
    let local2_events = local2_store.load_events(task)?;

    // Both should have 3+ events (created + 2 labels + merge commits)
    assert!(local1_events.len() >= 3);
    assert!(local2_events.len() >= 3);

    // Check that both have the same labels (CRDT convergence)
    let snapshot1 = git_mile_core::TaskSnapshot::replay(&local1_events);
    let snapshot2 = git_mile_core::TaskSnapshot::replay(&local2_events);

    assert_eq!(snapshot1.labels.len(), 2);
    assert_eq!(snapshot2.labels.len(), 2);
    assert!(snapshot1.labels.contains("label1"));
    assert!(snapshot1.labels.contains("label2"));
    assert!(snapshot2.labels.contains("label1"));
    assert!(snapshot2.labels.contains("label2"));

    // Cleanup
    fs::remove_dir_all(&local1_path)?;
    fs::remove_dir_all(&local2_path)?;
    fs::remove_dir_all(&remote_path)?;
    Ok(())
}

#[test]
fn test_push_fails_with_missing_remote() -> Result<()> {
    let local_path = temp_repo_path()?;
    Repository::init(&local_path)?;
    let store = GitStore::open(&local_path)?;

    // Try to push without a remote
    let result = store.push_refs("nonexistent", false);
    assert!(result.is_err());
    if let Err(e) = result {
        assert!(e.to_string().contains("not found"));
    }

    // Cleanup
    fs::remove_dir_all(&local_path)?;
    Ok(())
}

#[test]
fn test_pull_fails_with_missing_remote() -> Result<()> {
    let local_path = temp_repo_path()?;
    Repository::init(&local_path)?;
    let store = GitStore::open(&local_path)?;

    // Try to pull without a remote
    let result = store.pull_refs("nonexistent");
    assert!(result.is_err());
    if let Err(e) = result {
        assert!(e.to_string().contains("not found"));
    }

    // Cleanup
    fs::remove_dir_all(&local_path)?;
    Ok(())
}

#[test]
fn test_push_with_force_flag() -> Result<()> {
    let (remote_path, _remote_repo) = setup_remote_repo()?;
    let (local_path, local_store) = setup_local_repo_with_remote(&remote_path)?;

    let task = TaskId::new();
    let actor = Actor {
        name: "tester".into(),
        email: "tester@example.invalid".into(),
    };

    let event = Event::new(
        task,
        &actor,
        EventKind::TaskCreated {
            title: "Test task".into(),
            labels: vec![],
            assignees: vec![],
            description: None,
            state: None,
            state_kind: None,
        },
    );

    local_store.append_event(&event)?;
    local_store.push_refs("origin", false)?;

    // Create another event
    let event2 = Event::new(
        task,
        &actor,
        EventKind::LabelsAdded {
            labels: vec!["test".into()],
        },
    );
    local_store.append_event(&event2)?;

    // Force push should succeed
    local_store.push_refs("origin", true)?;

    // Cleanup
    fs::remove_dir_all(&local_path)?;
    fs::remove_dir_all(&remote_path)?;
    Ok(())
}
