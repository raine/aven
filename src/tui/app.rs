use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;

use crate::mutation::{cycle_priority, set_deleted, set_status};
use crate::query::{
    ProjectListItem, SidebarCounts, TaskFilters, TaskListItem, TaskSort, list_project_items,
    list_task_items, sidebar_counts,
};
use crate::tui::event::Action;
use crate::tui::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Tasks,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SidebarTarget {
    All,
    Inbox,
    Active,
    Project(String),
}

#[derive(Debug, Clone)]
pub(crate) struct SidebarEntry {
    pub(crate) label: String,
    pub(crate) count: i64,
    pub(crate) target: Option<SidebarTarget>,
    pub(crate) section: bool,
}

pub(crate) struct App {
    pub(crate) pool: SqlitePool,
    pub(crate) should_quit: bool,
    pub(crate) focus: Focus,
    pub(crate) sidebar: ListState,
    pub(crate) table: TableState,
    pub(crate) tasks: Vec<TaskListItem>,
    pub(crate) projects: Vec<ProjectListItem>,
    pub(crate) counts: SidebarCounts,
    pub(crate) sidebar_entries: Vec<SidebarEntry>,
    pub(crate) active_view: SidebarTarget,
    pub(crate) filters: TaskFilters,
    pub(crate) sort: TaskSort,
    pub(crate) detail_open: bool,
    pub(crate) help_open: bool,
    pub(crate) search_open: bool,
    pub(crate) search_input: String,
    pub(crate) message: Option<String>,
    pub(crate) last_refresh: Instant,
}

impl App {
    pub(crate) async fn new(pool: SqlitePool) -> Result<Self> {
        let mut app = Self {
            pool,
            should_quit: false,
            focus: Focus::Tasks,
            sidebar: ListState::default(),
            table: TableState::default(),
            tasks: Vec::new(),
            projects: Vec::new(),
            counts: SidebarCounts::default(),
            sidebar_entries: Vec::new(),
            active_view: SidebarTarget::All,
            filters: TaskFilters::default(),
            sort: TaskSort::Queue,
            detail_open: false,
            help_open: false,
            search_open: false,
            search_input: String::new(),
            message: None,
            last_refresh: Instant::now(),
        };
        app.refresh().await?;
        app.sidebar.select(Some(1));
        Ok(app)
    }

