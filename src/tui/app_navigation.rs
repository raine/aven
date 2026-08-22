use anyhow::Result;

use crate::tui::app::{App, Focus, LastChangeReturnState, RecentActionReturnState};
use crate::tui::navigation::{next_index, next_selectable_sidebar};
use crate::tui::overlay::{OverlayState, PickerIntent, PickerItem};
use crate::tui::store::{MainRowSelection, TaskQuery, TaskViewState};

impl App {
    pub(super) fn restore_sidebar_selection(&mut self) {
        self.list.select_sidebar(self.store.sidebar_selection());
    }

    pub(super) fn preserve_or_restore_sidebar_selection(&mut self) {
        let selected = self.list.selected_sidebar().filter(|&index| {
            self.store
                .sidebar_entries
                .get(index)
                .is_some_and(|entry| entry.target.is_some())
        });
        self.list
            .select_sidebar(selected.or_else(|| self.store.sidebar_selection()));
    }

    pub(super) async fn move_selection(&mut self, delta: isize) -> Result<()> {
        match self.list.focus() {
            Focus::Tasks => {
                let next = if self.store.view_state.is_columns() {
                    crate::tui::columns::ColumnBoard::new(
                        &self.store.task_columns,
                        &self.store.tasks,
                    )
                    .move_vertical(self.list.selected_task(), delta)
                } else if self.store.view_state.query == crate::tui::store::TaskQuery::Epics {
                    let current = self
                        .list
                        .selected_task()
                        .and_then(|index| crate::tui::ui::task_visual_row(&self.store, index));
                    next_index(
                        current,
                        crate::tui::ui::task_visual_row_count(&self.store),
                        delta,
                        true,
                    )
                    .and_then(|row| crate::tui::ui::task_index_at_visual_row(&self.store, row))
                } else {
                    next_index(
                        self.list.selected_task(),
                        self.store.main_row_count(),
                        delta,
                        true,
                    )
                };
                self.list.select_task(next);
            }
            Focus::Sidebar => {
                let next = next_selectable_sidebar(
                    self.list.selected_sidebar(),
                    &self.store.sidebar_entries,
                    delta,
                    true,
                );
                self.list.select_sidebar(next);
            }
        }
        Ok(())
    }

    pub(super) async fn select_edge(&mut self, last: bool) -> Result<()> {
        match self.list.focus() {
            Focus::Tasks => {
                if self.store.view_state.is_columns() {
                    let next = crate::tui::columns::ColumnBoard::new(
                        &self.store.task_columns,
                        &self.store.tasks,
                    )
                    .edge(self.list.selected_task(), last);
                    self.list.select_task(next);
                } else if self.store.view_state.query == crate::tui::store::TaskQuery::Epics {
                    let row_count = crate::tui::ui::task_visual_row_count(&self.store);
                    let row = if row_count > 0 {
                        Some(if last { row_count - 1 } else { 0 })
                    } else {
                        None
                    };
                    self.list.select_task(row.and_then(|row| {
                        crate::tui::ui::task_index_at_visual_row(&self.store, row)
                    }));
                } else {
                    let row_count = self.store.main_row_count();
                    if row_count == 0 {
                        self.list.select_task(None);
                    } else {
                        self.list
                            .select_task(Some(if last { row_count - 1 } else { 0 }));
                    }
                }
            }
            Focus::Sidebar => {
                let next = if last {
                    self.store
                        .sidebar_entries
                        .iter()
                        .rposition(|entry| entry.target.is_some())
                } else {
                    self.store
                        .sidebar_entries
                        .iter()
                        .position(|entry| entry.target.is_some())
                };
                self.list.select_sidebar(next);
            }
        }
        Ok(())
    }

    pub(super) fn toggle_focus(&mut self) {
        if self.list.toggle_focus() {
            self.preserve_or_restore_sidebar_selection();
        }
    }

    pub(super) fn toggle_sidebar(&mut self) {
        if self.list.toggle_sidebar() {
            self.preserve_or_restore_sidebar_selection();
        } else {
            self.overlay = None;
            self.preserve_or_restore_sidebar_selection();
        }
    }

    pub(super) fn move_left(&mut self) {
        if self.list.focus() == Focus::Tasks && self.store.view_state.is_columns() {
            let next =
                crate::tui::columns::ColumnBoard::new(&self.store.task_columns, &self.store.tasks)
                    .move_horizontal(self.list.selected_task(), -1);
            if next.is_some() {
                self.list.select_task(next);
                return;
            }
        }
        self.list.focus_sidebar();
        self.preserve_or_restore_sidebar_selection();
        self.overlay = None;
    }

