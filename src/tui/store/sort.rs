use anyhow::Result;

use super::{SelectionRestore, TaskOrder, TaskQuery, TuiStore};

impl TuiStore {
    pub(crate) fn sort_label(&self) -> &'static str {
        if self.view_state.query == TaskQuery::Queue {
            return "ranked";
        }
        if self.view_state.query == TaskQuery::Search {
            return "relevance";
        }
        if self.view_state.query == TaskQuery::Upcoming {
            return "available";
        }
        match self.view_state.order {
            TaskOrder::Created => "created",
            TaskOrder::Updated => "updated",
            TaskOrder::Priority => "priority",
            TaskOrder::Project => "project",
            TaskOrder::Title => "title",
            TaskOrder::DueOn => "due",
        }
    }

    pub(crate) fn sort_direction_label(&self) -> &'static str {
        match self.view_state.sort_direction() {
            crate::query::SortDirection::Asc => "asc",
            crate::query::SortDirection::Desc => "desc",
        }
    }

    #[cfg(test)]
    pub(crate) async fn set_order(&mut self, order: TaskOrder) -> Result<Option<usize>> {
        self.set_order_restoring(order, &SelectionRestore::Default)
            .await
    }

    pub(crate) async fn set_order_restoring(
        &mut self,
        order: TaskOrder,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        Self::set_view_order(&mut view_state, order);
        Ok(self
            .refresh_replacement(restore, Some(view_state), None)
            .await?
            .selected)
    }

    #[cfg(test)]
    pub(crate) async fn reverse_sort(&mut self) -> Result<Option<usize>> {
        self.reverse_sort_restoring(&SelectionRestore::Default)
            .await
    }

    pub(crate) async fn reverse_sort_restoring(
        &mut self,
        restore: &SelectionRestore,
    ) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        Self::reverse_view_order(&mut view_state);
        Ok(self
            .refresh_replacement(restore, Some(view_state), None)
            .await?
            .selected)
    }
}
