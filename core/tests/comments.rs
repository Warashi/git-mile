use git_mile_core::clock::ReplicaId;
use git_mile_core::issue::{AppendIssueCommentInput, CreateIssueInput, IssueStatus, IssueStore};
use git_mile_core::mile::{AppendCommentInput, CreateMileInput, MileStatus, MileStore};

#[test]
fn issue_snapshot_roundtrips_description_comments_and_labels() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let store = IssueStore::open(temp.path()).expect("open issue store");
    let replica = "fixture-issue";

    let issue_id = store
        .create_issue(CreateIssueInput {
            replica_id: ReplicaId::new(replica),
            author: "tester".into(),
            message: Some("create".into()),
            title: "Snapshot Issue".into(),
            description: Some("## Heading\n\nBody text".into()),
            initial_status: IssueStatus::Open,
            initial_comment: Some("First comment".into()),
            labels: vec!["alpha".into(), "beta".into()],
        })
        .expect("create issue")
        .id;

    let outcome = store
        .append_comment(AppendIssueCommentInput {
            issue_id,
            replica_id: ReplicaId::new(replica),
            author: "tester".into(),
            message: Some("second".into()),
            comment_id: None,
            body: "Follow up".into(),
        })
        .expect("append comment");

    let serialized = serde_json::to_string(&outcome.snapshot).expect("serialize snapshot");
    let decoded: serde_json::Value = serde_json::from_str(&serialized).expect("decode snapshot");

    assert_eq!(decoded["description"], "## Heading\n\nBody text");
    let labels = decoded["labels"].as_array().expect("labels array");
    assert!(labels.iter().any(|value| value == "alpha"));
    assert!(labels.iter().any(|value| value == "beta"));
    let comments = decoded["comments"].as_array().expect("comments array");
    assert_eq!(comments.len(), 2);
    assert_eq!(comments[0]["body"], "First comment");
    assert_eq!(comments[1]["body"], "Follow up");
}

#[test]
fn milestone_comments_remain_in_lamport_order() {
    let temp = tempfile::tempdir().expect("create temp dir");
    let store = MileStore::open(temp.path()).expect("open milestone store");
    let primary = "replica-primary";
    let secondary = "replica-secondary";

    let mile_id = store
        .create_mile(CreateMileInput {
            replica_id: ReplicaId::new(primary),
            author: "tester".into(),
            message: Some("create".into()),
            title: "Milestone".into(),
            description: Some("Summary".into()),
            initial_status: MileStatus::Open,
            initial_comment: Some("Kickoff".into()),
            labels: vec![],
        })
        .expect("create milestone")
        .id;

    store
        .append_comment(AppendCommentInput {
            mile_id: mile_id.clone(),
            replica_id: ReplicaId::new(secondary),
            author: "other".into(),
            message: Some("second".into()),
            comment_id: None,
            body: "Update from secondary".into(),
        })
        .expect("append from secondary");

    store
        .append_comment(AppendCommentInput {
            mile_id: mile_id.clone(),
            replica_id: ReplicaId::new(primary),
            author: "tester".into(),
            message: Some("third".into()),
            comment_id: None,
            body: "Primary follow up".into(),
        })
        .expect("append from primary");

    let refreshed = store.load_mile(&mile_id).expect("load milestone snapshot");

    assert_eq!(refreshed.comments.len(), 3);
    let timestamps: Vec<_> = refreshed
        .comments
        .iter()
        .map(|comment| comment.created_at.clone())
        .collect();
    let mut sorted = timestamps.clone();
    sorted.sort();
    assert_eq!(
        timestamps, sorted,
        "comments should remain in Lamport order"
    );
}
