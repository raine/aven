use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;

use crate::tui::event::Action;
use crate::tui::store::{SidebarEntry, TuiStore};
use crate::tui::ui;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    Sidebar,
    Tasks,
}

pub(crate) struct WidgetState {
    pub(crate) sidebar: ListState,
    pub(crate) table: TableState,
}

pub(crate) struct App {
    pub(crate) store: TuiStore,
    pub(crate) should_quit: bool,
    pub(crate) focus: Focus,
    pub(crate) widgets: WidgetState,
    pub(crate) detail_open: bool,
    pub(crate) help_open: bool,
    pub(crate) search_open: bool,
    pub(crate) search_input: String,
    pub(crate) message: Option<String>,
}

impl App {
    pub(crate) async fn new(pool: SqlitePool) -> Result<Self> {
        let store = TuiStore::new(pool).await?;
        let mut app = Self {
            store,
            should_quit: false,
            focus: Focus::Tasks,
            widgets: WidgetState {
                sidebar: ListState::default(),
                table: TableState::default(),
            },
            detail_open: false,
            help_open: false,
            search_open: false,
            search_input: String::new(),
            message: None,
        };
        app.widgets.sidebar.select(app.store.sidebar_selection());
        app.widgets
            .table
            .select(Some(0).filter(|_| !app.store.tasks.is_empty()));
        Ok(app)
    }

    pub(crate) async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            let view = self.view();
            terminal.draw(|frame| ui::render(frame, &self.store, &mut self.widgets, &view))?;

            if event::poll(Duration::from_millis(250))?
                && let Event::Key(key) = event::read()?
            {
                self.handle(Action::from_key(key.code, self.search_open))
                    .await?;
            }

            if self.store.last_refresh.elapsed() >= Duration::from_secs(5) {
                self.refresh().await?;
            }
        }
        Ok(())
    }

    pub(crate) fn view(&self) -> ui::ViewState {
        ui::ViewState {
            focus: self.focus,
            detail_open: self.detail_open,
            help_open: self.help_open,
            search_open: self.search_open,
            search_input: self.search_input.clone(),
            message: self.message.clone(),
        }
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
                self.store.cycle_sort();
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

    async fn refresh(&mut self) -> Result<()> {
        let selected_id = self
            .store
            .selected_task(self.widgets.table.selected())
            .map(|item| item.task.id.clone());
        self.widgets
            .table
            .select(self.store.refresh(selected_id.as_deref()).await?);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        Ok(())
    }

    async fn move_selection(&mut self, delta: isize) -> Result<()> {
        match self.focus {
            Focus::Tasks => {
                let next = next_index(
                    self.widgets.table.selected(),
                    self.store.tasks.len(),
                    delta,
                    true,
                );
                self.widgets.table.select(next);
            }
            Focus::Sidebar => {
                let next = next_selectable_sidebar(
                    self.widgets.sidebar.selected(),
                    &self.store.sidebar_entries,
                    delta,
                    true,
                );
                self.widgets.sidebar.select(next);
                self.apply_sidebar_selection().await?;
            }
        }
        Ok(())
    }

    async fn select_edge(&mut self, last: bool) -> Result<()> {
        match self.focus {
            Focus::Tasks => {
                if self.store.tasks.is_empty() {
                    self.widgets.table.select(None);
                } else {
                    self.widgets.table.select(Some(if last {
                        self.store.tasks.len() - 1
                    } else {
                        0
                    }));
                }
            }
            Focus::Sidebar => {
                let next = if last {
                    self.store
                        .sidebar_entries
                        .iter()
                        .rposition(|entry| entry.target.is_some())
                } else {
                    self.store
                        .sidebar_entries
                        .iter()
                        .position(|entry| entry.target.is_some())
                };
                self.widgets.sidebar.select(next);
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
        self.store
            .apply_sidebar_selection(self.widgets.sidebar.selected())
            .await?;
        self.detail_open = false;
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.widgets
            .table
            .select(Some(0).filter(|_| !self.store.tasks.is_empty()));
        Ok(())
    }

    fn begin_search(&mut self) {
        self.search_open = true;
        self.search_input = self.store.filters.search.clone().unwrap_or_default();
    }

    async fn accept_search(&mut self) -> Result<()> {
        self.search_open = false;
        self.widgets
            .table
            .select(self.store.accept_search(&self.search_input).await?);
        Ok(())
    }

    fn cancel_search(&mut self) {
        self.search_open = false;
        self.search_input.clear();
    }

    async fn update_status(&mut self, status: &'static str) -> Result<()> {
        if let Some(message) = self
            .store
            .update_status(self.widgets.table.selected(), status)
            .await?
        {
            self.message = Some(message);
            self.restore_selection_after_mutation();
        }
        Ok(())
    }

    async fn update_priority(&mut self, reverse: bool) -> Result<()> {
        if let Some(message) = self
            .store
            .update_priority(self.widgets.table.selected(), reverse)
            .await?
        {
            self.message = Some(message);
            self.restore_selection_after_mutation();
        }
        Ok(())
    }

    async fn update_deleted(&mut self, deleted: bool) -> Result<()> {
        if let Some(message) = self
            .store
            .update_deleted(self.widgets.table.selected(), deleted)
            .await?
        {
            self.message = Some(message);
            self.restore_selection_after_mutation();
        }
        Ok(())
    }

    fn restore_selection_after_mutation(&mut self) {
        self.widgets.sidebar.select(self.store.sidebar_selection());
        if self.store.tasks.is_empty() {
            self.widgets.table.select(None);
        } else if self
            .widgets
            .table
            .selected()
            .is_none_or(|index| index >= self.store.tasks.len())
        {
            self.widgets.table.select(Some(0));
        }
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
    for _ in 0..entries.len() {
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
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::store::SidebarTarget;

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
