use anyhow::Result;

use crate::tui::app::{App, Focus, LastChangeReturnState};
use crate::tui::navigation::{next_index, next_selectable_sidebar};
use crate::tui::overlay::{OverlayRoute, OverlayState, PickerItem};
use crate::tui::store::{TaskFilterModifiers, TaskScope, TaskView, TaskViewState};

impl App {
    pub(super) fn restore_sidebar_selection(&mut self) {
        self.widgets.sidebar.select(self.store.sidebar_selection());
    }

    pub(super) fn preserve_or_restore_sidebar_selection(&mut self) {
        let selected = self.widgets.sidebar.selected().filter(|&index| {
            self.store
                .sidebar_entries
                .get(index)
                .is_some_and(|entry| entry.target.is_some())
        });
        self.widgets
            .sidebar
            .select(selected.or_else(|| self.store.sidebar_selection()));
    }

    pub(super) async fn move_selection(&mut self, delta: isize) -> Result<()> {
        match self.focus {
            Focus::Tasks => {
                let next = if self.store.view_state.view == crate::tui::store::TaskView::Columns {
                    crate::tui::columns::ColumnBoard::new(
                        &self.store.task_columns,
                        &self.store.tasks,
                    )
                    .move_vertical(self.widgets.table.selected(), delta)
                } else if self.store.view_state.view == crate::tui::store::TaskView::Epics {
                    let current = self
                        .widgets
                        .table
                        .selected()
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
                        self.widgets.table.selected(),
                        self.store.main_row_count(),
                        delta,
                        true,
                    )
                };
                self.widgets.table.select(next);
            }
            Focus::Sidebar => {
                let next = next_selectable_sidebar(
                    self.widgets.sidebar.selected(),
                    &self.store.sidebar_entries,
                    delta,
                    true,
                );
                self.widgets.sidebar.select(next);
            }
        }
        Ok(())
    }

    pub(super) async fn select_edge(&mut self, last: bool) -> Result<()> {
        match self.focus {
            Focus::Tasks => {
                if self.store.view_state.view == crate::tui::store::TaskView::Columns {
                    let next = crate::tui::columns::ColumnBoard::new(
                        &self.store.task_columns,
                        &self.store.tasks,
                    )
                    .edge(self.widgets.table.selected(), last);
                    self.widgets.table.select(next);
                } else if self.store.view_state.view == crate::tui::store::TaskView::Epics {
                    let row_count = crate::tui::ui::task_visual_row_count(&self.store);
                    let row = Some(if last { row_count.saturating_sub(1) } else { 0 })
                        .filter(|_| row_count > 0);
                    self.widgets.table.select(row.and_then(|row| {
                        crate::tui::ui::task_index_at_visual_row(&self.store, row)
                    }));
                } else {
                    let row_count = self.store.main_row_count();
                    if row_count == 0 {
                        self.widgets.table.select(None);
                    } else {
                        self.widgets
                            .table
                            .select(Some(if last { row_count - 1 } else { 0 }));
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
                self.widgets.sidebar.select(next);
            }
        }
        Ok(())
    }

    pub(super) fn toggle_focus(&mut self) {
        if !self.sidebar_visible && self.focus == Focus::Tasks {
            self.sidebar_visible = true;
            self.focus = Focus::Sidebar;
            self.preserve_or_restore_sidebar_selection();
            self.set_info("sidebar visible");
            return;
        }

        self.focus = match self.focus {
            Focus::Sidebar => Focus::Tasks,
            Focus::Tasks => Focus::Sidebar,
        };
    }

    pub(super) fn toggle_sidebar(&mut self) {
        self.sidebar_visible = !self.sidebar_visible;
        if self.sidebar_visible {
            self.focus = Focus::Sidebar;
            self.preserve_or_restore_sidebar_selection();
            self.set_info("sidebar visible");
        } else {
            self.focus = Focus::Tasks;
            self.overlay = None;
            self.preserve_or_restore_sidebar_selection();
            self.set_info("task list expanded");
        }
    }

    pub(super) fn move_left(&mut self) {
        if self.focus == Focus::Tasks
            && self.store.view_state.view == crate::tui::store::TaskView::Columns
        {
            let next =
                crate::tui::columns::ColumnBoard::new(&self.store.task_columns, &self.store.tasks)
                    .move_horizontal(self.widgets.table.selected(), -1);
            if next.is_some() {
                self.widgets.table.select(next);
                return;
            }
        }
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.preserve_or_restore_sidebar_selection();
        self.overlay = None;
    }

    pub(super) fn move_right(&mut self) {
        if self.focus == Focus::Tasks
            && self.store.view_state.view == crate::tui::store::TaskView::Columns
        {
            let next =
                crate::tui::columns::ColumnBoard::new(&self.store.task_columns, &self.store.tasks)
                    .move_horizontal(self.widgets.table.selected(), 1);
            if next.is_some() {
                self.widgets.table.select(next);
            }
            return;
        }
        self.focus = Focus::Tasks;
        if self.store.view_state.view == crate::tui::store::TaskView::Columns
            && self.widgets.table.selected().is_none()
        {
            self.widgets.table.select(
                crate::tui::columns::ColumnBoard::new(&self.store.task_columns, &self.store.tasks)
                    .first(),
            );
        }
        self.overlay = None;
    }

    pub(super) fn previous_item(&mut self) {
        if self.store.view_state.view == crate::tui::store::TaskView::Conflicts {
            self.move_to_conflict(-1);
        } else {
            self.set_info("previous item is available in conflict flows");
        }
    }

    pub(super) fn next_item(&mut self) {
        if self.store.view_state.view == crate::tui::store::TaskView::Conflicts {
            self.move_to_conflict(1);
        } else {
            self.set_info("next item is available in conflict flows");
        }
    }

    pub(super) fn select_detail_task(&mut self, delta: isize) {
        let current = self.widgets.table.selected();
        let next = next_index(current, self.store.tasks.len(), delta, true);
        self.widgets.table.select(next);
        self.focus = Focus::Tasks;
        if current != next {
            self.detail_focus = None;
            self.detail_hover = None;
            self.detail_expanded_sections.clear();
            let message = if delta > 0 {
                "selected next task"
            } else {
                "selected previous task"
            };
            self.set_info(message);
        }
    }

    pub(super) async fn activate_or_toggle_detail(&mut self) -> Result<()> {
        if self.focus == Focus::Sidebar {
            self.apply_sidebar_selection().await?;
        } else if matches!(self.overlay, Some(OverlayState::Detail { .. })) {
            self.clear_detail_session();
        } else if self.store.view_state.view == crate::tui::store::TaskView::RecentActions {
            self.set_info("recent actions are read-only");
        } else {
            self.detail_navigation_history.clear();
            self.detail_focus = None;
            self.detail_hover = None;
            self.detail_expanded_sections.clear();
            self.overlay = Some(OverlayState::Detail { scroll: 0 });
            self.detail_context_scroll = 0;
        }
        Ok(())
    }

    pub(super) async fn apply_sidebar_selection(&mut self) -> Result<()> {
        let target = self
            .widgets
            .sidebar
            .selected()
            .and_then(|index| self.store.sidebar_entries.get(index))
            .and_then(|entry| entry.target.clone());
        match target {
            Some(crate::tui::store::SidebarEntryTarget::View(view)) => self.show_view(view).await?,
            Some(crate::tui::store::SidebarEntryTarget::Scope(scope)) => {
                self.show_scope(scope).await?
            }
            None => {}
        }
        self.focus = Focus::Tasks;
        self.overlay = None;
        self.restore_sidebar_selection();
        self.prune_task_marks();
        self.widgets
            .table
            .select(Some(0).filter(|_| self.store.main_row_count() > 0));
        Ok(())
    }

    pub(super) fn restore_detail_overlay(&mut self, return_to_detail: bool) {
        self.restore_detail_overlay_at_scroll(return_to_detail, self.detail_context_scroll);
    }

    pub(super) fn restore_detail_overlay_at_scroll(&mut self, return_to_detail: bool, scroll: u16) {
        if return_to_detail
            && self
                .store
                .selected_task(self.widgets.table.selected())
                .is_some()
        {
            self.detail_context = false;
            self.detail_context_scroll = scroll;
            self.overlay = Some(OverlayState::Detail { scroll });
        }
    }

    pub(super) fn cancel_overlay(&mut self) {
        self.pending_shortcut.clear();
        self.authoring.clear();
        self.conflict_flow.clear();
        self.pending_rename_project = None;
        self.pending_delete_project = None;
        self.pending_delete_attachment = None;
        self.clear_live_search_preview();
        self.detail_navigation_history.clear();
        self.detail_focus = None;
        self.detail_hover = None;
        self.detail_expanded_sections.clear();
        self.removed_epic_child = None;
        self.epic_child_authoring = None;
        let had_overlay = self.overlay.take().is_some();
        self.detail_context = false;
        if !had_overlay && self.focus == Focus::Sidebar {
            self.focus = Focus::Tasks;
            self.preserve_or_restore_sidebar_selection();
        }
    }

    pub(super) async fn open_task_by_id_with_return(
        &mut self,
        task_id: crate::ids::TaskId,
    ) -> Result<bool> {
        let selected_index = self.widgets.table.selected();
        let return_state = LastChangeReturnState {
            view_state: self.store.view_state.clone(),
            selected_task_id: self
                .store
                .selected_task(selected_index)
                .map(|item| item.task.id.clone()),
            selected_index,
            table_offset: self.widgets.table.offset(),
            return_to_detail: matches!(self.overlay, Some(OverlayState::Detail { .. }))
                || self.detail_context,
            detail_scroll: match self.overlay {
                Some(OverlayState::Detail { scroll }) => scroll,
                _ => self.detail_context_scroll,
            },
            detail_focus: self.detail_focus.clone(),
            detail_expanded_sections: self.detail_expanded_sections.clone(),
        };
        self.store.view_state = TaskViewState {
            scope: TaskScope::Workspace,
            view: TaskView::Search,
            filter_modifiers: TaskFilterModifiers {
                task_ids: vec![task_id.clone()],
                ..TaskFilterModifiers::default()
            },
            ..TaskViewState::default()
        };
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
            self.widgets.table.select(
                self.store
                    .restored_task_selection_at_index(return_state.selected_index),
            );
            *self.widgets.table.offset_mut() = return_state.table_offset;
            return Ok(false);
        };
        self.last_change_return = Some(return_state);
        self.focus = Focus::Tasks;
        self.widgets.table.select(Some(selected));
        self.overlay = Some(OverlayState::Detail { scroll: 0 });
        Ok(true)
    }

    pub(super) async fn return_to_last_change(&mut self) -> Result<()> {
        let Some(task_id) = self.last_changed_task_id.clone() else {
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
            self.focus = Focus::Tasks;
            self.widgets.table.select(Some(index));
            if matches!(self.overlay, Some(OverlayState::Detail { .. })) {
                self.overlay = Some(OverlayState::Detail { scroll: 0 });
            }
            return Ok(());
        }

        let selected_index = self.widgets.table.selected();
        let return_state = LastChangeReturnState {
            view_state: self.store.view_state.clone(),
            selected_task_id: self
                .store
                .selected_task(selected_index)
                .map(|item| item.task.id.clone()),
            selected_index,
            table_offset: self.widgets.table.offset(),
            return_to_detail: matches!(self.overlay, Some(OverlayState::Detail { .. }))
                || self.detail_context,
            detail_scroll: match self.overlay {
                Some(OverlayState::Detail { scroll }) => scroll,
                _ => self.detail_context_scroll,
            },
            detail_focus: self.detail_focus.clone(),
            detail_expanded_sections: self.detail_expanded_sections.clone(),
        };
        self.store.view_state = TaskViewState {
            scope: TaskScope::Workspace,
            view: TaskView::Search,
            filter_modifiers: TaskFilterModifiers {
                task_ids: vec![task_id.clone()],
                ..TaskFilterModifiers::default()
            },
            ..TaskViewState::default()
        };
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
            self.widgets.table.select(selected);
            *self.widgets.table.offset_mut() = return_state.table_offset;
            self.last_changed_task_id = None;
            self.set_warning("recently changed task is unavailable");
            return Ok(());
        };
        self.last_change_return = Some(return_state);
        self.focus = Focus::Tasks;
        self.widgets.table.select(Some(selected));
        self.overlay = Some(OverlayState::Detail { scroll: 0 });
        Ok(())
    }

    pub(super) async fn restore_last_change_return(&mut self) -> Result<bool> {
        let Some(return_state) = self.last_change_return.take() else {
            return Ok(false);
        };
        self.cancel_authoring_overlay();
        self.overlay = None;
        self.detail_context = false;
        self.store.view_state = return_state.view_state;
        let selected = self
            .store
            .refresh(return_state.selected_task_id.as_ref())
            .await?
            .or_else(|| {
                self.store
                    .restored_task_selection_at_index(return_state.selected_index)
            });
        self.widgets.table.select(selected);
        *self.widgets.table.offset_mut() = return_state.table_offset;
        self.detail_context_scroll = return_state.detail_scroll;
        self.detail_focus = return_state.detail_focus;
        self.detail_hover = None;
        self.detail_expanded_sections = return_state.detail_expanded_sections;
        if return_state.return_to_detail {
            self.overlay = Some(OverlayState::Detail {
                scroll: return_state.detail_scroll,
            });
        }
        self.preserve_or_restore_sidebar_selection();
        self.prune_task_marks();
        Ok(true)
    }

    pub(super) fn apply_mutation_result(&mut self, result: crate::tui::store::MutationMessage) {
        self.widgets.table.select(result.selected);
        self.preserve_or_restore_sidebar_selection();
        self.prune_task_marks();
        self.set_success(result.message);
    }

    pub(super) fn open_picker_overlay(
        &mut self,
        route: OverlayRoute,
        title: impl Into<String>,
        items: Vec<PickerItem>,
        multi: bool,
    ) {
        self.overlay = Some(OverlayState::picker(route, title, items, multi));
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
            self.widgets.table.select(None);
        } else if self
            .widgets
            .table
            .selected()
            .is_none_or(|index| index >= row_count)
        {
            self.widgets.table.select(Some(0));
        }
    }
}
