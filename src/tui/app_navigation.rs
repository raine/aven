use anyhow::Result;

use crate::tui::app::{App, Focus};
use crate::tui::navigation::{next_index, next_selectable_sidebar};
use crate::tui::overlay::{OverlayRoute, OverlayState, PickerItem};

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
                let next = next_index(
                    self.widgets.table.selected(),
                    self.store.main_row_count(),
                    delta,
                    true,
                );
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
                let row_count = self.store.main_row_count();
                if row_count == 0 {
                    self.widgets.table.select(None);
                } else {
                    self.widgets
                        .table
                        .select(Some(if last { row_count - 1 } else { 0 }));
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
        self.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.preserve_or_restore_sidebar_selection();
        self.overlay = None;
    }

    pub(super) fn move_right(&mut self) {
        self.focus = Focus::Tasks;
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
            self.overlay = None;
        } else if self.store.view_state.view == crate::tui::store::TaskView::RecentActions {
            self.set_info("recent actions are read-only");
        } else {
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
        self.clear_live_search_preview();
        let had_overlay = self.overlay.take().is_some();
        self.detail_context = false;
        if !had_overlay && self.focus == Focus::Sidebar {
            self.focus = Focus::Tasks;
            self.preserve_or_restore_sidebar_selection();
        }
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
