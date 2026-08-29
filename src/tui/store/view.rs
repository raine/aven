use crate::ids::WorkspaceId;
use anyhow::Result;
use aven_core::db::Database;
use tokio::task::JoinHandle;

use crate::query::{self, SortDirection, TaskSearchQuery};

use super::{
    ClosedTaskVisibility, MainRowIdentity, MainRowPosition, MainRowSelection, SelectionRestore,
    SidebarEntryTarget, TaskFilterModifiers, TaskOrder, TaskProjectionOrigin, TaskQuery, TaskScope,
    TaskScopeTarget, TuiStore,
};

async fn search_preview_with_database(
    database: Database,
    workspace_id: WorkspaceId,
    input: String,
    project: Option<String>,
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
        .search_task_preview_set_from_current_projection(
            &workspace_id,
            TaskSearchQuery {
                text,
                project,
                metadata: Vec::new(),
                has_metadata: Vec::new(),
                missing_metadata: Vec::new(),
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
                Some(SidebarEntryTarget::View(view)) => *view == self.view_state.query,
                _ => false,
            })
            .or(Some(1))
    }

    #[cfg(test)]
    pub(crate) async fn show_view(&mut self, query: TaskQuery) -> Result<Option<usize>> {
        self.show_view_restoring(query, &SelectionRestore::Default)
            .await
    }

    pub(crate) async fn show_view_restoring(
        &mut self,
        query: TaskQuery,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.set_query(query);
        if !query.supports_closed_filter() {
            view_state.filter_modifiers.closed = ClosedTaskVisibility::Default;
        }
        if matches!(query, TaskQuery::Upcoming | TaskQuery::Overdue) {
            view_state.direction = SortDirection::Asc;
        }
        if query == TaskQuery::Search
            && matches!(
                view_state.projection_origin,
                TaskProjectionOrigin::NamedView
            )
        {
            view_state.projection_origin = TaskProjectionOrigin::SearchPrompt;
        } else if query != TaskQuery::Search {
            view_state.projection_origin = TaskProjectionOrigin::NamedView;
        }
        Ok(self
            .refresh_replacement(restore, Some(view_state), None)
            .await?
            .selected)
    }

    pub(crate) async fn restore_view_state(
        &mut self,
        view_state: super::TaskViewState,
        selected: Option<&MainRowSelection>,
    ) -> Result<super::ScopeRefreshResult> {
        let restore = selected
            .cloned()
            .map(SelectionRestore::Identity)
            .unwrap_or(SelectionRestore::Default);
        self.restore_view_state_with_restore(view_state, &restore)
            .await
    }

    pub(crate) async fn restore_view_state_with_restore(
        &mut self,
        view_state: super::TaskViewState,
        restore: &SelectionRestore,
    ) -> Result<super::ScopeRefreshResult> {
        self.refresh_replacement(restore, Some(view_state), None)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn show_scope(&mut self, target: TaskScopeTarget) -> Result<Option<usize>> {
        self.show_scope_restoring(target, &SelectionRestore::Default)
            .await
    }

    pub(crate) async fn show_scope_restoring(
        &mut self,
        target: TaskScopeTarget,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.reset_projection_origin();
        view_state.scope = match target {
            TaskScopeTarget::Workspace => TaskScope::Workspace,
            TaskScopeTarget::Project(project) => TaskScope::Project(project),
        };
        Ok(self
            .refresh_replacement(restore, Some(view_state), None)
            .await?
            .selected)
    }

    #[cfg(test)]
    pub(crate) async fn clear_filters(&mut self) -> Result<Option<usize>> {
        self.clear_filters_restoring(&SelectionRestore::Default)
            .await
    }

    #[cfg(test)]
    pub(crate) async fn clear_filters_restoring(
        &mut self,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        self.clear_filters_restoring_history(restore, &[]).await
    }

    pub(crate) async fn clear_filters_restoring_history(
        &mut self,
        restore: &SelectionRestore,
        historical_identities: &[MainRowIdentity],
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.filter_modifiers = TaskFilterModifiers::default();
        view_state.reset_projection_origin();
        view_state.recurring = super::RecurringSeriesViewState::default();
        let result = self
            .refresh_replacement(restore, Some(view_state), None)
            .await?;
        Ok(historical_identities
            .iter()
            .find_map(|identity| self.identity_selection(identity))
            .or(result.selected))
    }

    pub(crate) async fn filter_label_restoring(
        &mut self,
        label: String,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.filter_modifiers.label = Some(label);
        Ok(self
            .refresh_replacement(restore, Some(view_state), None)
            .await?
            .selected)
    }

    pub(crate) async fn filter_priority_restoring(
        &mut self,
        priority: String,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.filter_modifiers.priority = Some(priority);
        Ok(self
            .refresh_replacement(restore, Some(view_state), None)
            .await?
            .selected)
    }

    #[cfg(test)]
    pub(crate) async fn toggle_closed_filter(&mut self) -> Result<Option<usize>> {
        self.toggle_closed_filter_restoring(&SelectionRestore::Default)
            .await
    }

    pub(crate) async fn toggle_closed_filter_restoring(
        &mut self,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.filter_modifiers.closed = match view_state.filter_modifiers.closed {
            ClosedTaskVisibility::Default => ClosedTaskVisibility::Included,
            ClosedTaskVisibility::Included => ClosedTaskVisibility::Only,
            ClosedTaskVisibility::Only => ClosedTaskVisibility::Default,
        };
        Ok(self
            .refresh_replacement(restore, Some(view_state), None)
            .await?
            .selected)
    }

    #[cfg(test)]
    pub(crate) async fn toggle_deleted_filter(&mut self) -> Result<Option<usize>> {
        self.toggle_deleted_filter_restoring(&SelectionRestore::Default)
            .await
    }

    pub(crate) async fn toggle_deleted_filter_restoring(
        &mut self,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
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
            .refresh_replacement(restore, Some(view_state), None)
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
            None,
            limit,
        )
        .await
    }

    pub(crate) fn spawn_search_preview(
        &self,
        input: String,
        project: Option<String>,
        limit: usize,
    ) -> JoinHandle<Result<query::TaskSearchPreviewResultSet>> {
        tokio::spawn(search_preview_with_database(
            self.database.clone(),
            self.active_workspace.id.clone(),
            input,
            project,
            limit,
        ))
    }

    pub(crate) async fn accept_search(&mut self, input: &str) -> Result<Option<usize>> {
        let text = input.trim();
        if text.is_empty() {
            let mut view_state = self.view_state.clone();
            view_state.projection_origin = TaskProjectionOrigin::NamedView;
            view_state.set_query(TaskQuery::Queue);
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
                    project: None,
                    metadata: Vec::new(),
                    has_metadata: Vec::new(),
                    missing_metadata: Vec::new(),
                    include_deleted: false,
                    limit: 100,
                },
            )
            .await?;
        let mut view_state = self.view_state.clone();
        view_state.scope = TaskScope::Workspace;
        view_state.set_query(TaskQuery::Search);
        view_state.projection_origin = TaskProjectionOrigin::Search {
            query: text.to_string(),
            task_ids: results
                .iter()
                .map(|result| result.item.task.id.clone())
                .collect(),
        };
        view_state.filter_modifiers = TaskFilterModifiers::default();
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    #[cfg(test)]
    pub(crate) async fn set_recurring_search(
        &mut self,
        input: String,
        selected_id: Option<&aven_core::recurrence::RecurrenceSeriesId>,
    ) -> Result<Option<usize>> {
        let restore = selected_id
            .cloned()
            .map(MainRowIdentity::RecurrenceSeries)
            .map(SelectionRestore::Identity)
            .unwrap_or(SelectionRestore::Default);
        self.set_recurring_search_restoring(input, &restore).await
    }

    pub(crate) async fn set_recurring_search_restoring(
        &mut self,
        input: String,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.recurring.search = (!input.trim().is_empty()).then(|| input.trim().to_string());
        Ok(self
            .refresh_replacement(restore, Some(view_state), None)
            .await?
            .selected)
    }

    pub(crate) async fn cycle_recurring_lifecycle_restoring(
        &mut self,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        use crate::query::RecurrenceSeriesLifecycleFilter as Filter;
        let mut view_state = self.view_state.clone();
        view_state.recurring.lifecycle = match view_state.recurring.lifecycle {
            Filter::ActiveOrPaused => Filter::Active,
            Filter::Active => Filter::Paused,
            Filter::Paused => Filter::Stopped,
            Filter::Stopped => Filter::All,
            Filter::All => Filter::ActiveOrPaused,
        };
        Ok(self
            .refresh_replacement(restore, Some(view_state), None)
            .await?
            .selected)
    }

    pub(crate) async fn show_task_by_id(
        &mut self,
        task_id: crate::ids::TaskId,
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        view_state.scope = TaskScope::Workspace;
        view_state.set_query(TaskQuery::Search);
        view_state.projection_origin = TaskProjectionOrigin::ExactTasks(vec![task_id.clone()]);
        view_state.filter_modifiers = TaskFilterModifiers::default();
        Ok(self
            .refresh_with_view_state(view_state, Some(&task_id))
            .await?
            .selected)
    }

    pub(super) fn set_view_order(view_state: &mut super::TaskViewState, order: TaskOrder) {
        if view_state.query == TaskQuery::Queue {
            view_state.set_query(TaskQuery::Open);
        }
        view_state.order = order;
        if matches!(order, TaskOrder::Created | TaskOrder::Updated) {
            view_state.direction = SortDirection::Desc;
        }
    }

    pub(super) fn reverse_view_order(view_state: &mut super::TaskViewState) {
        if view_state.query == TaskQuery::Queue {
            view_state.set_query(TaskQuery::Open);
        }
        view_state.direction = view_state.direction.toggled();
    }

    pub(super) fn restored_main_selection(&self, restore: &SelectionRestore) -> Option<usize> {
        if self.main_row_count() == 0 {
            return None;
        }
        match restore {
            SelectionRestore::Default => self.first_visible_selection(),
            SelectionRestore::Identity(identity) => self
                .identity_selection(identity)
                .or_else(|| self.first_visible_selection()),
            SelectionRestore::Anchor(anchor) => {
                if !self.identity_is_compatible(&anchor.identity) {
                    return self.first_visible_selection();
                }
                self.identity_selection(&anchor.identity)
                    .or_else(|| self.position_selection(&anchor.position))
                    .or_else(|| self.first_visible_selection())
            }
            SelectionRestore::Index(index) => Some((*index).min(self.main_row_count() - 1)),
        }
    }

    fn identity_is_compatible(&self, identity: &MainRowIdentity) -> bool {
        matches!(
            (self.view_state.query, identity),
            (TaskQuery::Recurring, MainRowIdentity::RecurrenceSeries(_))
                | (TaskQuery::RecentActions, MainRowIdentity::RecentAction(_))
        ) || (!matches!(
            self.view_state.query,
            TaskQuery::Recurring | TaskQuery::RecentActions
        ) && matches!(identity, MainRowIdentity::Task(_)))
    }

    fn identity_selection(&self, identity: &MainRowIdentity) -> Option<usize> {
        if !self.identity_is_compatible(identity) {
            return None;
        }
        match identity {
            MainRowIdentity::Task(id) => self.tasks.iter().position(|item| &item.task.id == id),
            MainRowIdentity::RecurrenceSeries(id) => self
                .recurrence_series
                .iter()
                .position(|item| &item.series.id == id),
            MainRowIdentity::RecentAction(id) => self
                .recent_actions
                .iter()
                .position(|item| &item.change_id == id),
        }
    }

    fn first_visible_selection(&self) -> Option<usize> {
        if self.view_state.is_columns() {
            return self.column_board().first();
        }
        match self.view_state.query {
            TaskQuery::Epics => self.epic_selection_at_or_near_visual_row(0),
            _ => (self.main_row_count() > 0).then_some(0),
        }
    }

    fn position_selection(&self, position: &MainRowPosition) -> Option<usize> {
        if self.view_state.is_columns() {
            return match position {
                MainRowPosition::Column { column, row } => {
                    self.column_board().selection_at_or_near(*column, *row)
                }
                _ => None,
            };
        }
        match (self.view_state.query, position) {
            (TaskQuery::Epics, MainRowPosition::EpicVisualRow(row)) => {
                self.epic_selection_at_or_near_visual_row(*row)
            }
            (TaskQuery::Epics, _) => None,
            (_, MainRowPosition::Flat(row)) => Some((*row).min(self.main_row_count() - 1)),
            _ => None,
        }
    }

    fn epic_selection_at_or_near_visual_row(&self, row: usize) -> Option<usize> {
        let count = crate::tui::ui::task_visual_row_count(self);
        if count == 0 {
            return None;
        }
        let row = row.min(count - 1);
        (0..count).find_map(|distance| {
            row.checked_sub(distance)
                .and_then(|candidate| crate::tui::ui::task_index_at_visual_row(self, candidate))
                .or_else(|| {
                    let candidate = row.saturating_add(distance);
                    (candidate < count)
                        .then(|| crate::tui::ui::task_index_at_visual_row(self, candidate))
                        .flatten()
                })
        })
    }

    pub(super) fn restored_task_selection(
        &self,
        selected_id: Option<&crate::ids::TaskId>,
    ) -> Option<usize> {
        let restore = selected_id
            .cloned()
            .map(MainRowIdentity::Task)
            .map(SelectionRestore::Identity)
            .unwrap_or(SelectionRestore::Default);
        self.restored_main_selection(&restore)
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
