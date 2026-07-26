use anyhow::Result;

use super::{TaskOrder, TaskView, TuiStore};

impl TuiStore {
    pub(crate) fn sort_label(&self) -> &'static str {
        if self.view_state.view == TaskView::Queue {
            return "ranked";
        }
        if self.view_state.view == TaskView::Search {
            return "relevance";
        }
        if self.view_state.view == TaskView::Upcoming {
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

    pub(crate) async fn set_order(&mut self, order: TaskOrder) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        Self::set_view_order(&mut view_state, order);
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }

    pub(crate) async fn reverse_sort(&mut self) -> Result<Option<usize>> {
        let mut view_state = self.view_state.clone();
        Self::reverse_view_order(&mut view_state);
        Ok(self
            .refresh_with_view_state(view_state, None)
            .await?
            .selected)
    }
}