    pub(super) async fn move_right(&mut self) -> Result<()> {
        let selected = self.list.selected_task();
        let epic_selected = self.list.focus() == Focus::Tasks
            && self.store.view_state.query == TaskQuery::Epics
            && selected
                .and_then(|index| self.store.tasks.get(index))
                .is_some_and(|item| item.task.is_epic);
        if epic_selected {
            if let Some(result) = self.store.toggle_selected_epic(selected).await? {
                self.list.select_task(result.selected);
            }
            return Ok(());
        }
        if self.list.focus() == Focus::Tasks && self.store.view_state.is_columns() {
            let next =
                crate::tui::columns::ColumnBoard::new(&self.store.task_columns, &self.store.tasks)
                    .move_horizontal(self.list.selected_task(), 1);
            if next.is_some() {
                self.list.select_task(next);
            }
            return Ok(());
        }
        self.list.focus_tasks();
        if self.store.view_state.is_columns() && self.list.selected_task().is_none() {
            self.list.select_task(
                crate::tui::columns::ColumnBoard::new(&self.store.task_columns, &self.store.tasks)
                    .first(),
            );
        }
        self.overlay = None;
        Ok(())
    }

    pub(super) fn previous_item(&mut self) {
        if self.store.view_state.query == crate::tui::store::TaskQuery::Conflicts {
            self.move_to_conflict(-1);
        } else {
            self.set_info("previous item is available in conflict flows");
        }
    }

    pub(super) fn next_item(&mut self) {
        if self.store.view_state.query == crate::tui::store::TaskQuery::Conflicts {
            self.move_to_conflict(1);
        } else {
            self.set_info("next item is available in conflict flows");
        }
    }

    pub(super) async fn select_detail_task(&mut self, delta: isize) -> Result<()> {
        let sibling_target = self
            .detail
            .state()
            .and_then(|detail| detail.sibling_target(delta));
        if let Some(target) = sibling_target {
            let Some(item) = self.store.load_task_item(&target.task_id).await? else {
                self.set_warning("sibling task is unavailable");
                return Ok(());
            };
            let selected = self
                .store
                .restore_view_state(
                    target.view_state,
                    Some(&MainRowSelection::Task(target.task_id.clone())),
                )
                .await?
                .selected;
            let index = if let Some(index) = selected.filter(|&index| {
                self.store
                    .tasks
                    .get(index)
                    .is_some_and(|candidate| candidate.task.id == target.task_id)
            }) {
                index
            } else {
                self.store.show_exact_task(item);
                0
            };
            self.list.select_task(Some(index));
            self.list.focus_tasks();
            if let Some(detail) = self.detail.state_mut() {
                detail.select_sibling_task(&target.task_id);
                detail.reset_task_state(0);
            }
            return Ok(());
        }
        if self
            .detail
            .state()
            .is_some_and(|detail| detail.has_sibling_context())
        {
            self.set_info("no other tasks in source list");
            return Ok(());
        }

        let current = self.list.selected_task();
        let next = next_index(current, self.store.tasks.len(), delta, true);
        self.list.select_task(next);
        self.list.focus_tasks();
        if current != next
            && let Some(detail) = self.detail.state_mut()
        {
            detail.reset_task_state(0);
        }
        Ok(())
    }

    pub(super) async fn activate_or_toggle_detail(&mut self) -> Result<()> {
        if self.list.focus() == Focus::Sidebar {
            self.apply_sidebar_selection().await?;
        } else if self.store.view_state.query == crate::tui::store::TaskQuery::Recurring {
            if self.detail.is_active() {
                self.open_recurrence_occurrence().await?;
            } else if let Some(series_id) = self
                .store
                .selected_recurrence_series(self.list.selected_task())
                .map(|item| item.series.id.clone())
            {
                self.store.load_recurrence_series_detail(&series_id).await?;
                self.detail = crate::tui::detail_session::DetailSession::open(0);
                self.overlay = None;
            } else {
                self.set_warning("no recurring series selected");
            }
        } else if self.detail.is_active() {
            if self.list.has_recent_action_return() {
                self.close_detail_session().await?;
            } else {
                self.clear_detail_session();
            }
        } else if self.store.view_state.query == crate::tui::store::TaskQuery::RecentActions {
            self.open_recent_action_task().await?;
        } else {
            self.detail = crate::tui::detail_session::DetailSession::open(0);
            self.show_detail(0);
        }
        Ok(())
    }

