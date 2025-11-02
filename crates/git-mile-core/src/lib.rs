//! Domain types & replay logic for git-mile events.

/// Event payload definitions.
pub mod event;
/// Identifier types.
pub mod id;

use crate::event::{Event, EventKind};
use crate::id::{EventId, TaskId};
use crdts::lwwreg::LWWReg;
use crdts::orswot::Orswot;
use crdts::CmRDT;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::hash::Hash;
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// Materialized view of a task by replaying events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    /// Identifier of the task.
    pub id: TaskId,
    /// Human-readable title.
    pub title: String,
    /// Current state label.
    pub state: Option<String>,
    /// Assigned labels.
    pub labels: BTreeSet<String>,
    /// Current assignees.
    pub assignees: BTreeSet<String>,
    /// Latest description in Markdown.
    pub description: String,
    /// Child tasks.
    pub children: BTreeSet<TaskId>,
    /// Parent tasks.
    pub parents: BTreeSet<TaskId>,
    /// Generic relation buckets.
    pub relates: BTreeMap<String, BTreeSet<TaskId>>,
    /// RFC 3339 timestamp of the most recent event.
    pub updated_rfc3339: Option<String>,
    #[serde(skip)]
    #[serde(default)]
    crdt: TaskCrdt,
}

impl Default for TaskSnapshot {
    fn default() -> Self {
        let crdt = TaskCrdt::default();
        let mut snap = Self {
            id: TaskId::default(),
            title: String::new(),
            state: None,
            labels: BTreeSet::new(),
            assignees: BTreeSet::new(),
            description: String::new(),
            children: BTreeSet::new(),
            parents: BTreeSet::new(),
            relates: BTreeMap::new(),
            updated_rfc3339: None,
            crdt,
        };
        snap.sync_from_crdt();
        snap
    }
}

impl TaskSnapshot {
    /// Apply a single event using CRDT-based aggregation.
    pub fn apply(&mut self, ev: &Event) {
        self.crdt.apply(ev);
        self.sync_from_crdt();
    }

    /// Replay many events in time order.
    #[must_use]
    pub fn replay(mut events: Vec<Event>) -> Self {
        events.sort_by_key(|e| (e.ts, e.id));
        let mut snap = Self::default();
        for e in &events {
            snap.apply(e);
        }
        snap
    }

    fn sync_from_crdt(&mut self) {
        self.id = self.crdt.id.unwrap_or_default();
        self.title = self.crdt.title.val.clone();
        self.state = self.crdt.state.val.clone();
        self.description = self.crdt.description.val.clone();
        self.labels = orswot_to_set(&self.crdt.labels);
        self.assignees = orswot_to_set(&self.crdt.assignees);
        self.children = orswot_to_set(&self.crdt.children);
        self.parents = orswot_to_set(&self.crdt.parents);
        self.relates = self
            .crdt
            .relations
            .iter()
            .map(|(kind, set)| (kind.clone(), orswot_to_set(set)))
            .filter(|(_, members)| !members.is_empty())
            .collect();
        self.updated_rfc3339 = self.crdt.updated.and_then(EventStamp::into_rfc3339);
    }
}

#[derive(Debug, Clone, Default)]
struct TaskCrdt {
    id: Option<TaskId>,
    title: LWWReg<String, EventStamp>,
    state: LWWReg<Option<String>, EventStamp>,
    description: LWWReg<String, EventStamp>,
    labels: Orswot<String, EventId>,
    assignees: Orswot<String, EventId>,
    children: Orswot<TaskId, EventId>,
    parents: Orswot<TaskId, EventId>,
    relations: BTreeMap<String, Orswot<TaskId, EventId>>,
    updated: Option<EventStamp>,
}

