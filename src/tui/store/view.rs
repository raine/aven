use anyhow::Result;

use crate::query::{self, SortDirection, TaskSearchQuery};

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
        if view != TaskView::Search {
            self.view_state.filter_modifiers.task_ids.clear();
        }
        self.refresh(None).await
    }

    pub(crate) async fn show_scope(&mut self, target: TaskScopeTarget) -> Result<Option<usize>> {
        self.view_state.filter_modifiers.task_ids.clear();
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
        let modifiers = &mut self.view_state.filter_modifiers;
        if modifiers.deleted_only {
            modifiers.deleted_only = false;
            modifiers.include_deleted = false;
        } else if modifiers.include_deleted {
            modifiers.deleted_only = true;
        } else {
            modifiers.include_deleted = true;
        }
        self.refresh(None).await
    }

    pub(crate) async fn search_preview(
        &self,
        input: &str,
        limit: usize,
    ) -> Result<query::TaskSearchPreviewResultSet> {
        let text = input.trim();
        if text.is_empty() {
            return Ok(query::TaskSearchPreviewResultSet {
                items: Vec::new(),
                total_matches: 0,
            });
        }
        let mut conn = self.pool.acquire().await?;
        query::search_task_preview_set_in_workspace(
            &mut conn,
            self.active_workspace.id.as_str(),
            TaskSearchQuery {
                text: text.to_string(),
                include_deleted: false,
                limit,
            },
        )
        .await
    }

    pub(crate) async fn accept_search(&mut self, input: &str) -> Result<Option<usize>> {
        let text = input.trim();
        if text.is_empty() {
            self.view_state.filter_modifiers.task_ids.clear();
            self.view_state.view = TaskView::Queue;
            return self.refresh(None).await;
        }
        let mut conn = self.pool.acquire().await?;
        let results = query::search_task_items_in_workspace(
            &mut conn,
            self.active_workspace.id.as_str(),
            TaskSearchQuery {
                text: text.to_string(),
                include_deleted: false,
                limit: 100,
            },
        )
        .await?;
        drop(conn);
        self.view_state.scope = TaskScope::Workspace;
        self.view_state.view = TaskView::Search;
        self.view_state.filter_modifiers = TaskFilterModifiers {
            task_ids: results
                .iter()
                .map(|result| result.item.task.id.clone())
                .collect(),
            ..TaskFilterModifiers::default()
        };
        self.refresh(None).await
    }

    pub(crate) fn set_view_order(&mut self, order: TaskOrder) {
        if self.view_state.view == TaskView::Queue {
            self.view_state.view = TaskView::Open;
        }
        self.view_state.order = order;
        if order == TaskOrder::Created {
            self.view_state.direction = SortDirection::Desc;
        }
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
