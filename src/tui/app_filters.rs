use anyhow::Result;

use crate::tui::app::App;
use crate::tui::overlay::{
    HeaderMenuAction, HeaderMenuItem, HeaderMenuKind, OverlayState, PickerIntent,
};
use crate::tui::store::{TaskOrder, TaskScope, TaskScopeTarget, TaskView};

pub(crate) const FILTER_LABEL_TITLE: &str = "Filter: label";
pub(crate) const FILTER_PRIORITY_TITLE: &str = "Filter: priority";
pub(crate) const SCOPE_PROJECT_TITLE: &str = "Scope: project";
pub(crate) const SWITCH_WORKSPACE_TITLE: &str = "Switch workspace";

impl App {
    pub(super) fn begin_filter_label(&mut self) {
        self.pending_shortcut.clear();
        let mut items = self.store.label_picker_items();
        for item in &mut items {
            item.selected =
                Some(&item.value) == self.store.view_state.filter_modifiers.label.as_ref();
        }
        self.open_picker_overlay(PickerIntent::FilterLabel, FILTER_LABEL_TITLE, items, false);
    }

    pub(super) fn begin_filter_priority(&mut self) {
        self.pending_shortcut.clear();
        let selected = self
            .store
            .view_state
            .filter_modifiers
            .priority
            .as_deref()
            .unwrap_or_default();
        let items = self.store.priority_picker_items(selected);
        self.open_picker_overlay(
            PickerIntent::FilterPriority,
            FILTER_PRIORITY_TITLE,
            items,
            false,
        );
    }

    pub(super) async fn begin_switch_workspace(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.store.refresh(None).await?;
        let items = self.store.workspace_picker_items();
        self.open_picker_overlay(
            PickerIntent::SwitchWorkspace,
            SWITCH_WORKSPACE_TITLE,
            items,
            false,
        );
        Ok(())
    }

