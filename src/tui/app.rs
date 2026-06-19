use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;

use crate::tui::event::{
    Action, CommandLookup, ShortcutLookup, lookup_command, resolve_shortcut, shortcut_label,
};
use crate::tui::overlay::{OverlayOutcome, OverlayState, OverlayView};
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
    pub(crate) overlay: Option<OverlayState>,
    pub(crate) message: Option<String>,
    pub(crate) message_at: Option<Instant>,
    pending_shortcut: Vec<KeyCode>,
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
            overlay: None,
            message: None,
            message_at: None,
            pending_shortcut: Vec::new(),
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
                let result = self.dispatch_key(key).await;
                if let Err(error) = result {
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
            overlay: self.overlay.as_ref().map(OverlayView::from),
            message: self.message.clone(),
            pending_shortcut: self
                .pending_shortcut
                .iter()
                .map(|code| crate::tui::event::key_label(*code))
                .collect(),
        }
    }

    async fn dispatch_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.overlay_captures_input() {
            self.handle_overlay_key(key).await
        } else {
            self.handle_normal_key(key.code).await
        }
    }

    fn overlay_captures_input(&self) -> bool {
        self.overlay
            .as_ref()
            .is_some_and(OverlayState::captures_input)
    }

    async fn handle_normal_key(&mut self, code: KeyCode) -> Result<()> {
        if self.overlay_captures_input() {
            return self
                .handle_overlay_key(KeyEvent::new(code, KeyModifiers::NONE))
                .await;
        }

        if code == KeyCode::Esc {
            if !self.pending_shortcut.is_empty() {
                self.pending_shortcut.clear();
            } else {
                self.handle(Action::CancelOverlay).await?;
            }
            return Ok(());
        }

        let mut sequence = self.pending_shortcut.clone();
        sequence.push(code);
        match resolve_shortcut(&sequence) {
            ShortcutLookup::Found(action) | ShortcutLookup::Ambiguous(action) => {
                self.pending_shortcut.clear();
                self.handle(action).await?;
            }
            ShortcutLookup::Prefix => {
                self.pending_shortcut = sequence;
            }
            ShortcutLookup::Missing => {
                let label = shortcut_label(&sequence);
                self.pending_shortcut.clear();
                self.set_message(format!("invalid shortcut: {label}"));
            }
        }
        Ok(())
    }

    pub(crate) async fn handle_overlay_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(overlay) = self.overlay.take() else {
            return Ok(());
        };

        match overlay {
            OverlayState::Search { mut input } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.accept_search_input(input).await?,
                KeyCode::Backspace => {
                    input.pop();
                    self.overlay = Some(OverlayState::Search { input });
                }
                KeyCode::Char(ch) => {
                    input.push(ch);
                    self.overlay = Some(OverlayState::Search { input });
                }
                _ => self.overlay = Some(OverlayState::Search { input }),
            },
            OverlayState::Command { mut input } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Some(action) = self.accept_command_input(&input) {
                        self.execute(action).await?;
                    } else {
                        self.overlay = Some(OverlayState::Command { input });
                    }
                }
                KeyCode::Backspace => {
                    input.pop();
                    self.overlay = Some(OverlayState::Command { input });
                }
                KeyCode::Char(ch) => {
                    input.push(ch);
                    self.overlay = Some(OverlayState::Command { input });
                }
                _ => self.overlay = Some(OverlayState::Command { input }),
            },
            overlay => self.handle_generic_overlay_key(key, overlay),
        }

        Ok(())
    }

    fn handle_generic_overlay_key(&mut self, key: KeyEvent, overlay: OverlayState) {
        let outcome = crate::tui::overlay::handle_generic_overlay_key(key, overlay);
        match outcome {
            OverlayOutcome::None(overlay) => self.overlay = Some(overlay),
            OverlayOutcome::Cancelled => {}
            OverlayOutcome::Submitted(submit) => self.set_message(submit.message()),
        }
    }

    async fn handle(&mut self, action: Action) -> Result<()> {
        self.execute(action).await
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
            Action::ToggleHelp => self.toggle_help(),
            Action::BeginSearch => self.begin_search(),
            Action::BeginCommand => self.begin_command(),
            Action::Refresh => self.refresh().await?,
            Action::CycleSort => {
                self.store.cycle_sort();
                self.refresh().await?;
            }
            Action::SetStatus(status) => self.update_status(status).await?,
            Action::CyclePriority(reverse) => self.update_priority(reverse).await?,
            Action::Delete => self.update_deleted(true).await?,
            Action::Restore => self.update_deleted(false).await?,
            Action::Planned(name) => self.set_message(format!(":{name} is not yet implemented")),
            Action::Disabled(name) => self.set_message(format!(":{name} is disabled")),
            Action::AcceptCommand
            | Action::CancelCommand
            | Action::BackspaceCommand
            | Action::CommandChar(_)
            | Action::AcceptSearch
            | Action::CancelSearch
            | Action::BackspaceSearch
            | Action::SearchChar(_)
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
            self.apply_sidebar_selection().await?;
        } else if matches!(self.overlay, Some(OverlayState::Detail)) {
            self.overlay = None;
        } else {
            self.overlay = Some(OverlayState::Detail);
        }
        Ok(())
    }

    async fn apply_sidebar_selection(&mut self) -> Result<()> {
        self.store
            .apply_sidebar_selection(self.widgets.sidebar.selected())
            .await?;
        self.focus = Focus::Tasks;
        self.overlay = None;
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.widgets
            .table
            .select(Some(0).filter(|_| !self.store.tasks.is_empty()));
        Ok(())
    }

    pub(crate) fn begin_search(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::Search {
            input: self.store.filters.search.clone().unwrap_or_default(),
        });
    }

    async fn accept_search_input(&mut self, input: String) -> Result<()> {
        self.widgets
            .table
            .select(self.store.accept_search(&input).await?);
        Ok(())
    }

    pub(crate) fn begin_command(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::Command {
            input: String::new(),
        });
    }

    fn accept_command_input(&mut self, input: &str) -> Option<Action> {
        match lookup_command(input) {
            CommandLookup::Found(action) => {
                self.pending_shortcut.clear();
                Some(action)
            }
            CommandLookup::Empty => {
                self.set_message("empty command".to_string());
                None
            }
            CommandLookup::Ambiguous => {
                self.set_message(format!("ambiguous command: {}", input.trim()));
                None
            }
            CommandLookup::Missing => {
                self.set_message(format!("unknown command: {}", input.trim()));
                None
            }
        }
    }

    fn toggle_help(&mut self) {
        if matches!(self.overlay, Some(OverlayState::Help)) {
            self.overlay = None;
        } else {
            self.overlay = Some(OverlayState::Help);
        }
    }

    fn cancel_overlay(&mut self) {
        self.pending_shortcut.clear();
        let had_overlay = self.overlay.take().is_some();
        if !had_overlay && self.focus == Focus::Sidebar {
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
    use crate::tui::overlay::{ConfirmState, TextInputState};
    use crate::tui::store::SidebarTarget;

    async fn test_app() -> App {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        App::new(pool).await.unwrap()
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

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

    #[tokio::test]
    async fn prefix_key_enters_prefix_mode() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        assert_eq!(app.pending_shortcut, vec![KeyCode::Char('m')]);
    }

    #[tokio::test]
    async fn prefix_is_inactive_while_overlay_captures_input() {
        let mut app = test_app().await;
        app.begin_search();
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();

        assert!(app.pending_shortcut.is_empty());
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search { input }) if input == "m"
        ));
    }

    #[tokio::test]
    async fn esc_cancels_prefix_before_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Help);
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        app.handle_normal_key(KeyCode::Esc).await.unwrap();
        assert!(app.pending_shortcut.is_empty());
        assert!(matches!(app.overlay, Some(OverlayState::Help)));

        app.handle_normal_key(KeyCode::Esc).await.unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn command_overlay_executes_unique_lookup_and_keeps_overlay_on_errors() {
        let mut app = test_app().await;

        app.begin_command();
        for ch in "ref".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());

        app.begin_command();
        app.handle_overlay_key(key(KeyCode::Char('s')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
        assert_eq!(app.message.as_deref(), Some("ambiguous command: s"));

        app.begin_command();
        for ch in "zzzz".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Command { .. })));
        assert_eq!(app.message.as_deref(), Some("unknown command: zzzz"));
    }

    #[tokio::test]
    async fn search_replaces_existing_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Help);
        app.begin_search();
        assert!(matches!(app.overlay, Some(OverlayState::Search { .. })));
    }

    #[tokio::test]
    async fn toggle_help_closes_active_help_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Help);
        app.toggle_help();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn invalid_continuation_shows_message() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('z')).await.unwrap();
        assert!(app.pending_shortcut.is_empty());
        assert_eq!(app.message.as_deref(), Some("invalid shortcut: m z"));
    }

    #[tokio::test]
    async fn valid_continuation_executes_and_clears() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert!(app.pending_shortcut.is_empty());
    }

    #[tokio::test]
    async fn planned_shortcut_reports_not_yet_implemented() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        assert_eq!(
            app.message.as_deref(),
            Some(":add-task is not yet implemented")
        );
    }

    #[tokio::test]
    async fn disabled_shortcut_reports_disabled() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert_eq!(app.message.as_deref(), Some(":conflict-use-a is disabled"));
    }

    #[tokio::test]
    async fn generic_text_input_submits_message() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::TextInput(TextInputState {
            title: "Title".to_string(),
            prompt: "Enter title".to_string(),
            input: "done".to_string(),
            cursor: 4,
        }));
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("submitted Title"));
    }

    #[tokio::test]
    async fn generic_confirm_submits_on_y() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Confirm(ConfirmState {
            title: "Delete".to_string(),
            prompt: "Continue?".to_string(),
        }));
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("confirmed Delete"));
    }
}