    pub(super) async fn apply_sidebar_selection(&mut self) -> Result<()> {
        let target = self
            .list
            .selected_sidebar()
            .and_then(|index| self.store.sidebar_entries.get(index))
            .and_then(|entry| entry.target.clone());
        match target {
            Some(crate::tui::store::SidebarEntryTarget::View(view)) => self.show_view(view).await?,
            Some(crate::tui::store::SidebarEntryTarget::Scope(scope)) => {
                self.show_scope(scope).await?
            }
            None => {}
        }
        self.list.focus_tasks();
        self.overlay = None;
        self.restore_sidebar_selection();
        self.prune_task_marks();
        self.list
            .select_task((self.store.main_row_count() > 0).then_some(0));
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn restore_detail_overlay_at_scroll(&mut self, return_to_detail: bool, scroll: u16) {
        if !return_to_detail {
            return;
        }
        let recurring = self.store.view_state.query == crate::tui::store::TaskQuery::Recurring;
        let detail_is_available = if recurring {
            self.store
                .selected_recurrence_series(self.list.selected_task())
                .zip(self.store.recurrence_detail.as_ref())
                .is_some_and(|(selected, detail)| selected.series.id == detail.series.id)
        } else {
            self.store
                .selected_task(self.list.selected_task())
                .is_some()
        };
        if detail_is_available {
            self.detail = crate::tui::detail_session::DetailSession::open(scroll);
            self.overlay = None;
        } else {
            self.detail.close();
            if recurring {
                self.store.recurrence_detail = None;
            }
        }
    }

    pub(super) fn cancel_overlay(&mut self) {
        self.pending_shortcut.clear();
        self.authoring.clear();
        self.clear_live_search_preview();
        let cancellation_message = self.workspace_cancellation_message();
        let had_overlay = self.overlay.take().is_some();
        if let Some(message) = cancellation_message {
            self.set_info(message);
        }
        if !had_overlay && self.detail.is_active() {
            self.detail.close();
            if self.store.view_state.query == crate::tui::store::TaskQuery::Recurring {
                self.store.recurrence_detail = None;
            }
        } else if !had_overlay && self.list.focus() == Focus::Sidebar {
            self.list.focus_tasks();
            self.preserve_or_restore_sidebar_selection();
        } else if !had_overlay && !self.list.marked_task_ids().is_empty() {
            self.clear_marks();
        }
    }

    pub(super) async fn open_recent_action_task(&mut self) -> Result<()> {
        let Some(selected_index) = self.list.selected_task() else {
            self.set_warning("no recent action selected");
            return Ok(());
        };
        let Some(action) = self
            .store
            .selected_recent_action(Some(selected_index))
            .cloned()
        else {
            self.set_warning("no recent action selected");
            return Ok(());
        };
        if action.entity_type != "task" {
            self.set_warning("recent action has no task identity");
            return Ok(());
        }
        let Ok(task_id) = action.entity_id.parse() else {
            self.set_warning("recent action task is unavailable");
            return Ok(());
        };
        let Some(item) = self.store.load_task_item(&task_id).await? else {
            self.set_warning("recent action task is unavailable");
            return Ok(());
        };

        self.list.set_recent_action_return(RecentActionReturnState {
            view_state: self.store.view_state.clone(),
            change_id: action.change_id,
            selected_index,
            table_offset: self.list.task_offset(),
        });
        self.store.show_exact_task(item);
        self.list.focus_tasks();
        self.list.select_task(Some(0));
        self.list.clear_task_click();
        self.detail = crate::tui::detail_session::DetailSession::open(0);
        self.show_detail(0);
        Ok(())
    }

    pub(super) async fn restore_recent_action_return(&mut self) -> Result<bool> {
        let Some(return_state) = self.list.take_recent_action_return() else {
            return Ok(false);
        };
        self.store.view_state = return_state.view_state;
        self.store.refresh(None).await?;
        let selected = self
            .store
            .recent_actions
            .iter()
            .position(|action| action.change_id == return_state.change_id)
            .or_else(|| {
                (!self.store.recent_actions.is_empty()).then_some(
                    return_state
                        .selected_index
                        .min(self.store.recent_actions.len() - 1),
                )
            });
        self.list.select_task(selected);
        self.list.set_task_offset(return_state.table_offset);
        self.list.focus_tasks();
        self.preserve_or_restore_sidebar_selection();
        Ok(true)
    }

    pub(super) async fn return_to_last_change(&mut self) -> Result<()> {
        let Some(task_id) = self.list.last_changed_task_id().cloned() else {
            self.set_info("no recently changed task");
            return Ok(());
        };

        if let Some(index) = self
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == task_id)
            && crate::tui::ui::task_visual_row(&self.store, index).is_some()
        {
            self.list.focus_tasks();
            self.list.select_task(Some(index));
            if let Some(detail) = self.detail.state_mut() {
                detail.reset_task_state(0);
                self.show_detail(0);
            }
            return Ok(());
        }

        let selected_index = self.list.selected_task();
        let selected_task_id = self
            .store
            .selected_task(selected_index)
            .map(|item| item.task.id.clone());
        let return_state = LastChangeReturnState {
            view_state: self.store.view_state.clone(),
            selected_task_id: selected_task_id.clone(),
            selected_index,
            table_offset: self.list.task_offset(),
            detail: self.detail.state().and_then(|detail| {
                selected_task_id
                    .clone()
                    .map(|task_id| detail.snapshot(task_id, self.store.view_state.clone()))
            }),
        };
        self.store.view_state = TaskViewState::for_exact_task(task_id.clone());
        let selected = self.store.refresh(Some(&task_id)).await?;
        let Some(selected) = selected.filter(|index| {
            self.store
                .tasks
                .get(*index)
                .is_some_and(|item| item.task.id == task_id)
        }) else {
            self.store.view_state = return_state.view_state;
            self.store
                .refresh(return_state.selected_task_id.as_ref())
                .await?;
            let selected = self
                .store
                .restored_task_selection_at_index(return_state.selected_index);
            self.list.select_task(selected);
            self.list.set_task_offset(return_state.table_offset);
            self.list.forget_last_changed_task();
            self.set_warning("recently changed task is unavailable");
            return Ok(());
        };
        self.list.set_last_change_return(return_state);
        self.list.focus_tasks();
        self.list.select_task(Some(selected));
        self.detail = crate::tui::detail_session::DetailSession::open(0);
        self.show_detail(0);
        Ok(())
    }

