use anyhow::Result;

use crate::query::TaskFilters;

use super::{SidebarTarget, TuiStore};

impl TuiStore {
    pub(crate) fn sidebar_selection(&self) -> Option<usize> {
        self.sidebar_entries
            .iter()
            .position(|entry| entry.target.as_ref() == Some(&self.active_view))
            .or(Some(1))
    }

    pub(crate) async fn apply_sidebar_selection(&mut self, selected: Option<usize>) -> Result<()> {
        let Some(target) = selected
            .and_then(|index| self.sidebar_entries.get(index))
            .and_then(|entry| entry.target.clone())
        else {
            return Ok(());
        };
        self.show_view(target).await?;
        Ok(())
    }

    pub(crate) async fn show_view(&mut self, target: SidebarTarget) -> Result<Option<usize>> {
        self.active_view = target;
        self.apply_active_view_filters();
        self.refresh(None).await
    }

    pub(crate) async fn clear_filters(&mut self) -> Result<Option<usize>> {
        self.active_view = SidebarTarget::All;
        self.filters = TaskFilters {
            hide_done: true,
            ..TaskFilters::default()
        };
        self.refresh(None).await
    }

    async fn apply_attribute_filter(
        &mut self,
        setter: impl FnOnce(&mut TaskFilters),
    ) -> Result<Option<usize>> {
        self.active_view = SidebarTarget::All;
        self.filters.include_deleted = false;
        self.filters.conflicts_only = false;
        setter(&mut self.filters);
        self.refresh(None).await
    }

    pub(crate) async fn filter_project(&mut self, project: String) -> Result<Option<usize>> {
        self.apply_attribute_filter(|filters| filters.project = Some(project))
            .await
    }

    pub(crate) async fn filter_label(&mut self, label: String) -> Result<Option<usize>> {
        self.apply_attribute_filter(|filters| filters.label = Some(label))
            .await
    }

    pub(crate) async fn filter_status(&mut self, status: String) -> Result<Option<usize>> {
        self.apply_attribute_filter(|filters| filters.status = Some(status))
            .await
    }

    pub(crate) async fn filter_priority(&mut self, priority: String) -> Result<Option<usize>> {
        self.apply_attribute_filter(|filters| filters.priority = Some(priority))
            .await
    }

    pub(crate) async fn toggle_deleted_filter(&mut self) -> Result<Option<usize>> {
        self.active_view = SidebarTarget::All;
        self.filters.include_deleted = !self.filters.include_deleted;
        self.filters.conflicts_only = false;
        self.refresh(None).await
    }

    pub(super) fn apply_active_view_filters(&mut self) {
        let search = self.filters.search.clone();
        self.filters = TaskFilters {
            search,
            ..TaskFilters::default()
        };
        match &self.active_view {
            SidebarTarget::All => self.filters.hide_done = true,
            SidebarTarget::Inbox => self.filters.status = Some("inbox".to_string()),
            SidebarTarget::Active => self.filters.status = Some("active".to_string()),
            SidebarTarget::Backlog => self.filters.status = Some("backlog".to_string()),
            SidebarTarget::Todo => self.filters.status = Some("todo".to_string()),
            SidebarTarget::Done => self.filters.status = Some("done".to_string()),
            SidebarTarget::Conflicts => self.filters.conflicts_only = true,
            SidebarTarget::Project(project) => self.filters.project = Some(project.clone()),
        }
    }

    pub(crate) async fn accept_search(&mut self, input: &str) -> Result<Option<usize>> {
        self.filters.search = if input.trim().is_empty() {
            None
        } else {
            Some(input.trim().to_string())
        };
        self.refresh(None).await
    }

    pub(super) fn restored_task_selection(&self, selected_id: Option<&str>) -> Option<usize> {
        if self.tasks.is_empty() {
            return None;
        }
        selected_id
            .and_then(|id| self.tasks.iter().position(|item| item.task.id == id))
            .or(Some(0))
    }
}
