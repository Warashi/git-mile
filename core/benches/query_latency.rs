use std::sync::Arc;

use criterion::{Criterion, criterion_group, criterion_main};
use git_mile_core::clock::{LamportTimestamp, ReplicaId};
use git_mile_core::model::{Comment, CommentParent, IssueDetails, Markdown};
use git_mile_core::query::{QueryEngine, QueryRequest, issue_schema, parse_query};

fn issue_dataset(size: usize) -> Arc<Vec<IssueDetails>> {
    let replica = ReplicaId::new("bench");
    let mut items = Vec::with_capacity(size);
    for index in 0..size {
        let id = git_mile_core::issue::IssueId::new();
        let created = LamportTimestamp::new(index as u64 + 1, replica.clone());
        let updated = LamportTimestamp::new(index as u64 + 2, replica.clone());
        let comment = Comment {
            id: git_mile_core::model::CommentId::new_v4(),
            parent: CommentParent::Issue(id.clone()),
            body_markdown: Markdown::new(format!("Discussion for issue {index}")),
            author_id: "bench".into(),
            created_at: created.clone(),
            edited_at: None,
        };

        let status = if index % 3 == 0 {
            git_mile_core::issue::IssueStatus::Draft
        } else if index % 5 == 0 {
            git_mile_core::issue::IssueStatus::Closed
        } else {
            git_mile_core::issue::IssueStatus::Open
        };

        let mut labels = std::collections::BTreeSet::new();
        if index % 2 == 0 {
            labels.insert("bug".to_string());
        }
        labels.insert(format!("team-{}", index % 4));

        items.push(IssueDetails {
            id,
            title: format!("Benchmark Issue {index}"),
            description: Some(Markdown::new("Benchmark payload")),
            status,
            initial_comment_id: Some(comment.id),
            labels,
            comments: vec![comment],
            label_events: Vec::new(),
            created_at: created.clone(),
            updated_at: updated.clone(),
            clock_snapshot: updated,
        });
    }

    Arc::new(items)
}

fn bench_issue_query(c: &mut Criterion) {
    let schema = issue_schema();
    let engine = QueryEngine::new(schema.clone());
    let dataset = issue_dataset(2_000);
    let filter = parse_query("(= status \"open\")").expect("parse filter");
    let request = QueryRequest {
        filter: Some(filter),
        sort: vec![],
        limit: Some(100),
        cursor: None,
    };

    c.bench_function("issue_query_open_status", |b| {
        b.iter(|| {
            let iter = dataset.iter().cloned();
            let _ = engine.execute(iter, &request, None).expect("query");
        });
    });
}

criterion_group!(query_benches, bench_issue_query);
criterion_main!(query_benches);
