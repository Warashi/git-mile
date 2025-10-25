use git_mile_core::clock::ReplicaId;
use git_mile_core::issue::{CreateIssueInput, IssueStatus, IssueStore, UpdateIssueLabelsInput};
use git_mile_core::mile::{CreateMileInput, MileStatus, MileStore, UpdateLabelsInput};
use git_mile_core::service::MilestoneService;

#[test]
fn issue_label_updates_apply_additions_and_removals() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let store = IssueStore::open(temp.path()).expect("open issue store");
    let replica = ReplicaId::new("issue-labels");

    let snapshot = store
        .create_issue(CreateIssueInput {
            replica_id: replica.clone(),
            author: "tester".into(),
            message: Some("create".into()),
            title: "Label Issue".into(),
            description: None,
            initial_status: IssueStatus::Open,
            initial_comment: None,
            labels: vec!["alpha".into(), "beta".into()],
        })
        .expect("create issue");

    let outcome = store
        .update_labels(UpdateIssueLabelsInput {
            issue_id: snapshot.id.clone(),
            replica_id: replica.clone(),
            author: "tester".into(),
            message: Some("update labels".into()),
            add: vec!["gamma".into(), "beta".into()],
            remove: vec!["alpha".into()],
        })
        .expect("update labels");

    assert!(outcome.changed);
    assert_eq!(outcome.added, vec![String::from("gamma")]);
    assert_eq!(outcome.removed, vec![String::from("alpha")]);
    assert!(outcome.snapshot.labels.contains("beta"));
    assert!(outcome.snapshot.labels.contains("gamma"));
    assert!(!outcome.snapshot.labels.contains("alpha"));
}

#[test]
fn milestone_label_events_capture_history() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let store = MileStore::open(temp.path()).expect("open milestone store");
    let replica = ReplicaId::new("milestone-labels");

    let snapshot = store
        .create_mile(CreateMileInput {
            replica_id: replica.clone(),
            author: "tester".into(),
            message: Some("create".into()),
            title: "Label Milestone".into(),
            description: None,
            initial_status: MileStatus::Draft,
            initial_comment: None,
            labels: vec![],
        })
        .expect("create milestone");

    store
        .update_labels(UpdateLabelsInput {
            mile_id: snapshot.id.clone(),
            replica_id: replica.clone(),
            author: "tester".into(),
            message: Some("add labels".into()),
            add: vec!["alpha".into(), "beta".into()],
            remove: vec![],
        })
        .expect("add labels");

    store
        .update_labels(UpdateLabelsInput {
            mile_id: snapshot.id.clone(),
            replica_id: replica.clone(),
            author: "tester".into(),
            message: Some("remove label".into()),
            add: vec![],
            remove: vec!["alpha".into()],
        })
        .expect("remove label");

    drop(store);

    let service = MilestoneService::open(temp.path()).expect("open milestone service");
    let details = service
        .get_with_comments(&snapshot.id)
        .expect("load milestone details");

    assert_eq!(details.labels.len(), 1);
    assert!(details.labels.contains(&"beta".to_string()));
    assert_eq!(details.label_events.len(), 3);
    assert_eq!(details.label_events[0].label_id, "alpha");
    assert_eq!(details.label_events[1].label_id, "beta");
    assert_eq!(details.label_events[2].label_id, "alpha");
}
