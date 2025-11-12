use std::collections::HashMap;

use git_mile_core::TaskFilter;
use git_mile_core::id::TaskId;

use git_mile_app::TaskView;

/// Manages task visibility, filters, and selection independent of IO.
#[derive(Debug, Default)]
pub(super) struct TaskVisibility {
    filter: TaskFilter,
    visible: Vec<usize>,
    visible_index: HashMap<TaskId, usize>,
    selected: usize,
}

#[allow(clippy::missing_const_for_fn)]
impl TaskVisibility {
    pub(super) fn filter(&self) -> &TaskFilter {
        &self.filter
    }

    pub(super) fn set_filter(&mut self, filter: TaskFilter) {
        self.filter = filter;
    }

    pub(super) fn rebuild(&mut self, tasks: &[TaskView], preferred: Option<TaskId>) {
        self.visible.clear();
        self.visible_index.clear();

        if tasks.is_empty() {
            self.selected = 0;
            return;
        }

        if self.filter.is_empty() {
            for (idx, view) in tasks.iter().enumerate() {
                let pos = self.visible.len();
                self.visible.push(idx);
                self.visible_index.insert(view.snapshot.id, pos);
            }
        } else {
            for (idx, view) in tasks.iter().enumerate() {
                if self.filter.matches(&view.snapshot) {
                    let pos = self.visible.len();
                    self.visible.push(idx);
                    self.visible_index.insert(view.snapshot.id, pos);
                }
            }
        }

        self.selected = self.resolve_selection(preferred);
    }

    fn resolve_selection(&self, preferred: Option<TaskId>) -> usize {
        if self.visible.is_empty() {
            return 0;
        }
        if let Some(id) = preferred
            && let Some(&index) = self.visible_index.get(&id)
        {
            return index;
        }
        self.selected.min(self.visible.len() - 1)
    }

    pub(super) fn has_visible_tasks(&self) -> bool {
        !self.visible.is_empty()
    }

    #[cfg(test)]
    pub(super) fn visible_indexes(&self) -> &[usize] {
        &self.visible
    }

