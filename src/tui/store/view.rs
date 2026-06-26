use anyhow::Result;

use super::{
    SidebarEntryTarget, TaskFilterModifiers, TaskOrder, TaskScope, TaskScopeTarget, TaskView,
    TuiStore,
};

impl TuiStore {
    pub(crate) fn sidebar_selection(&self) -> Option<usize> {
        if let Some(index) = self.sidebar_entries.iter().position(|entry| {
            matches!(
                (&entry.target, &self.view_state.scope),
                (
                    Some(SidebarEntryTarget::Scope(TaskScopeTarget::Project(project))),
                    TaskScope::Project(scope),
                ) if project == scope
            )
        }) {
            return Some(index);
        }
        self.sidebar_entries
            .iter()
            .position(|entry| match &entry.target {
                Some(SidebarEntryTarget::View(view)) => *view == self.view_state.view,
                _ => false,
            })
            .or(Some(1))
    }

    pub(crate) async fn apply_sidebar_selection(&mut self, selected: Option<usize>) -> Result<()> {
        let Some(target) = selected
            .and_then(|index| self.sidebar_entries.get(index))
            .and_then(|entry| entry.target.clone())
        else {
            return Ok(());
        };
        match target {
            SidebarEntryTarget::View(view) => {
                self.show_view(view).await?;
            }
            SidebarEntryTarget::Scope(scope) => {
                self.show_scope(scope).await?;
            }
        }
        Ok(())
    }

    pub(crate) async fn show_view(&mut self, view: TaskView) -> Result<Option<usize>> {
        self.view_state.view = view;
        self.refresh(None).await
    }

    pub(crate) async fn show_scope(&mut self, target: TaskScopeTarget) -> Result<Option<usize>> {
        self.view_state.scope = match target {
            TaskScopeTarget::Workspace => TaskScope::Workspace,
            TaskScopeTarget::Project(project) => TaskScope::Project(project),
        };
        self.refresh(None).await
    }

    pub(crate) async fn clear_filters(&mut self) -> Result<Option<usize>> {
        self.view_state.filter_modifiers = TaskFilterModifiers::default();
        self.refresh(None).await
    }

    pub(crate) async fn filter_label(&mut self, label: String) -> Result<Option<usize>> {
        self.view_state.filter_modifiers.label = Some(label);
        self.refresh(None).await
    }

    pub(crate) async fn filter_priority(&mut self, priority: String) -> Result<Option<usize>> {
        self.view_state.filter_modifiers.priority = Some(priority);
        self.refresh(None).await
    }

    pub(crate) async fn toggle_deleted_filter(&mut self) -> Result<Option<usize>> {
        self.view_state.filter_modifiers.include_deleted =
            !self.view_state.filter_modifiers.include_deleted;
        self.refresh(None).await
    }

    pub(crate) async fn accept_search(&mut self, input: &str) -> Result<Option<usize>> {
        self.view_state.filter_modifiers.search = if input.trim().is_empty() {
            None
        } else {
            Some(input.trim().to_string())
        };
        self.refresh(None).await
    }

    pub(crate) fn set_view_order(&mut self, order: TaskOrder) {
        if self.view_state.view == TaskView::Queue {
            self.view_state.view = TaskView::Open;
        }
        self.view_state.order = order;
    }

    pub(crate) fn reverse_view_order(&mut self) {
        if self.view_state.view == TaskView::Queue {
            self.view_state.view = TaskView::Open;
        }
        self.view_state.direction = self.view_state.direction.toggled();
    }

    pub(super) fn restored_task_selection(&self, selected_id: Option<&str>) -> Option<usize> {
        if self.tasks.is_empty() {
            return None;
        }
        selected_id
            .and_then(|id| self.tasks.iter().position(|item| item.task.id == id))
            .or(Some(0))
    }

    pub(super) fn restored_task_selection_at_index(
        &self,
        selected: Option<usize>,
    ) -> Option<usize> {
        if self.tasks.is_empty() {
            None
        } else {
            selected.map(|index| index.min(self.tasks.len() - 1))
        }
    }
}
