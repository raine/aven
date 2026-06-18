use std::time::Instant;

use anyhow::Result;
use sqlx::SqlitePool;

use crate::mutation::{cycle_priority, set_deleted, set_status};
use crate::query::{
    ProjectListItem, SidebarCounts, TaskFilters, TaskListItem, TaskSort, list_project_items,
    list_task_items, sidebar_counts,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarTarget {
    All,
    Active,
    Todo,
    Project(String),
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarEntry {
    pub(crate) label: String,
    pub(crate) count: i64,
    pub(crate) target: Option<SidebarTarget>,
    pub(crate) section: bool,
}

pub(crate) struct TuiStore {
    pool: SqlitePool,
    pub(crate) tasks: Vec<TaskListItem>,
    pub(crate) projects: Vec<ProjectListItem>,
    pub(crate) counts: SidebarCounts,
    pub(crate) sidebar_entries: Vec<SidebarEntry>,
    pub(crate) active_view: SidebarTarget,
    pub(crate) filters: TaskFilters,
    pub(crate) sort: TaskSort,
    pub(crate) last_refresh: Instant,
}

impl TuiStore {
    pub(crate) async fn new(pool: SqlitePool) -> Result<Self> {
        let mut store = Self {
            pool,
            tasks: Vec::new(),
            projects: Vec::new(),
            counts: SidebarCounts::default(),
            sidebar_entries: Vec::new(),
            active_view: SidebarTarget::All,
            filters: TaskFilters::default(),
            sort: TaskSort::Queue,
            last_refresh: Instant::now(),
        };
        store.refresh(None).await?;
        Ok(store)
    }

    pub(crate) fn selected_task(&self, selected: Option<usize>) -> Option<&TaskListItem> {
        selected.and_then(|index| self.tasks.get(index))
    }

    pub(crate) fn sort_label(&self) -> &'static str {
        match self.sort {
            TaskSort::Queue => "queue",
            TaskSort::Created => "created",
            TaskSort::Updated => "updated",
            TaskSort::Project => "project",
            TaskSort::Title => "title",
        }
    }

    pub(crate) async fn refresh(&mut self, selected_id: Option<&str>) -> Result<Option<usize>> {
        let mut conn = self.pool.acquire().await?;
        self.projects = list_project_items(&mut conn).await?;
        self.counts = sidebar_counts(&mut conn).await?;
        self.tasks = list_task_items(&mut conn, self.filters.clone(), self.sort).await?;
        self.rebuild_sidebar();
        self.last_refresh = Instant::now();
        Ok(self.restored_task_selection(selected_id))
    }

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
        self.active_view = target;
        self.filters.project = None;
        self.filters.status = None;
        match &self.active_view {
            SidebarTarget::All => {}
            SidebarTarget::Active => self.filters.status = Some("active".to_string()),
            SidebarTarget::Todo => self.filters.status = Some("todo".to_string()),
            SidebarTarget::Project(project) => self.filters.project = Some(project.clone()),
        }
        self.refresh(None).await?;
        Ok(())
    }

    pub(crate) async fn accept_search(&mut self, input: &str) -> Result<Option<usize>> {
        self.filters.search = if input.trim().is_empty() {
            None
        } else {
            Some(input.trim().to_string())
        };
        self.refresh(None).await
    }

    pub(crate) fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            TaskSort::Queue => TaskSort::Created,
            TaskSort::Created => TaskSort::Updated,
            TaskSort::Updated => TaskSort::Project,
            TaskSort::Project => TaskSort::Title,
            TaskSort::Title => TaskSort::Queue,
        };
    }

    pub(crate) async fn update_status(
        &mut self,
        index: Option<usize>,
        status: &'static str,
    ) -> Result<Option<String>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let mut conn = self.pool.acquire().await?;
            set_status(&mut conn, &item.task, status).await?;
            drop(conn);
            self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(format!("set {} status={status}", item.display_ref)));
        }
        Ok(None)
    }

    pub(crate) async fn update_priority(
        &mut self,
        index: Option<usize>,
        reverse: bool,
    ) -> Result<Option<String>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let mut conn = self.pool.acquire().await?;
            let task = cycle_priority(&mut conn, &item.task, reverse).await?;
            drop(conn);
            self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(format!(
                "set {} priority={}",
                item.display_ref, task.priority
            )));
        }
        Ok(None)
    }

    pub(crate) async fn update_deleted(
        &mut self,
        index: Option<usize>,
        deleted: bool,
    ) -> Result<Option<String>> {
        if let Some(item) = self.selected_task(index).cloned() {
            let mut conn = self.pool.acquire().await?;
            set_deleted(&mut conn, &item.task, deleted).await?;
            drop(conn);
            self.filters.include_deleted = deleted;
            self.refresh(Some(&item.task.id)).await?;
            return Ok(Some(if deleted {
                format!("deleted {} (showing deleted)", item.display_ref)
            } else {
                format!("restored {}", item.display_ref)
            }));
        }
        Ok(None)
    }

    fn rebuild_sidebar(&mut self) {
        let mut entries = vec![
            SidebarEntry {
                label: "Smart Views".to_string(),
                count: 0,
                target: None,
                section: true,
            },
            SidebarEntry {
                label: "All".to_string(),
                count: self.counts.all,
                target: Some(SidebarTarget::All),
                section: false,
            },
            SidebarEntry {
                label: "Active".to_string(),
                count: self.counts.active,
                target: Some(SidebarTarget::Active),
                section: false,
            },
            SidebarEntry {
                label: "Todo".to_string(),
                count: self.counts.todo,
                target: Some(SidebarTarget::Todo),
                section: false,
            },
            SidebarEntry {
                label: String::new(),
                count: 0,
                target: None,
                section: true,
            },
            SidebarEntry {
                label: "Projects".to_string(),
                count: 0,
                target: None,
                section: true,
            },
        ];
        entries.extend(self.projects.iter().map(|project| SidebarEntry {
            label: if project.inbox_count > 0 {
                format!("{} {}*", project.prefix, project.name)
            } else {
                format!("{} {}", project.prefix, project.name)
            },
            count: project.open_count,
            target: Some(SidebarTarget::Project(project.key.clone())),
            section: false,
        }));
        self.sidebar_entries = entries;
    }

    fn restored_task_selection(&self, selected_id: Option<&str>) -> Option<usize> {
        if self.tasks.is_empty() {
            return None;
        }
        selected_id
            .and_then(|id| self.tasks.iter().position(|item| item.task.id == id))
            .or(Some(0))
    }
}
