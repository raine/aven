use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event};
use ratatui::DefaultTerminal;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;

use crate::tui::event::{Action, CommandLookup, lookup_command};
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
    pub(crate) command_open: bool,
    pub(crate) command_input: String,
    pub(crate) message: Option<String>,
    pub(crate) message_at: Option<Instant>,
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
            command_open: false,
            command_input: String::new(),
            message: None,
            message_at: None,
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
                let action = if self.command_open {
                    Action::from_command_key(key.code)
                } else if self.search_open {
                    Action::from_search_key(key.code)
                } else {
                    Action::from_normal_key(key.code)
                };
                if let Err(error) = self.handle(action).await {
                    self.set_message(format!("error: {error:#}"));
                }
            }

            if self.store.last_refresh.elapsed() >= Duration::from_secs(5)
                && let Err(error) = self.refresh().await
            {
                self.set_message(format!("refresh failed: {error:#}"));
            }

            self.clear_expired_message();
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
            command_open: self.command_open,
            command_input: self.command_input.clone(),
            message: self.message.clone(),
        }
    }

    async fn handle(&mut self, action: Action) -> Result<()> {
        match action {
            Action::AcceptCommand => {
                if let Some(action) = self.accept_command() {
                    self.execute(action).await?;
                }
            }
            Action::CancelCommand => self.cancel_command(),
            Action::BackspaceCommand => {
                self.command_input.pop();
            }
            Action::CommandChar(ch) => self.command_input.push(ch),
            action => self.execute(action).await?,
        }
        Ok(())
    }

    async fn execute(&mut self, action: Action) -> Result<()> {
        match action {
            Action::Quit => self.should_quit = true,
            Action::CancelOverlay => self.cancel_overlay(),
            Action::MoveDown => self.move_selection(1).await?,
            Action::MoveUp => self.move_selection(-1).await?,
            Action::First => self.select_edge(false).await?,
            Action::Last => self.select_edge(true).await?,
            Action::ToggleFocus => self.toggle_focus(),
            Action::ToggleDetail => self.activate_or_toggle_detail().await?,
            Action::ToggleHelp => self.help_open = !self.help_open,
            Action::BeginSearch => self.begin_search(),
            Action::BeginCommand => self.begin_command(),
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
            Action::AcceptCommand
            | Action::CancelCommand
            | Action::BackspaceCommand
            | Action::CommandChar(_)
            | Action::None => {}
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
            }
        }
        Ok(())
    }

    fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Sidebar => {
                self.widgets.sidebar.select(self.store.sidebar_selection());
                Focus::Tasks
            }
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
        self.focus = Focus::Tasks;
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

    fn begin_command(&mut self) {
        self.command_open = true;
        self.command_input.clear();
        self.help_open = false;
    }

    fn cancel_command(&mut self) {
        self.command_open = false;
        self.command_input.clear();
    }

    fn accept_command(&mut self) -> Option<Action> {
        match lookup_command(&self.command_input) {
            CommandLookup::Found(action) => {
                self.command_open = false;
                self.command_input.clear();
                Some(action)
            }
            CommandLookup::Empty => {
                self.set_message("empty command".to_string());
                None
            }
            CommandLookup::Ambiguous => {
                self.set_message(format!("ambiguous command: {}", self.command_input.trim()));
                None
            }
            CommandLookup::Missing => {
                self.set_message(format!("unknown command: {}", self.command_input.trim()));
                None
            }
        }
    }

    fn cancel_overlay(&mut self) {
        if self.command_open {
            self.cancel_command();
        } else if self.help_open {
            self.help_open = false;
        } else if self.detail_open {
            self.detail_open = false;
        } else if self.focus == Focus::Sidebar {
            self.focus = Focus::Tasks;
            self.widgets.sidebar.select(self.store.sidebar_selection());
        }
    }

    async fn update_status(&mut self, status: &'static str) -> Result<()> {
        if let Some(message) = self
            .store
            .update_status(self.widgets.table.selected(), status)
            .await?
        {
            self.set_message(message);
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
            self.set_message(message);
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
            self.set_message(message);
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

    fn set_message(&mut self, message: String) {
        self.message = Some(message);
        self.message_at = Some(Instant::now());
    }

    fn clear_expired_message(&mut self) {
        if self
            .message_at
            .is_some_and(|time| time.elapsed() >= Duration::from_secs(4))
        {
            self.message = None;
            self.message_at = None;
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
