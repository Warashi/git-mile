use git_mile_core::id::TaskId;

use crate::task_writer::TaskStore;

use super::view::{DetailFocus, Ui};

/// Tree node for hierarchical task display.
#[derive(Debug, Clone)]
pub(super) struct TreeNode {
    /// Task ID.
    pub(super) task_id: TaskId,
    /// Child nodes.
    pub(super) children: Vec<TreeNode>,
    /// Whether this node is expanded.
    pub(super) expanded: bool,
}

/// State for tree view navigation.
#[derive(Debug, Clone)]
pub(super) struct TreeViewState {
    /// Root nodes of the tree.
    pub(super) roots: Vec<TreeNode>,
    /// Flattened list of visible nodes (for navigation).
    pub(super) visible_nodes: Vec<(usize, TaskId)>, // (depth, task_id)
    /// Currently selected index in `visible_nodes`.
    pub(super) selected: usize,
}

impl TreeViewState {
    pub(super) const fn new() -> Self {
        Self {
            roots: Vec::new(),
            visible_nodes: Vec::new(),
            selected: 0,
        }
    }

    /// Rebuild visible nodes list from roots.
    pub(super) fn rebuild_visible_nodes(&mut self) {
        self.visible_nodes.clear();
        for root in &self.roots {
            Self::collect_visible_nodes_into(root, 0, &mut self.visible_nodes);
        }
    }

    /// Expand ancestors so the given task is visible.
    pub(super) fn expand_to_task(&mut self, task_id: TaskId) {
        for root in &mut self.roots {
            if Self::expand_path_to_task(root, task_id) {
                break;
            }
        }
    }

    /// Get currently selected task ID.
    pub(super) fn selected_task_id(&self) -> Option<TaskId> {
        self.visible_nodes.get(self.selected).map(|(_, id)| *id)
    }

    /// Find node by task ID (mutable).
    pub(super) fn find_node_mut(&mut self, task_id: TaskId) -> Option<&mut TreeNode> {
        for root in &mut self.roots {
            if let Some(node) = Self::find_node_in_tree_mut(root, task_id) {
                return Some(node);
            }
        }
        None
    }

    fn collect_visible_nodes_into(node: &TreeNode, depth: usize, visible_nodes: &mut Vec<(usize, TaskId)>) {
        visible_nodes.push((depth, node.task_id));
        if node.expanded {
            for child in &node.children {
                Self::collect_visible_nodes_into(child, depth + 1, visible_nodes);
            }
        }
    }

    fn expand_path_to_task(node: &mut TreeNode, task_id: TaskId) -> bool {
        if node.task_id == task_id {
            return true;
        }
        for child in &mut node.children {
            if Self::expand_path_to_task(child, task_id) {
                node.expanded = true;
                return true;
            }
        }
        false
    }

    fn find_node_in_tree_mut(node: &mut TreeNode, task_id: TaskId) -> Option<&mut TreeNode> {
        if node.task_id == task_id {
            return Some(node);
        }
        for child in &mut node.children {
            if let Some(found) = Self::find_node_in_tree_mut(child, task_id) {
                return Some(found);
            }
        }
        None
    }
}

impl<S: TaskStore> Ui<S> {
    pub(in crate::tui) fn open_tree_view(&mut self) {
        let Some(current_task) = self.selected_task() else {
            self.error("タスクが選択されていません");
            return;
        };

        let current_id = current_task.snapshot.id;
        let Some(root_task) = self.app.get_root(current_id) else {
            self.error("ルートタスクが見つかりません");
            return;
        };

        let root_id = root_task.snapshot.id;
        let Some(tree) = self.build_tree_from_root(root_id) else {
            self.error("ツリーの構築に失敗しました");
            return;
        };

        self.tree_state.roots = vec![tree];
        self.tree_state.expand_to_task(current_id);
        self.tree_state.rebuild_visible_nodes();

        if let Some(index) = self
            .tree_state
            .visible_nodes
            .iter()
            .position(|(_, id)| *id == current_id)
        {
            self.tree_state.selected = index;
        }

        self.detail_focus = DetailFocus::TreeView;
    }

    pub(in crate::tui) const fn tree_view_down(&mut self) {
        if self.tree_state.selected + 1 < self.tree_state.visible_nodes.len() {
            self.tree_state.selected += 1;
        }
    }

    pub(in crate::tui) const fn tree_view_up(&mut self) {
        if self.tree_state.selected > 0 {
            self.tree_state.selected -= 1;
        }
    }

