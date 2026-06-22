use anyhow::Result;

use crate::tui::app::{App, Focus};
use crate::tui::event::ViewTarget;
use crate::tui::overlay::OverlayRoute;
use crate::tui::store::SidebarTarget;

pub(crate) const FILTER_PROJECT_TITLE: &str = "Filter: project";
pub(crate) const FILTER_LABEL_TITLE: &str = "Filter: label";
pub(crate) const FILTER_STATUS_TITLE: &str = "Filter: status";
pub(crate) const FILTER_PRIORITY_TITLE: &str = "Filter: priority";
pub(crate) const VIEW_PROJECT_TITLE: &str = "Go: project";
pub(crate) const SWITCH_WORKSPACE_TITLE: &str = "Switch workspace";

impl App {
    pub(super) fn begin_filter_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = self.store.filters.project.as_deref().unwrap_or_default();
        let items = self.store.existing_project_picker_items(selected);
        self.open_picker_overlay(
            OverlayRoute::FilterProject,
            FILTER_PROJECT_TITLE,
            items,
            false,
        );
    }

    pub(super) fn begin_filter_label(&mut self) {
        self.pending_shortcut.clear();
        let mut items = self.store.label_picker_items();
        for item in &mut items {
            item.selected = Some(&item.value) == self.store.filters.label.as_ref();
        }
        self.open_picker_overlay(OverlayRoute::FilterLabel, FILTER_LABEL_TITLE, items, false);
    }

    pub(super) fn begin_filter_status(&mut self) {
        self.pending_shortcut.clear();
        let items = self
            .store
            .status_picker_items(self.store.filters.status.as_deref());
        self.open_picker_overlay(
            OverlayRoute::FilterStatus,
            FILTER_STATUS_TITLE,
            items,
            false,
        );
    }

    pub(super) fn begin_filter_priority(&mut self) {
        self.pending_shortcut.clear();
        let selected = self.store.filters.priority.as_deref().unwrap_or_default();
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

    fn begin_view_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = match &self.store.active_view {
            SidebarTarget::Project(project) => project.as_str(),
            _ => "",
        };
        let items = self.store.existing_project_picker_items(selected);
        self.open_picker_overlay(OverlayRoute::ViewProject, VIEW_PROJECT_TITLE, items, false);
    }

    pub(super) async fn show_view(&mut self, target: ViewTarget) -> Result<()> {
        let sidebar_target = match target {
            ViewTarget::All => SidebarTarget::All,
            ViewTarget::Inbox => SidebarTarget::Inbox,
            ViewTarget::Active => SidebarTarget::Active,
            ViewTarget::Backlog => SidebarTarget::Backlog,
            ViewTarget::Todo => SidebarTarget::Todo,
            ViewTarget::Done => SidebarTarget::Done,
            ViewTarget::Conflicts => SidebarTarget::Conflicts,
            ViewTarget::Project => {
                self.begin_view_project();
                return Ok(());
            }
        };
        let selected = self.store.show_view(sidebar_target).await?;
        self.apply_filter_selection(selected);
        self.set_info("view updated");
        Ok(())
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
        let message = if self.store.filters.include_deleted {
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

    pub(super) async fn submit_filter_project(&mut self, values: Vec<String>) -> Result<()> {
        let Some(project) =
            self.filter_value_or_reopen(values, "no matching project", Self::begin_filter_project)
        else {
            return Ok(());
        };
        let selected = self.store.filter_project(project).await?;
        self.apply_filter_selection(selected);
        self.set_success("project filter applied");
        Ok(())
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

    pub(super) async fn submit_filter_status(&mut self, values: Vec<String>) -> Result<()> {
        let Some(status) =
            self.filter_value_or_reopen(values, "no matching status", Self::begin_filter_status)
        else {
            return Ok(());
        };
        let selected = self.store.filter_status(status).await?;
        self.apply_filter_selection(selected);
        self.set_success("status filter applied");
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

    pub(super) async fn submit_view_project(&mut self, values: Vec<String>) -> Result<()> {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_view_project();
            return Ok(());
        };
        let selected = self
            .store
            .show_view(SidebarTarget::Project(project))
            .await?;
        self.apply_filter_selection(selected);
        self.set_info("project view selected");
        Ok(())
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