    pub(crate) async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| ui::render(frame, &mut self))?;

            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key) = event::read()?
            {
                self.handle(Action::from_key(key.code, self.search_open))
                    .await?;
            }

            if self.last_refresh.elapsed() >= Duration::from_secs(5) {
                self.refresh().await?;
            }
        }
        Ok(())
    }

    async fn handle(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Quit => self.should_quit = true,
            Action::MoveDown => self.move_selection(1).await?,
            Action::MoveUp => self.move_selection(-1).await?,
            Action::First => self.select_edge(false).await?,
            Action::Last => self.select_edge(true).await?,
            Action::ToggleFocus => self.toggle_focus(),
            Action::ToggleDetail => self.activate_or_toggle_detail().await?,
            Action::ToggleHelp => self.help_open = !self.help_open,
            Action::BeginSearch => self.begin_search(),
            Action::AcceptSearch => self.accept_search().await?,
            Action::CancelSearch => self.cancel_search(),
            Action::BackspaceSearch => {
                self.search_input.pop();
            }
            Action::SearchChar(ch) => self.search_input.push(ch),
            Action::Refresh => self.refresh().await?,
            Action::CycleSort => {
                self.cycle_sort();
                self.refresh().await?;
            }
            Action::SetStatus(status) => self.update_status(status).await?,
            Action::CyclePriority(reverse) => self.update_priority(reverse).await?,
            Action::Delete => self.update_deleted(true).await?,
            Action::Restore => self.update_deleted(false).await?,
            Action::None => {}
        }
        Ok(())
    }

    pub(crate) fn selected_task(&self) -> Option<&TaskListItem> {
        self.table
            .selected()
            .and_then(|index| self.tasks.get(index))
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

    async fn refresh(&mut self) -> Result<()> {
        let selected_id = self.selected_task().map(|item| item.task.id.clone());
        let mut conn = self.pool.acquire().await?;
        self.projects = list_project_items(&mut conn).await?;
        self.counts = sidebar_counts(&mut conn).await?;
        self.tasks = list_task_items(&mut conn, self.filters.clone(), self.sort).await?;
        self.rebuild_sidebar();
        self.restore_task_selection(selected_id.as_deref());
        self.last_refresh = Instant::now();
        Ok(())
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
                label: "Inbox".to_string(),
                count: self.counts.inbox,
                target: Some(SidebarTarget::Inbox),
                section: false,
            },
            SidebarEntry {
                label: "Active".to_string(),
                count: self.counts.active,
                target: Some(SidebarTarget::Active),
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

        let index = self
            .sidebar_entries
            .iter()
            .position(|entry| entry.target.as_ref() == Some(&self.active_view))
            .unwrap_or(1);
        self.sidebar.select(Some(index));
    }

    fn restore_task_selection(&mut self, selected_id: Option<&str>) {
        if self.tasks.is_empty() {
            self.table.select(None);
            return;
        }
        let selected = selected_id
            .and_then(|id| self.tasks.iter().position(|item| item.task.id == id))
            .or_else(|| {
                self.table
                    .selected()
                    .filter(|index| *index < self.tasks.len())
            })
            .unwrap_or(0);
        self.table.select(Some(selected));
    }

    async fn move_selection(&mut self, delta: isize) -> Result<()> {
        match self.focus {
            Focus::Tasks => {
                let next = next_index(self.table.selected(), self.tasks.len(), delta, true);
                self.table.select(next);
            }
            Focus::Sidebar => {
                let next = next_selectable_sidebar(
                    self.sidebar.selected(),
                    &self.sidebar_entries,
                    delta,
                    true,
                );
                self.sidebar.select(next);
                self.apply_sidebar_selection().await?;
            }
        }
        Ok(())
    }

    async fn select_edge(&mut self, last: bool) -> Result<()> {
        match self.focus {
            Focus::Tasks => {
                if self.tasks.is_empty() {
                    self.table.select(None);
                } else {
                    self.table
                        .select(Some(if last { self.tasks.len() - 1 } else { 0 }));
                }
            }
            Focus::Sidebar => {
                let next = if last {
                    self.sidebar_entries
                        .iter()
                        .rposition(|entry| entry.target.is_some())
                } else {
                    self.sidebar_entries
                        .iter()
                        .position(|entry| entry.target.is_some())
                };
                self.sidebar.select(next);
                self.apply_sidebar_selection().await?;
            }
        }
        Ok(())
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => Focus::Tasks,
            Focus::Tasks => Focus::Sidebar,
        };
    }

    async fn activate_or_toggle_detail(&mut self) -> Result<()> {
        if self.focus == Focus::Sidebar {
            self.apply_sidebar_selection().await
        } else {
            self.detail_open = !self.detail_open;
            Ok(())
        }
    }

    async fn apply_sidebar_selection(&mut self) -> Result<()> {
        let Some(target) = self
            .sidebar
            .selected()
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
            SidebarTarget::Inbox => self.filters.status = Some("inbox".to_string()),
            SidebarTarget::Active => self.filters.status = Some("active".to_string()),
            SidebarTarget::Project(project) => self.filters.project = Some(project.clone()),
        }
        self.detail_open = false;
        self.refresh().await
    }

    fn begin_search(&mut self) {
        self.search_open = true;
        self.search_input = self.filters.search.clone().unwrap_or_default();
    }

    async fn accept_search(&mut self) -> Result<()> {
        self.filters.search = if self.search_input.trim().is_empty() {
            None
        } else {
            Some(self.search_input.trim().to_string())
        };
        self.search_open = false;
        self.refresh().await
    }

    fn cancel_search(&mut self) {
        self.search_open = false;
        self.search_input.clear();
    }

    fn cycle_sort(&mut self) {
        self.sort = match self.sort {
            TaskSort::Queue => TaskSort::Created,
            TaskSort::Created => TaskSort::Updated,
            TaskSort::Updated => TaskSort::Project,
            TaskSort::Project => TaskSort::Title,
            TaskSort::Title => TaskSort::Queue,
        };
    }

    async fn update_status(&mut self, status: &'static str) -> Result<()> {
        if let Some(item) = self.selected_task().cloned() {
            let mut conn = self.pool.acquire().await?;
            set_status(&mut conn, &item.task, status).await?;
            drop(conn);
            self.message = Some(format!("set {} status={status}", item.display_ref));
            self.refresh().await?;
        }
        Ok(())
    }

    async fn update_priority(&mut self, reverse: bool) -> Result<()> {
        if let Some(item) = self.selected_task().cloned() {
            let mut conn = self.pool.acquire().await?;
            let task = cycle_priority(&mut conn, &item.task, reverse).await?;
            drop(conn);
            self.message = Some(format!(
                "set {} priority={}",
                item.display_ref, task.priority
            ));
            self.refresh().await?;
        }
        Ok(())
    }

    async fn update_deleted(&mut self, deleted: bool) -> Result<()> {
        if let Some(item) = self.selected_task().cloned() {
            let mut conn = self.pool.acquire().await?;
            set_deleted(&mut conn, &item.task, deleted).await?;
            drop(conn);
            self.filters.include_deleted = deleted;
            self.message = Some(if deleted {
                format!("deleted {} (showing deleted)", item.display_ref)
            } else {
                format!("restored {}", item.display_ref)
            });
            self.refresh().await?;
        }
        Ok(())
    }
}

