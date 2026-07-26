use std::collections::BTreeSet;

use ratatui::widgets::{ListState, TableState};

use crate::ids::TaskId;
use crate::tui::app::{DetailSection, DetailTargetId};
use crate::tui::bounded_history::BoundedHistory;
use crate::tui::store::TaskViewState;

const NAVIGATION_HISTORY_LIMIT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Tasks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LastChangeReturnState {
    pub(crate) view_state: TaskViewState,
    pub(crate) selected_task_id: Option<TaskId>,
    pub(crate) selected_index: Option<usize>,
    pub(crate) table_offset: usize,
    pub(crate) return_to_detail: bool,
    pub(crate) detail_scroll: u16,
    pub(crate) detail_focus: Option<DetailTargetId>,
    pub(crate) detail_expanded_sections: BTreeSet<DetailSection>,
}

pub(crate) struct ListSurface {
    sidebar: ListState,
    table: TableState,
    marked_task_ids: BTreeSet<TaskId>,
    focus: Focus,
    sidebar_visible: bool,
    navigation_history: BoundedHistory<TaskViewState>,
    last_changed_task_id: Option<TaskId>,
    last_change_return: Option<LastChangeReturnState>,
}

impl ListSurface {
    pub(crate) fn new(has_tasks: bool) -> Self {
        let mut table = TableState::default();
        table.select(has_tasks.then_some(0));
        Self {
            sidebar: ListState::default(),
            table,
            marked_task_ids: BTreeSet::new(),
            focus: Focus::Tasks,
            sidebar_visible: true,
            navigation_history: BoundedHistory::new(NAVIGATION_HISTORY_LIMIT),
            last_changed_task_id: None,
            last_change_return: None,
        }
    }

    pub(crate) fn focus(&self) -> Focus {
        self.focus
    }

    pub(crate) fn focus_tasks(&mut self) {
        self.focus = Focus::Tasks;
    }

    pub(crate) fn focus_sidebar(&mut self) {
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
    }

    pub(crate) fn toggle_focus(&mut self) -> bool {
        if !self.sidebar_visible && self.focus == Focus::Tasks {
            self.focus_sidebar();
            return true;
        }
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Tasks,
            Focus::Tasks => Focus::Sidebar,
        };
        false
    }

    pub(crate) fn sidebar_visible(&self) -> bool {
        self.sidebar_visible
    }

    pub(crate) fn toggle_sidebar(&mut self) -> bool {
        self.sidebar_visible = !self.sidebar_visible;
        if self.sidebar_visible {
            self.focus = Focus::Sidebar;
        } else {
            self.focus = Focus::Tasks;
        }
        self.sidebar_visible
    }

    #[cfg(test)]
    pub(crate) fn hide_sidebar(&mut self) {
        self.sidebar_visible = false;
        self.focus = Focus::Tasks;
    }

    pub(crate) fn selected_task(&self) -> Option<usize> {
        self.table.selected()
    }

    pub(crate) fn select_task(&mut self, selected: Option<usize>) {
        self.table.select(selected);
    }

    pub(crate) fn task_offset(&self) -> usize {
        self.table.offset()
    }

    pub(crate) fn set_task_offset(&mut self, offset: usize) {
        *self.table.offset_mut() = offset;
    }

    pub(crate) fn table_state(&self) -> &TableState {
        &self.table
    }

    pub(crate) fn table_state_mut(&mut self) -> &mut TableState {
        &mut self.table
    }

    pub(crate) fn selected_sidebar(&self) -> Option<usize> {
        self.sidebar.selected()
    }

    pub(crate) fn select_sidebar(&mut self, selected: Option<usize>) {
        self.sidebar.select(selected);
    }

    pub(crate) fn sidebar_state(&self) -> &ListState {
        &self.sidebar
    }

    pub(crate) fn sidebar_state_mut(&mut self) -> &mut ListState {
        &mut self.sidebar
    }

    pub(crate) fn marked_task_ids(&self) -> &BTreeSet<TaskId> {
        &self.marked_task_ids
    }

    pub(crate) fn toggle_mark(&mut self, task_id: TaskId) {
        if !self.marked_task_ids.insert(task_id.clone()) {
            self.marked_task_ids.remove(&task_id);
        }
    }

    pub(crate) fn all_marked<'a>(&self, task_ids: impl IntoIterator<Item = &'a TaskId>) -> bool {
        task_ids
            .into_iter()
            .all(|task_id| self.marked_task_ids.contains(task_id))
    }

    pub(crate) fn mark_all(&mut self, task_ids: impl IntoIterator<Item = TaskId>) {
        self.marked_task_ids.extend(task_ids);
    }

    pub(crate) fn clear_marks(&mut self) {
        self.marked_task_ids.clear();
    }

    pub(crate) fn retain_marks(&mut self, mut keep: impl FnMut(&TaskId) -> bool) {
        self.marked_task_ids.retain(|task_id| keep(task_id));
    }

    #[cfg(test)]
    pub(crate) fn mark(&mut self, task_id: TaskId) {
        self.marked_task_ids.insert(task_id);
    }

    pub(crate) fn push_navigation(&mut self, previous: TaskViewState, current: &TaskViewState) {
        if &previous != current {
            self.navigation_history.push(previous);
        }
    }

    pub(crate) fn pop_navigation(&mut self) -> Option<TaskViewState> {
        self.navigation_history.pop()
    }

    pub(crate) fn clear_navigation(&mut self) {
        self.navigation_history.clear();
    }

    pub(crate) fn record_changed_task(&mut self, task_id: TaskId) {
        self.last_changed_task_id = Some(task_id);
    }

    pub(crate) fn last_changed_task_id(&self) -> Option<&TaskId> {
        self.last_changed_task_id.as_ref()
    }

    pub(crate) fn clear_last_change(&mut self) {
        self.last_changed_task_id = None;
        self.last_change_return = None;
    }

    pub(crate) fn forget_last_changed_task(&mut self) {
        self.last_changed_task_id = None;
    }

    pub(crate) fn set_last_change_return(&mut self, state: LastChangeReturnState) {
        self.last_change_return = Some(state);
    }

    pub(crate) fn take_last_change_return(&mut self) -> Option<LastChangeReturnState> {
        self.last_change_return.take()
    }

    pub(crate) fn has_last_change_return(&self) -> bool {
        self.last_change_return.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_sidebar_focus_transition_reveals_sidebar() {
        let mut surface = ListSurface::new(false);
        surface.hide_sidebar();

        assert!(surface.toggle_focus());
        assert!(surface.sidebar_visible());
        assert_eq!(surface.focus(), Focus::Sidebar);
    }

    #[test]
    fn hiding_sidebar_returns_focus_to_tasks() {
        let mut surface = ListSurface::new(false);
        surface.focus_sidebar();

        assert!(!surface.toggle_sidebar());
        assert_eq!(surface.focus(), Focus::Tasks);
    }

    #[test]
    fn mark_transitions_are_set_consistent() {
        let mut surface = ListSurface::new(false);
        let task_id = TaskId::new();

        surface.toggle_mark(task_id.clone());
        assert!(surface.marked_task_ids().contains(&task_id));
        surface.toggle_mark(task_id.clone());
        assert!(!surface.marked_task_ids().contains(&task_id));
    }
}
