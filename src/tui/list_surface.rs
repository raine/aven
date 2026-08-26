use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use ratatui::widgets::{ListState, TableState};

use crate::ids::TaskId;
use crate::tui::bounded_history::BoundedHistory;
use crate::tui::detail_session::DetailSnapshot;
use crate::tui::store::{MainRowAnchor, MainRowIdentity, TaskViewState};

const NAVIGATION_HISTORY_LIMIT: usize = 32;
const TASK_ROW_DOUBLE_CLICK: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, PartialEq, Eq)]
struct TaskRowClick {
    task_id: TaskId,
    viewport_row: u16,
    at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ColumnDrag {
    pub(crate) task_id: TaskId,
    pub(crate) origin_lane: usize,
    pub(crate) hovered_lane: Option<usize>,
    pub(crate) pointer: (u16, u16),
    active: bool,
}

impl ColumnDrag {
    fn new(task_id: TaskId, origin_lane: usize, pointer: (u16, u16)) -> Self {
        Self {
            task_id,
            origin_lane,
            hovered_lane: None,
            pointer,
            active: false,
        }
    }

    pub(crate) fn is_active(&self) -> bool {
        self.active
    }

    fn update(&mut self, hovered_lane: Option<usize>, pointer: (u16, u16)) {
        self.active = true;
        self.hovered_lane = hovered_lane;
        self.pointer = pointer;
    }
}

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
    pub(crate) detail: Option<DetailSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecentActionReturnState {
    pub(crate) view_state: TaskViewState,
    pub(crate) change_id: String,
    pub(crate) selected_index: usize,
    pub(crate) table_offset: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NavigationState {
    pub(crate) view_state: TaskViewState,
    pub(crate) anchor: Option<MainRowAnchor>,
    pub(crate) selected_index: Option<usize>,
    pub(crate) table_offset: usize,
}

pub(crate) struct ListSurface {
    sidebar: ListState,
    table: TableState,
    marked_task_ids: BTreeSet<TaskId>,
    focus: Focus,
    sidebar_visible: bool,
    navigation_history: BoundedHistory<NavigationState>,
    forward_navigation_history: BoundedHistory<NavigationState>,
    last_changed_task_id: Option<TaskId>,
    last_change_return: Option<LastChangeReturnState>,
    recent_action_return: Option<RecentActionReturnState>,
    last_task_click: Option<TaskRowClick>,
    column_drag: Option<ColumnDrag>,
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
            forward_navigation_history: BoundedHistory::new(NAVIGATION_HISTORY_LIMIT),
            last_changed_task_id: None,
            last_change_return: None,
            recent_action_return: None,
            last_task_click: None,
            column_drag: None,
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

    pub(crate) fn unmark(&mut self, task_id: &TaskId) {
        self.marked_task_ids.remove(task_id);
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

    pub(crate) fn capture_navigation(
        &self,
        view_state: TaskViewState,
        anchor: Option<MainRowAnchor>,
    ) -> NavigationState {
        NavigationState {
            view_state,
            anchor,
            selected_index: self.table.selected(),
            table_offset: self.table.offset(),
        }
    }

    pub(crate) fn push_navigation(&mut self, previous: NavigationState, current: &TaskViewState) {
        if &previous.view_state != current {
            self.navigation_history.push(previous);
            self.forward_navigation_history.clear();
        }
    }

    pub(crate) fn pop_navigation(&mut self, current: NavigationState) -> Option<NavigationState> {
        let previous = self.navigation_history.pop()?;
        self.forward_navigation_history.push(current);
        Some(previous)
    }

    pub(crate) fn pop_forward_navigation(
        &mut self,
        current: NavigationState,
    ) -> Option<NavigationState> {
        let next = self.forward_navigation_history.pop()?;
        self.navigation_history.push(current);
        Some(next)
    }

    pub(crate) fn filter_history_identities(&self, target: &TaskViewState) -> Vec<MainRowIdentity> {
        self.navigation_history
            .iter_rev()
            .filter(|state| {
                state.view_state.query == target.query
                    && state.view_state.layout == target.layout
                    && state.view_state.scope == target.scope
                    && state.view_state.order == target.order
                    && state.view_state.direction == target.direction
            })
            .filter_map(|state| state.anchor.as_ref().map(|anchor| anchor.identity.clone()))
            .collect()
    }

    pub(crate) fn clear_navigation(&mut self) {
        self.navigation_history.clear();
        self.forward_navigation_history.clear();
    }

    #[cfg(test)]
    pub(crate) fn navigation_is_empty(&self) -> bool {
        self.navigation_history.is_empty()
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

    pub(crate) fn set_recent_action_return(&mut self, state: RecentActionReturnState) {
        self.recent_action_return = Some(state);
    }

    pub(crate) fn take_recent_action_return(&mut self) -> Option<RecentActionReturnState> {
        self.recent_action_return.take()
    }

    pub(crate) fn register_task_click(
        &mut self,
        task_id: TaskId,
        viewport_row: u16,
        at: Instant,
    ) -> bool {
        let is_double_click = self.last_task_click.as_ref().is_some_and(|previous| {
            previous.task_id == task_id
                && previous.viewport_row == viewport_row
                && at.duration_since(previous.at) <= TASK_ROW_DOUBLE_CLICK
        });
        self.last_task_click = (!is_double_click).then_some(TaskRowClick {
            task_id,
            viewport_row,
            at,
        });
        is_double_click
    }

    pub(crate) fn clear_task_click(&mut self) {
        self.last_task_click = None;
    }

    pub(crate) fn begin_column_drag(
        &mut self,
        task_id: TaskId,
        origin_lane: usize,
        pointer: (u16, u16),
    ) {
        self.column_drag = Some(ColumnDrag::new(task_id, origin_lane, pointer));
    }

    pub(crate) fn update_column_drag(
        &mut self,
        hovered_lane: Option<usize>,
        pointer: (u16, u16),
    ) -> bool {
        let Some(drag) = self.column_drag.as_mut() else {
            return false;
        };
        drag.update(hovered_lane, pointer);
        true
    }

    pub(crate) fn column_drag(&self) -> Option<&ColumnDrag> {
        self.column_drag.as_ref()
    }

    pub(crate) fn take_column_drag(&mut self) -> Option<ColumnDrag> {
        self.column_drag.take()
    }

    pub(crate) fn cancel_column_drag(&mut self) {
        self.column_drag = None;
    }

    pub(crate) fn expire_task_click(&mut self, at: Instant) {
        if self
            .last_task_click
            .as_ref()
            .is_some_and(|click| at.duration_since(click.at) > TASK_ROW_DOUBLE_CLICK)
        {
            self.last_task_click = None;
        }
    }

    #[cfg(test)]
    pub(crate) fn has_task_click(&self) -> bool {
        self.last_task_click.is_some()
    }

    pub(crate) fn has_last_change_return(&self) -> bool {
        self.last_change_return.is_some()
    }

    pub(crate) fn has_recent_action_return(&self) -> bool {
        self.recent_action_return.is_some()
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
    fn task_double_click_requires_same_task_and_viewport_row() {
        let mut surface = ListSurface::new(true);
        let task_id = TaskId::new();
        let other_id = TaskId::new();
        let start = Instant::now();

        assert!(!surface.register_task_click(task_id.clone(), 2, start));
        assert!(!surface.register_task_click(other_id, 2, start + Duration::from_millis(10)));
        assert!(!surface.register_task_click(
            task_id.clone(),
            3,
            start + Duration::from_millis(20),
        ));
        assert!(surface.register_task_click(task_id, 3, start + Duration::from_millis(30),));
        assert!(!surface.has_task_click());
    }

    #[test]
    fn expired_task_click_starts_a_new_click_sequence() {
        let mut surface = ListSurface::new(true);
        let task_id = TaskId::new();
        let start = Instant::now();

        assert!(!surface.register_task_click(task_id.clone(), 2, start));
        surface.expire_task_click(start + TASK_ROW_DOUBLE_CLICK + Duration::from_millis(1));
        assert!(!surface.register_task_click(
            task_id,
            2,
            start + TASK_ROW_DOUBLE_CLICK + Duration::from_millis(2),
        ));
        assert!(surface.has_task_click());
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

    #[test]
    fn column_drag_activates_on_motion_and_can_be_canceled() {
        let mut surface = ListSurface::new(true);
        let task_id = TaskId::new();

        surface.begin_column_drag(task_id.clone(), 1, (4, 5));
        assert!(!surface.column_drag().unwrap().is_active());
        assert!(surface.update_column_drag(Some(2), (20, 9)));
        assert_eq!(surface.column_drag().unwrap().hovered_lane, Some(2));
        assert_eq!(surface.column_drag().unwrap().pointer, (20, 9));
        assert!(surface.column_drag().unwrap().is_active());

        surface.cancel_column_drag();
        assert!(surface.column_drag().is_none());
    }
}
