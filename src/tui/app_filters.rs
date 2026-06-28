use anyhow::Result;

use crate::choices::{PRIORITIES, STATUSES};
use crate::tui::app::{App, Focus};
use crate::tui::overlay::{
    HeaderMenuAction, HeaderMenuItem, HeaderMenuKind, HeaderMenuState, OrderMenuState,
    OverlayRoute, OverlayState,
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
        self.open_picker_overlay(OverlayRoute::FilterLabel, FILTER_LABEL_TITLE, items, false);
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
            OverlayRoute::FilterPriority,
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
            OverlayRoute::SwitchWorkspace,
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
            OverlayRoute::ScopeProject,
            SCOPE_PROJECT_TITLE,
            items,
            false,
        );
    }

    pub(super) async fn show_view(&mut self, view: TaskView) -> Result<()> {
        let selected = self.store.show_view(view).await?;
        self.apply_filter_selection(selected);
        self.set_info("view updated");
        Ok(())
    }

    pub(super) async fn show_scope(&mut self, scope: TaskScopeTarget) -> Result<()> {
        let selected = self.store.show_scope(scope).await?;
        self.apply_filter_selection(selected);
        self.set_info("scope updated");
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
            ("c", "conflicts", TaskView::Conflicts),
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

    pub(super) fn show_status_menu(&mut self, column: u16, row: u16) {
        self.pending_shortcut.clear();
        let selected = self
            .store
            .selected_task(self.widgets.table.selected())
            .map(|item| item.task.status.as_str())
            .unwrap_or_default();
        let items = STATUSES
            .iter()
            .map(|status| HeaderMenuItem {
                key: status_menu_key(status).to_string(),
                label: (*status).to_string(),
                selected: *status == selected,
                action: HeaderMenuAction::Status((*status).to_string()),
            })
            .collect();
        self.overlay = Some(OverlayState::header_menu(
            HeaderMenuKind::Status,
            column,
            row,
            items,
        ));
    }

    pub(super) fn show_priority_menu(&mut self, column: u16, row: u16) {
        self.pending_shortcut.clear();
        let selected = self
            .store
            .selected_task(self.widgets.table.selected())
            .map(|item| item.task.priority.as_str())
            .unwrap_or_default();
        let items = PRIORITIES
            .iter()
            .map(|priority| HeaderMenuItem {
                key: priority_menu_key(priority).to_string(),
                label: (*priority).to_string(),
                selected: *priority == selected,
                action: HeaderMenuAction::Priority((*priority).to_string()),
            })
            .collect();
        self.overlay = Some(OverlayState::header_menu(
            HeaderMenuKind::Priority,
            column,
            row,
            items,
        ));
    }

    pub(super) async fn submit_header_menu(&mut self, action: HeaderMenuAction) -> Result<()> {
        self.overlay = None;
        match action {
            HeaderMenuAction::Workspace(workspace) => {
                let (message, selected) = self.store.switch_workspace(workspace).await?;
                self.apply_filter_selection(selected);
                self.set_success(message);
                Ok(())
            }
            HeaderMenuAction::WorkspaceScope => self.show_scope(TaskScopeTarget::Workspace).await,
            HeaderMenuAction::ProjectScope(project) => {
                self.show_scope(TaskScopeTarget::Project(project)).await
            }
            HeaderMenuAction::View(view) => self.show_view(view).await,
            HeaderMenuAction::Status(status) => self.submit_edit_status(status).await,
            HeaderMenuAction::Priority(priority) => self.submit_edit_priority(priority).await,
        }
    }

    pub(super) async fn submit_header_menu_at(
        &mut self,
        state: HeaderMenuState,
        column: u16,
        row: u16,
        terminal_size: ratatui::layout::Size,
    ) -> Result<()> {
        let Some(action) = header_menu_action_at(&state, column, row, terminal_size) else {
            self.overlay = None;
            return Ok(());
        };
        self.submit_header_menu(action).await
    }

    pub(super) async fn submit_order_menu(&mut self, order: TaskOrder) -> Result<()> {
        self.overlay = None;
        self.set_sort(order).await
    }

    pub(super) async fn submit_order_menu_at(
        &mut self,
        state: OrderMenuState,
        column: u16,
        row: u16,
        terminal_size: ratatui::layout::Size,
    ) -> Result<()> {
        let Some(order) = order_menu_order_at(state, column, row, terminal_size) else {
            self.overlay = None;
            return Ok(());
        };
        self.submit_order_menu(order).await
    }

    pub(super) fn apply_filter_selection(&mut self, selected: Option<usize>) {
        self.widgets.table.select(selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.focus = Focus::Tasks;
        self.overlay = None;
    }

    pub(super) async fn clear_filters(&mut self) -> Result<()> {
        let selected = self.store.clear_filters().await?;
        self.apply_filter_selection(selected);
        self.set_success("filters cleared");
        Ok(())
    }

    pub(super) async fn toggle_deleted_filter(&mut self) -> Result<()> {
        let selected = self.store.toggle_deleted_filter().await?;
        self.apply_filter_selection(selected);
        let modifiers = &self.store.view_state.filter_modifiers;
        let message = if modifiers.deleted_only {
            "showing deleted tasks only"
        } else if modifiers.include_deleted {
            "showing deleted tasks"
        } else {
            "hiding deleted tasks"
        };
        self.set_info(message);
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
        let selected = self.store.filter_label(label).await?;
        self.apply_filter_selection(selected);
        self.set_success("label filter applied");
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
        let selected = self.store.filter_priority(priority).await?;
        self.apply_filter_selection(selected);
        self.set_success("priority filter applied");
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
        let (message, selected) = self.store.switch_workspace(workspace).await?;
        self.apply_filter_selection(selected);
        self.set_success(message);
        Ok(())
    }
}

fn status_menu_key(status: &str) -> &'static str {
    match status {
        "inbox" => "i",
        "backlog" => "b",
        "todo" => "t",
        "active" => "a",
        "done" => "d",
        "canceled" => "c",
        _ => "?",
    }
}

fn priority_menu_key(priority: &str) -> &'static str {
    match priority {
        "none" => "n",
        "low" => "l",
        "medium" => "m",
        "high" => "h",
        "urgent" => "u",
        _ => "?",
    }
}

fn header_menu_action_at(
    state: &HeaderMenuState,
    column: u16,
    row: u16,
    terminal_size: ratatui::layout::Size,
) -> Option<HeaderMenuAction> {
    let area = state.area(terminal_size.width, terminal_size.height);
    if column < area.x
        || column >= area.x.saturating_add(area.width)
        || row < area.y
        || row >= area.y.saturating_add(area.height)
    {
        return None;
    }
    let item_index = row.saturating_sub(area.y).checked_sub(1)? as usize;
    state.items.get(item_index).map(|item| item.action.clone())
}

fn order_menu_order_at(
    state: OrderMenuState,
    column: u16,
    row: u16,
    terminal_size: ratatui::layout::Size,
) -> Option<TaskOrder> {
    let area = state.area(terminal_size.width, terminal_size.height);
    if column < area.x
        || column >= area.x.saturating_add(area.width)
        || row < area.y
        || row >= area.y.saturating_add(area.height)
    {
        return None;
    }
    match row.saturating_sub(area.y) {
        1 => Some(TaskOrder::Created),
        2 => Some(TaskOrder::Updated),
        3 => Some(TaskOrder::Priority),
        4 => Some(TaskOrder::Project),
        5 => Some(TaskOrder::Title),
        _ => None,
    }
}