impl TaskCrdt {
    fn apply(&mut self, ev: &Event) {
        self.id = Some(ev.task);
        let stamp = EventStamp::from_event(ev);
        self.updated = Some(self.updated.map_or(stamp, |existing| existing.max(stamp)));

        match &ev.kind {
            EventKind::TaskCreated {
                title,
                labels,
                assignees,
                description,
                state,
            } => {
                self.title.update(title.clone(), stamp);
                self.description
                    .update(description.clone().unwrap_or_default(), stamp);
                if let Some(st) = state {
                    self.state.update(Some(st.clone()), stamp);
                }
                add_all(&mut self.labels, labels.iter().cloned(), ev.id);
                add_all(&mut self.assignees, assignees.iter().cloned(), ev.id);
            }
            EventKind::TaskStateSet { state } => {
                self.state.update(Some(state.clone()), stamp);
            }
            EventKind::LabelsAdded { labels } => {
                add_all(&mut self.labels, labels.iter().cloned(), ev.id);
            }
            EventKind::LabelsRemoved { labels } => {
                remove_all(&mut self.labels, labels.iter().cloned());
            }
            EventKind::AssigneesAdded { assignees } => {
                add_all(&mut self.assignees, assignees.iter().cloned(), ev.id);
            }
            EventKind::AssigneesRemoved { assignees } => {
                remove_all(&mut self.assignees, assignees.iter().cloned());
            }
            EventKind::CommentAdded { .. } => {
                // Snapshot ignores comment bodies; updated timestamp handled above.
            }
            EventKind::ChildLinked { parent, child } => {
                if ev.task == *parent {
                    add_single(&mut self.children, *child, ev.id);
                }
                if ev.task == *child {
                    add_single(&mut self.parents, *parent, ev.id);
                }
            }
            EventKind::ChildUnlinked { parent, child } => {
                if ev.task == *parent {
                    remove_all(&mut self.children, std::iter::once(*child));
                }
                if ev.task == *child {
                    remove_all(&mut self.parents, std::iter::once(*parent));
                }
            }
            EventKind::RelationAdded { kind, target } => {
                let entry = self.relations.entry(kind.clone()).or_default();
                add_single(entry, *target, ev.id);
            }
            EventKind::RelationRemoved { kind, target } => {
                if let Some(entry) = self.relations.get_mut(kind) {
                    remove_all(entry, std::iter::once(*target));
                }
                if self
                    .relations
                    .get(kind)
                    .is_some_and(|entry| entry.read().val.is_empty())
                {
                    self.relations.remove(kind);
                }
            }
        }
    }
}

fn add_single<M>(set: &mut Orswot<M, EventId>, member: M, actor: EventId)
where
    M: Clone + Eq + Hash,
{
    add_all(set, std::iter::once(member), actor);
}

fn add_all<M, I>(set: &mut Orswot<M, EventId>, members: I, actor: EventId)
where
    M: Clone + Eq + Hash,
    I: IntoIterator<Item = M>,
{
    let collected: Vec<M> = members.into_iter().collect();
    if collected.is_empty() {
        return;
    }
    let ctx = set.read_ctx().derive_add_ctx(actor);
    set.apply(set.add_all(collected, ctx));
}

fn remove_all<M, I>(set: &mut Orswot<M, EventId>, members: I)
where
    M: Clone + Eq + Hash,
    I: IntoIterator<Item = M>,
{
    for member in members {
        let read = set.contains(&member);
        let (present, ctx) = read.split();
        if present {
            let rm_ctx = ctx.derive_rm_ctx();
            set.apply(set.rm(member, rm_ctx));
        }
    }
}

