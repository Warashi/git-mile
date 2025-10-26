#![cfg(feature = "property-tests")]

use proptest::prelude::*;

use git_mile_core::clock::{LamportTimestamp, ReplicaId};
use git_mile_core::issue::{IssueId, IssueStatus};
use git_mile_core::model::{Comment, CommentParent, IssueDetails, Markdown};
use git_mile_core::query::{QueryEngine, QueryRequest, issue_schema, parse_query};

fn issue_status_strategy() -> impl Strategy<Value = IssueStatus> {
    prop_oneof![
        Just(IssueStatus::Open),
        Just(IssueStatus::Draft),
        Just(IssueStatus::Closed),
    ]
}

fn make_issue(index: usize, status: IssueStatus) -> IssueDetails {
    let id = IssueId::new();
    let replica = ReplicaId::new("property-query");
    let created = LamportTimestamp::new((index as u64) + 1, replica.clone());
    let updated = LamportTimestamp::new((index as u64) + 2, replica);
    let comment = Comment {
        id: git_mile_core::model::CommentId::new_v4(),
        parent: CommentParent::Issue(id.clone()),
        body_markdown: Markdown::new(format!("Initial comment for {index}")),
        author_id: "bench".into(),
        created_at: created.clone(),
        edited_at: None,
    };

    let mut labels = std::collections::BTreeSet::new();
    labels.insert(format!("team-{}", index % 3));

    IssueDetails {
        id,
        title: format!("Issue {index}"),
        description: Some(Markdown::new("property test")),
        status,
        initial_comment_id: Some(comment.id),
        labels,
        comments: vec![comment],
        label_events: Vec::new(),
        created_at: created,
        updated_at: updated.clone(),
        clock_snapshot: updated,
    }
}

proptest! {
    #[test]
    fn status_filter_matches_manual(statuses in prop::collection::vec(issue_status_strategy(), 1..48), target in issue_status_strategy()) {
        let dataset: Vec<IssueDetails> = statuses
            .iter()
            .enumerate()
            .map(|(idx, status)| make_issue(idx, *status))
            .collect();

        let schema = issue_schema();
        let engine = QueryEngine::new(schema);
        let filter = parse_query(&format!("(= status \"{}\")", target)).expect("parse filter");
        let request = QueryRequest {
            filter: Some(filter),
            sort: vec![],
            limit: None,
            cursor: None,
        };

        let expected: Vec<IssueDetails> = dataset
            .iter()
            .filter(|issue| issue.status == target)
            .cloned()
            .collect();
        let response = engine
            .execute(dataset.iter().cloned(), &request, None)
            .expect("execute query");

        prop_assert_eq!(response.items, expected);
    }

    #[test]
    fn parser_handles_arbitrary_input(raw in proptest::collection::vec(any::<char>(), 0..64)) {
        let input: String = raw.into_iter().collect();
        let candidate = format!("({input})");
        let _ = parse_query(&candidate);
    }
}
