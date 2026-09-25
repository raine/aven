use crate::config::TaskTableConfig;
use crate::query::TaskListItem;
use crate::tui::store::{TaskListViewRef, TaskViewState, TuiStore};
use crate::tui::ui::empty_state::EmptyStateContext;

/// Already-loaded values that the task table renders and hit-tests.
pub(super) struct TaskListSource<'a> {
    pub(super) tasks: &'a [TaskListItem],
    pub(super) view: TaskListViewRef<'a>,
    pub(super) view_state: &'a TaskViewState,
    pub(super) table: &'a TaskTableConfig,
    pub(super) empty_state: EmptyStateContext<'a>,
}

impl<'a> TaskListSource<'a> {
    pub(super) fn from_store(store: &'a TuiStore) -> Self {
        Self {
            tasks: &store.tasks,
            view: store.task_list_view(),
            view_state: &store.view_state,
            table: &store.config().tui.table,
            empty_state: EmptyStateContext::from_store(store),
        }
    }
}