fn orswot_to_set<M>(set: &Orswot<M, EventId>) -> BTreeSet<M>
where
    M: Ord + Clone + Eq + Hash,
{
    set.read().val.into_iter().collect()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
struct EventStamp {
    ts: OffsetDateTime,
    id: EventId,
}

impl Default for EventStamp {
    fn default() -> Self {
        Self {
            ts: OffsetDateTime::UNIX_EPOCH,
            id: EventId::default(),
        }
    }
}

impl EventStamp {
    const fn from_event(ev: &Event) -> Self {
        Self { ts: ev.ts, id: ev.id }
    }

    fn max(self, other: Self) -> Self {
        if other > self {
            other
        } else {
            self
        }
    }

    fn into_rfc3339(self) -> Option<String> {
        self.ts.format(&Rfc3339).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{Actor, Event, EventKind};
    use time::{Duration, OffsetDateTime};

    #[test]
    fn apply_and_replay_updates_snapshot() {
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let mut created = Event::new(
            task,
            actor.clone(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: vec!["type/bug".into()],
                assignees: vec!["alice".into()],
                description: Some("desc".into()),
                state: Some("state/todo".into()),
            },
        );
        created.ts = OffsetDateTime::now_utc() - Duration::seconds(10);

        let mut label_removed = Event::new(
            task,
            actor.clone(),
            EventKind::LabelsRemoved {
                labels: vec!["type/bug".into()],
            },
        );
        label_removed.ts = created.ts + Duration::seconds(5);

        let mut state_set = Event::new(
            task,
            actor.clone(),
            EventKind::TaskStateSet {
                state: "state/done".into(),
            },
        );
        state_set.ts = label_removed.ts + Duration::seconds(5);

        let mut label_readd = Event::new(
            task,
            actor,
            EventKind::LabelsAdded {
                labels: vec!["type/bug".into()],
            },
        );
        label_readd.ts = state_set.ts + Duration::seconds(5);

        let expected_ts = label_readd
            .ts
            .format(&Rfc3339)
            .unwrap_or_else(|err| panic!("timestamp must format: {err}"));
        let snapshot = TaskSnapshot::replay(vec![label_readd, state_set, label_removed, created]);

        assert_eq!(snapshot.id, task);
        assert_eq!(snapshot.title, "Initial");
        assert_eq!(snapshot.description, "desc");
        assert_eq!(snapshot.state.as_deref(), Some("state/done"));
        assert!(snapshot.assignees.contains("alice"));
        assert!(snapshot.labels.contains("type/bug"));
        assert_eq!(snapshot.updated_rfc3339.as_deref(), Some(expected_ts.as_str()));
    }

    #[test]
    fn apply_matches_replay_results() {
        let task = TaskId::new();
        let actor = Actor {
            name: "tester".into(),
            email: "tester@example.invalid".into(),
        };

        let child = TaskId::new();
        let related = TaskId::new();
        let relation_kind = "relatesTo".to_owned();

        let mut created = Event::new(
            task,
            actor.clone(),
            EventKind::TaskCreated {
                title: "Initial".into(),
                labels: vec!["type/bug".into()],
                assignees: vec!["alice".into()],
                description: Some("desc".into()),
                state: Some("state/todo".into()),
            },
        );
        created.ts = OffsetDateTime::now_utc() - Duration::seconds(30);

        let mut link_child = Event::new(
            task,
            actor.clone(),
            EventKind::ChildLinked { parent: task, child },
        );
        link_child.ts = created.ts + Duration::seconds(5);

        let mut unlink_child = Event::new(
            task,
            actor.clone(),
            EventKind::ChildUnlinked { parent: task, child },
        );
        unlink_child.ts = link_child.ts + Duration::seconds(5);

        let mut relation_add = Event::new(
            task,
            actor.clone(),
            EventKind::RelationAdded {
                kind: relation_kind.clone(),
                target: related,
            },
        );
        relation_add.ts = unlink_child.ts + Duration::seconds(5);

        let mut relation_rm = Event::new(
            task,
            actor,
            EventKind::RelationRemoved {
                kind: relation_kind,
                target: related,
            },
        );
        relation_rm.ts = relation_add.ts + Duration::seconds(5);

        let events = vec![created, link_child, unlink_child, relation_add, relation_rm];

        let mut via_apply = TaskSnapshot::default();
        for event in &events {
            via_apply.apply(event);
        }

        let mut reversed = events.clone();
        reversed.reverse();
        let via_replay = TaskSnapshot::replay(reversed);

        assert_eq!(via_apply.title, via_replay.title);
        assert_eq!(via_apply.labels, via_replay.labels);
        assert_eq!(via_apply.assignees, via_replay.assignees);
        assert_eq!(via_apply.children, via_replay.children);
        assert_eq!(via_apply.parents, via_replay.parents);
        assert_eq!(via_apply.relates, via_replay.relates);
        assert_eq!(via_apply.updated_rfc3339, via_replay.updated_rfc3339);
    }
}
