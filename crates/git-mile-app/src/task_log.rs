//! Utility helpers for task event logs shared across interfaces.

use git_mile_core::OrderedEvents;
use git_mile_core::event::Event;

/// Return events ordered by lamport clock, timestamp, and event id.
#[must_use]
pub fn ordered_events(events: &[Event]) -> Vec<Event> {
    OrderedEvents::from(events).iter().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::event::{Actor, EventKind};
    use git_mile_core::id::TaskId;
    use time::Duration;

    #[test]
    fn ordered_events_sorts_by_lamport_then_timestamp_then_id() {
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let mut first = Event::new(
            task,
            &actor,
            EventKind::TaskCreated {
                title: "first".into(),
                labels: vec![],
                assignees: vec![],
                description: None,
                state: None,
                state_kind: None,
            },
        );
        first.lamport = 2;
        first.ts += Duration::seconds(10);

        let mut second = Event::new(task, &actor, EventKind::TaskStateCleared);
        second.lamport = 1;
        second.ts += Duration::seconds(20);

        let mut third = Event::new(
            task,
            &actor,
            EventKind::TaskTitleSet {
                title: "later".into(),
            },
        );
        third.lamport = 2;
        third.ts += Duration::seconds(5);

        let ordered = ordered_events(&[first.clone(), second.clone(), third.clone()]);
        let ids: Vec<_> = ordered.into_iter().map(|event| event.id).collect();

        assert_eq!(vec![second.id, third.id, first.id], ids);
    }
}
