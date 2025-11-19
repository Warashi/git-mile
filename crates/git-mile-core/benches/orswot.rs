#![allow(missing_docs)]

use criterion::{BatchSize, BenchmarkId, Criterion, black_box, criterion_group, criterion_main};
use git_mile_core::TaskSnapshot;
use git_mile_core::event::{Actor, Event, EventKind};
use git_mile_core::id::TaskId;

fn build_events(batch_count: usize, labels_per_event: usize) -> Vec<Event> {
    let actor = Actor {
        name: "bench".into(),
        email: "bench@example.invalid".into(),
    };
    let task = TaskId::new();

    let mut events = Vec::with_capacity(batch_count + 1);
    events.push(Event::new(
        task,
        &actor,
        EventKind::TaskCreated {
            title: "bench".into(),
            labels: Vec::new(),
            assignees: Vec::new(),
            description: None,
            state: None,
            state_kind: None,
        },
    ));

    for batch in 0..batch_count {
        let labels = (0..labels_per_event)
            .map(|idx| format!("label-{batch}-{idx}"))
            .collect();
        events.push(Event::new(task, &actor, EventKind::LabelsAdded { labels }));
    }

    events
}

fn replay_labels_benchmark(c: &mut Criterion) {
    let mut group = c.benchmark_group("task_snapshot_replay");
    for &labels_per_event in &[1usize, 4, 16, 32, 64] {
        group.bench_with_input(
            BenchmarkId::from_parameter(labels_per_event),
            &labels_per_event,
            |b, &labels| {
                b.iter_batched(
                    || build_events(64, labels),
                    |events| {
                        black_box(TaskSnapshot::replay(&events));
                    },
                    BatchSize::SmallInput,
                );
            },
        );
    }
    group.finish();
}

criterion_group!(benches, replay_labels_benchmark);
criterion_main!(benches);
