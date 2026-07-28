use crate::ids::WorkspaceId;
use anyhow::Result;
use aven_core::db::Database;
use tokio::task::JoinHandle;

use crate::query::{self, SortDirection, TaskSearchQuery};

use super::{
    ClosedTaskVisibility, SidebarEntryTarget, TaskFilterModifiers, TaskOrder, TaskScope,
    TaskScopeTarget, TaskView, TuiStore,
};

async fn search_preview_with_database(
    database: Database,
    workspace_id: WorkspaceId,
    input: String,
    limit: usize,
) -> Result<query::TaskSearchPreviewResultSet> {
    let text = input.trim().to_string();
    if text.is_empty() {
        return Ok(query::TaskSearchPreviewResultSet {
            items: Vec::new(),
            total_matches: 0,
        });
    }
    database
        .search_task_preview_set(
            &workspace_id,
            TaskSearchQuery {
                text,
                include_deleted: false,
                limit,
            },
        )
        .await
}

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

    pub(crate) async fn show_view(&mut self, view: TaskView) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.view = view;
        if !view.supports_closed_filter() {
            view_state.filter_modifiers.closed = ClosedTaskVisibility::Default;
        }
        if view == TaskView::Upcoming {
            view_state.direction = SortDirection::Asc;
        }
        if view != TaskView::Search {
            view_state.filter_modifiers.task_ids.clear();
        }
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    pub(crate) async fn restore_view_state(
        &mut self,
        view_state: super::TaskViewState,
    ) -> Result<super::ScopeRefreshResult> {
        self.refresh_with_view_state(view_state, None).await
    }

    pub(crate) async fn show_scope(&mut self, target: TaskScopeTarget) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.filter_modifiers.task_ids.clear();
        view_state.scope = match target {
            TaskScopeTarget::Workspace => TaskScope::Workspace,
            TaskScopeTarget::Project(project) => TaskScope::Project(project),
        };
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    pub(crate) async fn clear_filters(&mut self) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.filter_modifiers = TaskFilterModifiers::default();
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    pub(crate) async fn filter_label(&mut self, label: String) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.filter_modifiers.label = Some(label);
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    pub(crate) async fn filter_priority(&mut self, priority: String) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.filter_modifiers.priority = Some(priority);
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    pub(crate) async fn toggle_closed_filter(&mut self) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.filter_modifiers.closed = match view_state.filter_modifiers.closed {
            ClosedTaskVisibility::Default => ClosedTaskVisibility::Included,
            ClosedTaskVisibility::Included => ClosedTaskVisibility::Only,
            ClosedTaskVisibility::Only => ClosedTaskVisibility::Default,
        };
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    pub(crate) async fn toggle_deleted_filter(&mut self) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        let modifiers = &mut view_state.filter_modifiers;
        if modifiers.deleted_only {
            modifiers.deleted_only = false;
            modifiers.include_deleted = false;
        } else if modifiers.include_deleted {
            modifiers.deleted_only = true;
        } else {
            modifiers.include_deleted = true;
        }
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    #[cfg(test)]
    pub(crate) async fn search_preview(
        &self,
        input: &str,
        limit: usize,
    ) -> Result<query::TaskSearchPreviewResultSet> {
        search_preview_with_database(
            self.database.clone(),
            self.active_workspace.id.clone(),
            input.to_string(),
            limit,
        )
        .await
    }

    pub(crate) fn spawn_search_preview(
        &self,
        input: String,
        limit: usize,
    ) -> JoinHandle<Result<query::TaskSearchPreviewResultSet>> {
        tokio::spawn(search_preview_with_database(
            self.database.clone(),
            self.active_workspace.id.clone(),
            input,
            limit,
        ))
    }

    pub(crate) async fn accept_search(&mut self, input: &str) -> Result<Option<usize>> {
        let text = input.trim();
        if text.is_empty() {
            let mut view_state = self.view_state.clone();
            view_state.filter_modifiers.task_ids.clear();
            view_state.view = TaskView::Queue;
            return Ok(self
                .refresh_with_view_state(view_state, None)
                .await?
                .selected);
        }
        let results = self
            .database
            .search_task_items(
                &self.active_workspace.id,
                TaskSearchQuery {
                    text: text.to_string(),
                    include_deleted: false,
                    limit: 100,
                },
            )
            .await?;
        let mut view_state = self.view_state.clone();
        view_state.scope = TaskScope::Workspace;
        view_state.view = TaskView::Search;
        view_state.filter_modifiers = TaskFilterModifiers {
            task_ids: results
                .iter()
                .map(|result| result.item.task.id.clone())
                .collect(),
            ..TaskFilterModifiers::default()
        };
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    pub(super) fn set_view_order(view_state: &mut super::TaskViewState, order: TaskOrder) {
        if view_state.view == TaskView::Queue {
            view_state.view = TaskView::Open;
        }
        view_state.order = order;
        if order == TaskOrder::Created {
            view_state.direction = SortDirection::Desc;
        }
    }

    pub(super) fn reverse_view_order(view_state: &mut super::TaskViewState) {
        if view_state.view == TaskView::Queue {
            view_state.view = TaskView::Open;
        }
        view_state.direction = view_state.direction.toggled();
    }

    pub(super) fn restored_task_selection(
        &self,
        selected_id: Option<&crate::ids::TaskId>,
    ) -> Option<usize> {
        if self.tasks.is_empty() {
            return None;
        }
        selected_id
            .and_then(|id| self.tasks.iter().position(|item| &item.task.id == id))
            .or(Some(0))
    }

    pub(crate) fn restored_task_selection_at_index(
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