fn next_index(selected: Option<usize>, len: usize, delta: isize, wrap: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let current = selected.unwrap_or(0);
    let next = current as isize + delta;
    if (0..len as isize).contains(&next) {
        Some(next as usize)
    } else if wrap && delta > 0 {
        Some(0)
    } else if wrap && delta < 0 {
        Some(len - 1)
    } else {
        Some(current)
    }
}

fn next_selectable_sidebar(
    selected: Option<usize>,
    entries: &[SidebarEntry],
    delta: isize,
    wrap: bool,
) -> Option<usize> {
    if entries.is_empty() || entries.iter().all(|entry| entry.target.is_none()) {
        return None;
    }
    let mut index = selected.unwrap_or(0);
    loop {
        let next = index as isize + delta;
        index = if (0..entries.len() as isize).contains(&next) {
            next as usize
        } else if wrap && delta > 0 {
            0
        } else if wrap && delta < 0 {
            entries.len() - 1
        } else {
            index
        };
        if entries[index].target.is_some() {
            return Some(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(label: &str) -> SidebarEntry {
        SidebarEntry {
            label: label.to_string(),
            count: 0,
            target: None,
            section: true,
        }
    }

    fn item(label: &str) -> SidebarEntry {
        SidebarEntry {
            label: label.to_string(),
            count: 0,
            target: Some(SidebarTarget::All),
            section: false,
        }
    }

    #[test]
    fn wraps_up_from_first_sidebar_item_to_last_item() {
        let entries = [
            section("Smart Views"),
            item("All"),
            section("Projects"),
            item("APP app"),
        ];

        let selected = next_selectable_sidebar(Some(1), &entries, -1, true);

        assert_eq!(selected, Some(3));
    }

    #[test]
    fn wraps_down_from_last_sidebar_item_to_first_item() {
        let entries = [
            section("Smart Views"),
            item("All"),
            section("Projects"),
            item("APP app"),
        ];

        let selected = next_selectable_sidebar(Some(3), &entries, 1, true);

        assert_eq!(selected, Some(1));
    }

    #[test]
    fn wraps_up_from_first_task_to_last_task() {
        assert_eq!(next_index(Some(0), 3, -1, true), Some(2));
    }
}