    pub(super) async fn restore_last_change_return(&mut self) -> Result<bool> {
        let Some(return_state) = self.list.take_last_change_return() else {
            return Ok(false);
        };
        self.cancel_authoring_overlay();
        self.overlay = None;
        self.detail.close();
        self.store.view_state = return_state.view_state;
        let selected = self
            .store
            .refresh(return_state.selected_task_id.as_ref())
            .await?
            .or_else(|| {
                self.store
                    .restored_task_selection_at_index(return_state.selected_index)
            });
        self.list.select_task(selected);
        self.list.set_task_offset(return_state.table_offset);
        if let Some(return_detail) = return_state.detail {
            let mut detail = crate::tui::detail_session::DetailSession::open(return_detail.scroll);
            if let Some(state) = detail.state_mut() {
                state.restore_snapshot(&return_detail);
            }
            self.detail = detail;
            self.show_detail(return_detail.scroll);
        }
        self.preserve_or_restore_sidebar_selection();
        self.prune_task_marks();
        Ok(true)
    }

    pub(super) fn apply_mutation_result(&mut self, result: crate::tui::store::MutationMessage) {
        self.list.select_task(result.selected);
        self.preserve_or_restore_sidebar_selection();
        self.prune_task_marks();
        self.set_mutation_success(result.message);
    }

    pub(super) fn open_picker_overlay(
        &mut self,
        intent: PickerIntent,
        title: impl Into<String>,
        items: Vec<PickerItem>,
        multi: bool,
    ) {
        self.overlay = Some(OverlayState::picker(intent, title, items, multi));
    }

    pub(super) fn require_picker_value(
        &mut self,
        values: Vec<String>,
        message: &str,
    ) -> Option<String> {
        match values.first().cloned() {
            Some(value) => Some(value),
            None => {
                self.set_warning(message);
                None
            }
        }
    }

    pub(super) fn restore_selection_after_mutation(&mut self) {
        self.preserve_or_restore_sidebar_selection();
        self.prune_task_marks();
        let row_count = self.store.main_row_count();
        if row_count == 0 {
            self.list.select_task(None);
        } else if self
            .list
            .selected_task()
            .is_none_or(|index| index >= row_count)
        {
            self.list.select_task(Some(0));
        }
    }
}
