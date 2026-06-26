use anyhow::Result;

use super::{TaskOrder, TaskView, TuiStore};

impl TuiStore {
    pub(crate) fn sort_label(&self) -> &'static str {
        if self.view_state.view == TaskView::Queue {
            return "ranked";
        }
        match self.view_state.order {
            TaskOrder::Created => "created",
            TaskOrder::Updated => "updated",
            TaskOrder::Priority => "priority",
            TaskOrder::Project => "project",
            TaskOrder::Title => "title",
        }
    }

    pub(crate) fn sort_direction_label(&self) -> &'static str {
        match self.view_state.direction {
            crate::query::SortDirection::Asc => "asc",
            crate::query::SortDirection::Desc => "desc",
        }
    }

    pub(crate) async fn set_order(&mut self, order: TaskOrder) -> Result<Option<usize>> {
        self.set_view_order(order);
        self.refresh(None).await
    }

    pub(crate) async fn reverse_sort(&mut self) -> Result<Option<usize>> {
        self.reverse_view_order();
        self.refresh(None).await
    }
}