    pub(super) fn visible_tasks<'a>(
        &'a self,
        tasks: &'a [TaskView],
    ) -> impl Iterator<Item = &'a TaskView> + 'a {
        self.visible.iter().filter_map(move |&idx| tasks.get(idx))
    }

    pub(super) fn contains(&self, task_id: TaskId) -> bool {
        self.visible_index.contains_key(&task_id)
    }

    pub(super) fn selected_index(&self) -> usize {
        self.selected
    }

    pub(super) fn selected_task<'a>(&self, tasks: &'a [TaskView]) -> Option<&'a TaskView> {
        self.visible.get(self.selected).and_then(|&idx| tasks.get(idx))
    }

    pub(super) fn selected_task_id(&self, tasks: &[TaskView]) -> Option<TaskId> {
        self.selected_task(tasks).map(|view| view.snapshot.id)
    }

    pub(super) fn select_next(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        if self.selected + 1 < self.visible.len() {
            self.selected += 1;
        }
    }

    pub(super) fn select_prev(&mut self) {
        if self.visible.is_empty() {
            return;
        }
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub(super) fn jump_to_task(&mut self, task_id: TaskId) {
        if let Some(index) = self.visible_index.get(&task_id).copied() {
            self.selected = index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_mile_core::TaskSnapshot;
    use std::str::FromStr;

    fn view(id: &str, title: &str) -> TaskView {
        let mut snapshot = TaskSnapshot::default();
        snapshot.id = TaskId::from_str(id).unwrap_or_else(|err| panic!("valid id: {err}"));
        snapshot.title = title.to_owned();
        TaskView {
            snapshot,
            comments: Vec::new(),
            last_updated: None,
        }
    }

    #[test]
    fn rebuild_without_filter_lists_all_tasks() {
        let tasks = vec![
            view("00000000-0000-0000-0000-000000000001", "one"),
            view("00000000-0000-0000-0000-000000000002", "two"),
        ];
        let mut visibility = TaskVisibility::default();
        visibility.rebuild(&tasks, None);

        assert_eq!(visibility.visible_indexes(), &[0, 1]);
        assert_eq!(visibility.selected_index(), 0);
        assert_eq!(visibility.selected_task_id(&tasks), Some(tasks[0].snapshot.id));
    }

    #[test]
    fn rebuild_applies_filter_and_keeps_preferred_selection() {
        let tasks = vec![
            view("00000000-0000-0000-0000-000000000010", "Root"),
            view("00000000-0000-0000-0000-000000000011", "Child"),
            view("00000000-0000-0000-0000-000000000012", "Grandchild"),
        ];

        let mut visibility = TaskVisibility::default();
        visibility.set_filter(TaskFilter {
            text: Some("child".into()),
            ..TaskFilter::default()
        });
        let preferred = Some(tasks[2].snapshot.id);
        visibility.rebuild(&tasks, preferred);

        assert_eq!(visibility.visible_indexes(), &[1, 2]);
        assert_eq!(visibility.selected_task_id(&tasks), Some(tasks[2].snapshot.id));
    }

    #[test]
    fn navigation_wraps_within_bounds() {
        let tasks = vec![
            view("00000000-0000-0000-0000-000000000020", "first"),
            view("00000000-0000-0000-0000-000000000021", "second"),
        ];
        let mut visibility = TaskVisibility::default();
        visibility.rebuild(&tasks, None);
        visibility.select_next();
        assert_eq!(visibility.selected_task_id(&tasks), Some(tasks[1].snapshot.id));
        visibility.select_next();
        assert_eq!(visibility.selected_task_id(&tasks), Some(tasks[1].snapshot.id));
        visibility.select_prev();
        assert_eq!(visibility.selected_task_id(&tasks), Some(tasks[0].snapshot.id));
        visibility.select_prev();
        assert_eq!(visibility.selected_task_id(&tasks), Some(tasks[0].snapshot.id));
    }

    #[test]
    fn jump_to_task_updates_selection_when_visible() {
        let tasks = vec![
            view("00000000-0000-0000-0000-000000000030", "first"),
            view("00000000-0000-0000-0000-000000000031", "second"),
        ];
        let mut visibility = TaskVisibility::default();
        visibility.rebuild(&tasks, None);

        visibility.jump_to_task(tasks[1].snapshot.id);
        assert_eq!(visibility.selected_task_id(&tasks), Some(tasks[1].snapshot.id));
        visibility.jump_to_task(TaskId::new());
        assert_eq!(visibility.selected_task_id(&tasks), Some(tasks[1].snapshot.id));
    }

    #[test]
    fn filter_only_keeps_matching_tasks() {
        let tasks = vec![
            view("00000000-0000-0000-0000-000000000040", "Root"),
            view("00000000-0000-0000-0000-000000000041", "Child"),
            view("00000000-0000-0000-0000-000000000042", "Grandchild"),
        ];

        let mut visibility = TaskVisibility::default();
        visibility.set_filter(TaskFilter {
            text: Some("Grand".into()),
            ..TaskFilter::default()
        });
        visibility.rebuild(&tasks, None);

        let titles: Vec<&str> = visibility
            .visible_tasks(&tasks)
            .map(|view| view.snapshot.title.as_str())
            .collect();
        assert_eq!(titles, vec!["Grandchild"]);
    }

    #[test]
    fn filter_matches_parent_without_including_children() {
        let tasks = vec![
            view("00000000-0000-0000-0000-000000000050", "Root"),
            view("00000000-0000-0000-0000-000000000051", "Child"),
        ];

        let mut visibility = TaskVisibility::default();
        visibility.set_filter(TaskFilter {
            text: Some("Root".into()),
            ..TaskFilter::default()
        });
        visibility.rebuild(&tasks, None);

        let titles: Vec<&str> = visibility
            .visible_tasks(&tasks)
            .map(|view| view.snapshot.title.as_str())
            .collect();
        assert_eq!(titles, vec!["Root"]);
    }
}
