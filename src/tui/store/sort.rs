use anyhow::Result;

use crate::query::{SortDirection, TaskSort};

use super::TuiStore;

impl TuiStore {
    pub(crate) fn sort_label(&self) -> &'static str {
        match self.sort {
            TaskSort::Queue => "queue",
            TaskSort::Created => "created",
            TaskSort::Updated => "updated",
            TaskSort::Priority => "priority",
            TaskSort::Project => "project",
            TaskSort::Title => "title",
        }
    }

    pub(crate) fn sort_direction_label(&self) -> &'static str {
        match self.sort_direction {
            SortDirection::Asc => "asc",
            SortDirection::Desc => "desc",
        }
    }

    pub(crate) fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            TaskSort::Queue => TaskSort::Created,
            TaskSort::Created => TaskSort::Updated,
            TaskSort::Updated => TaskSort::Priority,
            TaskSort::Priority => TaskSort::Project,
            TaskSort::Project => TaskSort::Title,
            TaskSort::Title => TaskSort::Queue,
        };
    }

    pub(crate) async fn set_sort(&mut self, sort: TaskSort) -> Result<Option<usize>> {
        self.sort = sort;
        self.refresh(None).await
    }

    pub(crate) async fn reverse_sort(&mut self) -> Result<Option<usize>> {
        self.sort_direction = self.sort_direction.toggled();
        self.refresh(None).await
    }
}