    pub(super) fn begin_scope_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = match &self.store.view_state.scope {
            TaskScope::Project(project) => project.as_str(),
            TaskScope::Workspace => "",
        };
        let items = self.store.existing_project_picker_items(selected);
        self.open_picker_overlay(
            PickerIntent::ScopeProject,
            SCOPE_PROJECT_TITLE,
            items,
            false,
        );
    }

    pub(super) async fn show_view(&mut self, view: TaskView) -> Result<()> {
        let previous = self.store.view_state.clone();
        let selected = self.store.show_view(view).await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        Ok(())
    }

    pub(super) async fn show_scope(&mut self, scope: TaskScopeTarget) -> Result<()> {
        let previous = self.store.view_state.clone();
        let selected = self.store.show_scope(scope).await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        Ok(())
    }

    pub(super) async fn show_workspace_menu(&mut self, column: u16, row: u16) -> Result<()> {
        self.pending_shortcut.clear();
        self.store.refresh(None).await?;
        let items = self
            .store
            .workspace_picker_items()
            .into_iter()
            .enumerate()
            .map(|(index, item)| HeaderMenuItem {
                key: (index + 1).to_string(),
                label: item.label,
                selected: item.selected,
                action: HeaderMenuAction::Workspace(item.value),
            })
            .collect();
        self.overlay = Some(OverlayState::header_menu(
            HeaderMenuKind::Workspace,
            column,
            row,
            items,
        ));
        Ok(())
    }

    pub(super) fn show_scope_menu(&mut self, column: u16, row: u16) {
        self.pending_shortcut.clear();
        let selected_project = match &self.store.view_state.scope {
            TaskScope::Project(project) => project.as_str(),
            TaskScope::Workspace => "",
        };
        let mut items = vec![HeaderMenuItem {
            key: "w".to_string(),
            label: "workspace".to_string(),
            selected: matches!(self.store.view_state.scope, TaskScope::Workspace),
            action: HeaderMenuAction::WorkspaceScope,
        }];
        items.extend(
            self.store
                .existing_project_picker_items(selected_project)
                .into_iter()
                .enumerate()
                .map(|(index, item)| HeaderMenuItem {
                    key: (index + 1).to_string(),
                    label: item.label,
                    selected: item.selected,
                    action: HeaderMenuAction::ProjectScope(item.value),
                }),
        );
        self.overlay = Some(OverlayState::header_menu(
            HeaderMenuKind::Scope,
            column,
            row,
            items,
        ));
    }

    pub(super) fn show_view_menu(&mut self, column: u16, row: u16) {
        self.pending_shortcut.clear();
        let selected = self.store.view_state.view;
        let items = [
            ("q", "queue", TaskView::Queue),
            ("o", "open", TaskView::Open),
            ("t", "todo", TaskView::Todo),
            ("i", "inbox", TaskView::Inbox),
            ("a", "active", TaskView::Active),
            ("b", "backlog", TaskView::Backlog),
            ("d", "done", TaskView::Done),
            ("e", "epics", TaskView::Epics),
            ("u", "recurring", TaskView::Recurring),
            ("r", "recent", TaskView::RecentActions),
            ("c", "conflicts", TaskView::Conflicts),
            ("s", "search", TaskView::Search),
            ("l", "columns", TaskView::Columns),
        ]
        .into_iter()
        .map(|(key, label, view)| HeaderMenuItem {
            key: key.to_string(),
            label: label.to_string(),
            selected: selected == view,
            action: HeaderMenuAction::View(view),
        })
        .collect();
        self.overlay = Some(OverlayState::header_menu(
            HeaderMenuKind::View,
            column,
            row,
            items,
        ));
    }

    pub(super) fn show_order_menu(&mut self, column: u16, row: u16) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::order_menu(
            column,
            row,
            self.store.view_state.order,
        ));
    }

    pub(super) async fn submit_header_menu(&mut self, action: HeaderMenuAction) -> Result<()> {
        self.overlay = None;
        match action {
            HeaderMenuAction::Workspace(workspace) => {
                let selected = self.store.switch_workspace(workspace).await?;
                self.list.clear_last_change();
                self.clear_navigation_history();
                self.apply_filter_selection(selected);
                Ok(())
            }
            HeaderMenuAction::WorkspaceScope => self.show_scope(TaskScopeTarget::Workspace).await,
            HeaderMenuAction::ProjectScope(project) => {
                self.show_scope(TaskScopeTarget::Project(project)).await
            }
            HeaderMenuAction::View(view) => self.show_view(view).await,
        }
    }

    pub(super) async fn submit_order_menu(&mut self, order: TaskOrder) -> Result<()> {
        self.overlay = None;
        self.set_sort(order).await
    }

    pub(super) fn apply_filter_selection(&mut self, selected: Option<usize>) {
        self.list.select_task(selected);
        self.list.select_sidebar(self.store.sidebar_selection());
        self.prune_task_marks();
        self.list.focus_tasks();
        self.overlay = None;
    }

    pub(super) async fn clear_filters(&mut self) -> Result<()> {
        let previous = self.store.view_state.clone();
        let selected = self.store.clear_filters().await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        Ok(())
    }

    pub(super) async fn toggle_closed_filter(&mut self) -> Result<()> {
        if !self.store.view_state.view.supports_closed_filter() {
            self.set_warning("Closed visibility is available in Queue, Open, and Epics views");
            return Ok(());
        }
        let previous = self.store.view_state.clone();
        let selected = self.store.toggle_closed_filter().await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        Ok(())
    }

    pub(super) async fn toggle_deleted_filter(&mut self) -> Result<()> {
        let previous = self.store.view_state.clone();
        let selected = self.store.toggle_deleted_filter().await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        Ok(())
    }

    fn filter_value_or_reopen(
        &mut self,
        values: Vec<String>,
        empty_message: &str,
        reopen: fn(&mut Self),
    ) -> Option<String> {
        let Some(value) = self.require_picker_value(values, empty_message) else {
            reopen(self);
            return None;
        };
        Some(value)
    }

    pub(super) async fn submit_filter_label(&mut self, values: Vec<String>) -> Result<()> {
        let Some(label) =
            self.filter_value_or_reopen(values, "no matching label", Self::begin_filter_label)
        else {
            return Ok(());
        };
        let previous = self.store.view_state.clone();
        let selected = self.store.filter_label(label).await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        Ok(())
    }

    pub(super) async fn submit_filter_priority(&mut self, values: Vec<String>) -> Result<()> {
        let Some(priority) = self.filter_value_or_reopen(
            values,
            "no matching priority",
            Self::begin_filter_priority,
        ) else {
            return Ok(());
        };
        let previous = self.store.view_state.clone();
        let selected = self.store.filter_priority(priority).await?;
        self.push_navigation_state(previous);
        self.apply_filter_selection(selected);
        Ok(())
    }

    pub(super) async fn submit_scope_project(&mut self, values: Vec<String>) -> Result<()> {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_scope_project();
            return Ok(());
        };
        self.show_scope(TaskScopeTarget::Project(project)).await
    }

    pub(super) async fn submit_switch_workspace(&mut self, values: Vec<String>) -> Result<()> {
        let Some(workspace) = self.require_picker_value(values, "no matching workspace") else {
            self.begin_switch_workspace().await?;
            return Ok(());
        };
        let selected = self.store.switch_workspace(workspace).await?;
        self.list.clear_last_change();
        self.clear_navigation_history();
        self.apply_filter_selection(selected);
        Ok(())
    }
}