    pub(in crate::tui) fn tree_view_collapse(&mut self) {
        let Some(task_id) = self.tree_state.selected_task_id() else {
            return;
        };

        if let Some(node) = self.tree_state.find_node_mut(task_id) {
            if node.expanded && !node.children.is_empty() {
                node.expanded = false;
                self.tree_state.rebuild_visible_nodes();
            } else {
                self.move_to_parent_in_tree(task_id);
            }
        }
    }

    pub(in crate::tui) fn tree_view_expand(&mut self) {
        let Some(task_id) = self.tree_state.selected_task_id() else {
            return;
        };

        let children = self.app.get_children(task_id);
        if children.is_empty() {
            return;
        }

        if let Some(node) = self.tree_state.find_node_mut(task_id) {
            if node.expanded {
                if self.tree_state.selected + 1 < self.tree_state.visible_nodes.len() {
                    self.tree_state.selected += 1;
                }
            } else {
                node.expanded = true;
                self.tree_state.rebuild_visible_nodes();
            }
        }
    }

    pub(in crate::tui) fn tree_view_jump(&mut self) {
        let Some(task_id) = self.tree_state.selected_task_id() else {
            return;
        };

        self.app.visibility_mut().jump_to_task(task_id);
        self.detail_focus = DetailFocus::None;

        if let Some(task) = self.selected_task() {
            self.info(format!("タスクへジャンプ: {}", task.snapshot.title));
        }
    }

    fn build_tree_from_root(&self, root_id: TaskId) -> Option<TreeNode> {
        let root_view = self.app.tasks.iter().find(|t| t.snapshot.id == root_id)?;

        if !self.app.visibility().contains(root_id) {
            return None;
        }

        Some(TreeNode {
            task_id: root_view.snapshot.id,
            children: self.build_children_nodes(root_id),
            expanded: true,
        })
    }

    fn build_children_nodes(&self, parent_id: TaskId) -> Vec<TreeNode> {
        let children = self.app.get_children(parent_id);
        children
            .iter()
            .filter(|child| self.app.visibility().contains(child.snapshot.id))
            .map(|child| TreeNode {
                task_id: child.snapshot.id,
                children: self.build_children_nodes(child.snapshot.id),
                expanded: false,
            })
            .collect()
    }

    fn move_to_parent_in_tree(&mut self, task_id: TaskId) {
        let parents = self.app.get_parents(task_id);
        if let Some(parent) = parents.first()
            && let Some(index) = self
                .tree_state
                .visible_nodes
                .iter()
                .position(|(_, id)| *id == parent.snapshot.id)
        {
            self.tree_state.selected = index;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_path_to_selected_task() {
        let child_id = TaskId::new();
        let root_id = TaskId::new();
        let mut state = TreeViewState {
            roots: vec![TreeNode {
                task_id: root_id,
                children: vec![TreeNode {
                    task_id: child_id,
                    children: Vec::new(),
                    expanded: false,
                }],
                expanded: false,
            }],
            visible_nodes: Vec::new(),
            selected: 0,
        };

        state.expand_to_task(child_id);
        assert!(state.roots[0].expanded, "root should be expanded to reveal child");
        assert_eq!(state.roots[0].task_id, root_id);
    }

    #[test]
    fn rebuild_visible_nodes_includes_children() {
        let child_id = TaskId::new();
        let root_id = TaskId::new();
        let mut state = TreeViewState {
            roots: vec![TreeNode {
                task_id: root_id,
                children: vec![TreeNode {
                    task_id: child_id,
                    children: Vec::new(),
                    expanded: false,
                }],
                expanded: true,
            }],
            visible_nodes: Vec::new(),
            selected: 0,
        };

        state.rebuild_visible_nodes();
        assert_eq!(state.visible_nodes.len(), 2);
        assert_eq!(state.visible_nodes[0], (0, root_id));
        assert_eq!(state.visible_nodes[1], (1, child_id));
    }

    #[test]
    fn selected_task_id_tracks_current_row() {
        let mut state = TreeViewState {
            roots: vec![TreeNode {
                task_id: TaskId::new(),
                children: Vec::new(),
                expanded: true,
            }],
            visible_nodes: Vec::new(),
            selected: 0,
        };
        state.rebuild_visible_nodes();
        assert_eq!(state.selected_task_id(), Some(state.roots[0].task_id));
    }
}
