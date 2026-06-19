use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;

use crate::operations::TaskDraft;
use crate::tui::event::{
    Action, CommandLookup, ShortcutLookup, ViewTarget, lookup_command, resolve_shortcut,
    shortcut_label,
};
use crate::tui::overlay::{
    MultilineInputState, OverlayOutcome, OverlayState, OverlaySubmit, OverlayView, PickerItem,
    PickerState, TextInputState,
};
use crate::tui::store::{SidebarEntry, SidebarTarget, TuiStore};
use crate::tui::ui;

const ADD_PROJECT_TITLE: &str = "Add project";
const ADD_LABEL_TITLE: &str = "Add label";
const ADD_TASK_TITLE_TITLE: &str = "Add task: title";
const ADD_TASK_PROJECT_TITLE: &str = "Add task: project";
const ADD_TASK_PRIORITY_TITLE: &str = "Add task: priority";
const ADD_TASK_LABELS_TITLE: &str = "Add task: labels";
const ADD_TASK_DESCRIPTION_TITLE: &str = "Add task: description";
const ADD_NOTE_TITLE: &str = "Add note";
const EDIT_TITLE_TITLE: &str = "Edit task: title";
const EDIT_DESCRIPTION_TITLE: &str = "Edit task: description";
const EDIT_PROJECT_TITLE: &str = "Edit task: project";
const EDIT_PRIORITY_TITLE: &str = "Edit task: priority";
const EDIT_LABELS_TITLE: &str = "Edit task: labels";
const FILTER_PROJECT_TITLE: &str = "Filter: project";
const FILTER_LABEL_TITLE: &str = "Filter: label";
const FILTER_STATUS_TITLE: &str = "Filter: status";
const FILTER_PRIORITY_TITLE: &str = "Filter: priority";
const VIEW_PROJECT_TITLE: &str = "Go: project";

#[derive(Debug, Clone, PartialEq, Eq)]
struct AddTaskDraftState {
    title: String,
    project: Option<String>,
    priority: String,
    labels: Vec<String>,
}

impl Default for AddTaskDraftState {
    fn default() -> Self {
        Self {
            title: String::new(),
            project: None,
            priority: "none".to_string(),
            labels: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum AuthoringFlow {
    AddTask(AddTaskDraftState),
    AddNote {
        task_id: String,
        display_ref: String,
    },
}

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
    authoring: Option<AuthoringFlow>,
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
            authoring: None,
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
            overlay => self.handle_generic_overlay_key(key, overlay).await?,
        }

        Ok(())
    }

    async fn handle_generic_overlay_key(
        &mut self,
        key: KeyEvent,
        overlay: OverlayState,
    ) -> Result<()> {
        let outcome = crate::tui::overlay::handle_generic_overlay_key(key, overlay);
        match outcome {
            OverlayOutcome::None(overlay) => self.overlay = Some(overlay),
            OverlayOutcome::Cancelled => self.cancel_authoring_overlay(),
            OverlayOutcome::Submitted(submit) => self.handle_overlay_submit(submit).await?,
        }
        Ok(())
    }

    async fn handle_overlay_submit(&mut self, submit: OverlaySubmit) -> Result<()> {
        match submit {
            OverlaySubmit::Text { title, value } if title == ADD_TASK_TITLE_TITLE => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    self.set_message("task title is required".to_string());
                    self.begin_add_task_title();
                } else if let Some(AuthoringFlow::AddTask(draft)) = self.authoring.as_mut() {
                    draft.title = trimmed.to_string();
                    self.begin_add_task_project();
                }
            }
            OverlaySubmit::Picker { title, values } if title == ADD_TASK_PROJECT_TITLE => {
                if let Some(AuthoringFlow::AddTask(draft)) = self.authoring.as_mut() {
                    draft.project = values.first().filter(|value| !value.is_empty()).cloned();
                    self.begin_add_task_priority();
                }
            }
            OverlaySubmit::Picker { title, values } if title == ADD_TASK_PRIORITY_TITLE => {
                if let Some(AuthoringFlow::AddTask(draft)) = self.authoring.as_mut() {
                    draft.priority = values
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "none".to_string());
                    self.begin_add_task_labels();
                }
            }
            OverlaySubmit::Picker { title, values } if title == ADD_TASK_LABELS_TITLE => {
                if let Some(AuthoringFlow::AddTask(draft)) = self.authoring.as_mut() {
                    draft.labels = values;
                    self.begin_add_task_description();
                }
            }
            OverlaySubmit::Multiline { title, value } if title == ADD_TASK_DESCRIPTION_TITLE => {
                self.submit_add_task(value).await?;
            }
            OverlaySubmit::Multiline { title, value } if title == ADD_NOTE_TITLE => {
                self.submit_add_note(value).await?;
            }
            OverlaySubmit::Text { title, value } if title == ADD_PROJECT_TITLE => {
                let message = self.store.create_project(value).await?;
                self.restore_selection_after_mutation();
                self.set_message(message);
            }
            OverlaySubmit::Text { title, value } if title == ADD_LABEL_TITLE => {
                let message = self.store.create_label(value).await?;
                self.set_message(message);
            }
            OverlaySubmit::Text { title, value } if title == EDIT_TITLE_TITLE => {
                self.submit_edit_title(value).await?;
            }
            OverlaySubmit::Multiline { title, value } if title == EDIT_DESCRIPTION_TITLE => {
                self.submit_edit_description(value).await?;
            }
            OverlaySubmit::Picker { title, values } if title == EDIT_PROJECT_TITLE => {
                if let Some(project) = values.first() {
                    self.submit_edit_project(project.clone()).await?;
                } else {
                    self.set_message("no matching project".to_string());
                    self.begin_edit_project();
                }
            }
            OverlaySubmit::Picker { title, values } if title == EDIT_PRIORITY_TITLE => {
                if let Some(priority) = values.first() {
                    self.submit_edit_priority(priority.clone()).await?;
                } else {
                    self.set_message("no matching priority".to_string());
                    self.begin_edit_priority();
                }
            }
            OverlaySubmit::Picker { title, values } if title == EDIT_LABELS_TITLE => {
                self.submit_edit_labels(values).await?;
            }
            OverlaySubmit::Picker { title, values } if title == FILTER_PROJECT_TITLE => {
                self.submit_filter_project(values).await?;
            }
            OverlaySubmit::Picker { title, values } if title == FILTER_LABEL_TITLE => {
                self.submit_filter_label(values).await?;
            }
            OverlaySubmit::Picker { title, values } if title == FILTER_STATUS_TITLE => {
                self.submit_filter_status(values).await?;
            }
            OverlaySubmit::Picker { title, values } if title == FILTER_PRIORITY_TITLE => {
                self.submit_filter_priority(values).await?;
            }
            OverlaySubmit::Picker { title, values } if title == VIEW_PROJECT_TITLE => {
                self.submit_view_project(values).await?;
            }
            other => self.set_message(other.message()),
        }
        Ok(())
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
            Action::SetPriority(priority) => self.set_exact_priority(priority).await?,
            Action::CyclePriority(reverse) => self.update_priority(reverse).await?,
            Action::BeginEditTitle => self.begin_edit_title(),
            Action::BeginEditDescription => self.begin_edit_description(),
            Action::BeginEditProject => self.begin_edit_project(),
            Action::BeginEditPriority => self.begin_edit_priority(),
            Action::BeginEditLabels => self.begin_edit_labels(),
            Action::Delete => self.update_deleted(true).await?,
            Action::Restore => self.update_deleted(false).await?,
            Action::BeginAddProject => self.begin_add_project(),
            Action::BeginAddLabel => self.begin_add_label(),
            Action::BeginAddTask => self.begin_add_task(),
            Action::BeginAddNote => self.begin_add_note(),
            Action::BeginFilterProject => self.begin_filter_project(),
            Action::BeginFilterLabel => self.begin_filter_label(),
            Action::BeginFilterStatus => self.begin_filter_status(),
            Action::BeginFilterPriority => self.begin_filter_priority(),
            Action::ClearFilters => self.clear_filters().await?,
            Action::ToggleDeletedFilter => self.toggle_deleted_filter().await?,
            Action::ShowView(target) => self.show_view(target).await?,
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

    fn begin_add_project(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::TextInput(TextInputState {
            title: ADD_PROJECT_TITLE.to_string(),
            prompt: "project name:".to_string(),
            input: String::new(),
            cursor: 0,
        }));
    }

    fn begin_add_label(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::TextInput(TextInputState {
            title: ADD_LABEL_TITLE.to_string(),
            prompt: "label name:".to_string(),
            input: String::new(),
            cursor: 0,
        }));
    }

    fn begin_add_task(&mut self) {
        self.pending_shortcut.clear();
        self.authoring = Some(AuthoringFlow::AddTask(AddTaskDraftState::default()));
        self.begin_add_task_title();
    }

    fn begin_add_task_title(&mut self) {
        let input = match &self.authoring {
            Some(AuthoringFlow::AddTask(draft)) => draft.title.clone(),
            _ => return,
        };
        let cursor = input.len();
        self.overlay = Some(OverlayState::TextInput(TextInputState {
            title: ADD_TASK_TITLE_TITLE.to_string(),
            prompt: "title:".to_string(),
            input,
            cursor,
        }));
    }

    fn begin_add_note(&mut self) {
        self.pending_shortcut.clear();
        let Some(item) = self
            .store
            .selected_task(self.widgets.table.selected())
            .cloned()
        else {
            self.set_message("no selected task for note".to_string());
            return;
        };
        self.authoring = Some(AuthoringFlow::AddNote {
            task_id: item.task.id.clone(),
            display_ref: item.display_ref.clone(),
        });
        self.overlay = Some(OverlayState::MultilineInput(MultilineInputState {
            title: ADD_NOTE_TITLE.to_string(),
            prompt: "note body:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        }));
    }

    fn begin_add_task_project(&mut self) {
        let selected = match &self.authoring {
            Some(AuthoringFlow::AddTask(draft)) => draft.project.as_deref(),
            _ => return,
        };
        let items = self.store.project_picker_items(selected);
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: ADD_TASK_PROJECT_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: false,
        }));
    }

    fn begin_add_task_priority(&mut self) {
        let selected = match &self.authoring {
            Some(AuthoringFlow::AddTask(draft)) => draft.priority.as_str(),
            _ => return,
        };
        let items = self.store.priority_picker_items(selected);
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: ADD_TASK_PRIORITY_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: false,
        }));
    }

    fn begin_add_task_labels(&mut self) {
        let selected_labels = match &self.authoring {
            Some(AuthoringFlow::AddTask(draft)) => draft.labels.clone(),
            _ => return,
        };
        let mut items = self.store.label_picker_items();
        for item in &mut items {
            item.selected = selected_labels.contains(&item.value);
        }
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: ADD_TASK_LABELS_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: true,
        }));
    }

    fn begin_add_task_description(&mut self) {
        self.overlay = Some(OverlayState::MultilineInput(MultilineInputState {
            title: ADD_TASK_DESCRIPTION_TITLE.to_string(),
            prompt: "description:".to_string(),
            lines: vec![String::new()],
            row: 0,
            column: 0,
        }));
    }

    async fn submit_add_task(&mut self, description: String) -> Result<()> {
        let Some(AuthoringFlow::AddTask(draft)) = self.authoring.take() else {
            return Ok(());
        };
        let current_selected = self.widgets.table.selected();
        let (message, selected) = self
            .store
            .create_task(
                TaskDraft {
                    title: draft.title,
                    description,
                    project: draft.project,
                    priority: draft.priority,
                    labels: draft.labels,
                },
                current_selected,
            )
            .await?;
        self.widgets.table.select(selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        if selected.is_none() {
            self.restore_selection_after_mutation();
        }
        self.set_message(message);
        Ok(())
    }

    async fn submit_add_note(&mut self, body: String) -> Result<()> {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            self.authoring = None;
            self.set_message("note body is required".to_string());
            return Ok(());
        }
        let Some(AuthoringFlow::AddNote {
            task_id,
            display_ref,
        }) = self.authoring.take()
        else {
            self.set_message("no selected task for note".to_string());
            return Ok(());
        };
        let note_id = self
            .store
            .add_note_to_task(&task_id, trimmed.to_string())
            .await?;
        self.set_message(format!("added note {note_id} to {display_ref}"));
        Ok(())
    }

    fn cancel_authoring_overlay(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = None;
        self.authoring = None;
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
        self.authoring = None;
        let had_overlay = self.overlay.take().is_some();
        if !had_overlay && self.focus == Focus::Sidebar {
            self.focus = Focus::Tasks;
            self.widgets.sidebar.select(self.store.sidebar_selection());
        }
    }

    async fn update_status(&mut self, status: &'static str) -> Result<()> {
        if let Some(result) = self
            .store
            .update_status(self.widgets.table.selected(), status)
            .await?
        {
            self.apply_mutation_result(result);
        }
        Ok(())
    }

    async fn set_exact_priority(&mut self, priority: &'static str) -> Result<()> {
        if let Some(result) = self
            .store
            .set_exact_priority(self.widgets.table.selected(), priority)
            .await?
        {
            self.apply_mutation_result(result);
        } else {
            self.set_message("no selected task to edit".to_string());
        }
        Ok(())
    }

    async fn update_priority(&mut self, reverse: bool) -> Result<()> {
        if let Some(result) = self
            .store
            .update_priority(self.widgets.table.selected(), reverse)
            .await?
        {
            self.apply_mutation_result(result);
        }
        Ok(())
    }

    async fn update_deleted(&mut self, deleted: bool) -> Result<()> {
        if let Some(result) = self
            .store
            .update_deleted(self.widgets.table.selected(), deleted)
            .await?
        {
            self.apply_mutation_result(result);
        }
        Ok(())
    }

    fn apply_mutation_result(&mut self, result: crate::tui::store::MutationMessage) {
        self.widgets.table.select(result.selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.set_message(result.message);
    }

    fn begin_edit_title(&mut self) {
        self.pending_shortcut.clear();
        let Some(item) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_message("no selected task to edit".to_string());
            return;
        };
        self.open_edit_title_overlay(item.task.title.clone());
    }

    fn open_edit_title_overlay(&mut self, input: String) {
        let cursor = input.len();
        self.overlay = Some(OverlayState::TextInput(TextInputState {
            title: EDIT_TITLE_TITLE.to_string(),
            prompt: "title:".to_string(),
            input,
            cursor,
        }));
    }

    fn begin_edit_description(&mut self) {
        self.pending_shortcut.clear();
        let Some(item) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_message("no selected task to edit".to_string());
            return;
        };
        let mut lines = item
            .task
            .description
            .split('\n')
            .map(str::to_string)
            .collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let row = lines.len() - 1;
        let column = lines[row].len();
        self.overlay = Some(OverlayState::MultilineInput(MultilineInputState {
            title: EDIT_DESCRIPTION_TITLE.to_string(),
            prompt: "description:".to_string(),
            lines,
            row,
            column,
        }));
    }

    fn begin_edit_project(&mut self) {
        self.pending_shortcut.clear();
        let Some(item) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_message("no selected task to edit".to_string());
            return;
        };
        let selected = item.task.project_key.as_str();
        let items = self.store.existing_project_picker_items(selected);
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: EDIT_PROJECT_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: false,
        }));
    }

    fn begin_edit_priority(&mut self) {
        self.pending_shortcut.clear();
        let Some(item) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_message("no selected task to edit".to_string());
            return;
        };
        let items = self
            .store
            .priority_picker_items(item.task.priority.as_str());
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: EDIT_PRIORITY_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: false,
        }));
    }

    fn begin_edit_labels(&mut self) {
        self.pending_shortcut.clear();
        let Some(item) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_message("no selected task to edit".to_string());
            return;
        };
        let mut items = self.store.label_picker_items();
        for picker_item in &mut items {
            picker_item.selected = item.labels.contains(&picker_item.value);
        }
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: EDIT_LABELS_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: true,
        }));
    }

    async fn submit_edit_title(&mut self, value: String) -> Result<()> {
        let trimmed = value.trim().to_string();
        if trimmed.is_empty() {
            self.set_message("task title is required".to_string());
            self.open_edit_title_overlay(value);
            return Ok(());
        }
        match self
            .store
            .update_title(self.widgets.table.selected(), trimmed)
            .await
        {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_message("no selected task to edit".to_string()),
            Err(error) => {
                self.set_message(format!("error: {error:#}"));
                self.open_edit_title_overlay(value);
            }
        }
        Ok(())
    }

    async fn submit_edit_description(&mut self, value: String) -> Result<()> {
        match self
            .store
            .update_description(self.widgets.table.selected(), value.clone())
            .await
        {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_message("no selected task to edit".to_string()),
            Err(error) => {
                self.set_message(format!("error: {error:#}"));
                let mut lines = value.split('\n').map(str::to_string).collect::<Vec<_>>();
                if lines.is_empty() {
                    lines.push(String::new());
                }
                let row = lines.len() - 1;
                let column = lines[row].len();
                self.overlay = Some(OverlayState::MultilineInput(MultilineInputState {
                    title: EDIT_DESCRIPTION_TITLE.to_string(),
                    prompt: "description:".to_string(),
                    lines,
                    row,
                    column,
                }));
            }
        }
        Ok(())
    }

    async fn submit_edit_project(&mut self, project: String) -> Result<()> {
        match self
            .store
            .update_project(self.widgets.table.selected(), project)
            .await
        {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_message("no selected task to edit".to_string()),
            Err(error) => {
                self.set_message(format!("error: {error:#}"));
                self.begin_edit_project();
            }
        }
        Ok(())
    }

    async fn submit_edit_priority(&mut self, priority: String) -> Result<()> {
        match self
            .store
            .set_exact_priority(self.widgets.table.selected(), &priority)
            .await
        {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_message("no selected task to edit".to_string()),
            Err(error) => {
                self.set_message(format!("error: {error:#}"));
                self.begin_edit_priority();
            }
        }
        Ok(())
    }

    async fn submit_edit_labels(&mut self, labels: Vec<String>) -> Result<()> {
        match self
            .store
            .update_labels(self.widgets.table.selected(), labels)
            .await
        {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_message("no selected task to edit".to_string()),
            Err(error) => {
                self.set_message(format!("error: {error:#}"));
                self.begin_edit_labels();
            }
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

    fn begin_filter_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = self.store.filters.project.as_deref().unwrap_or_default();
        let items = self.store.existing_project_picker_items(selected);
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: FILTER_PROJECT_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: false,
        }));
    }

    fn begin_filter_label(&mut self) {
        self.pending_shortcut.clear();
        let mut items = self.store.label_picker_items();
        for item in &mut items {
            item.selected = Some(&item.value) == self.store.filters.label.as_ref();
        }
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: FILTER_LABEL_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: false,
        }));
    }

    fn begin_filter_status(&mut self) {
        self.pending_shortcut.clear();
        let items = self
            .store
            .status_picker_items(self.store.filters.status.as_deref());
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: FILTER_STATUS_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: false,
        }));
    }

    fn begin_filter_priority(&mut self) {
        self.pending_shortcut.clear();
        let selected = self.store.filters.priority.as_deref().unwrap_or_default();
        let items = self.store.priority_picker_items(selected);
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: FILTER_PRIORITY_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: false,
        }));
    }

    fn begin_view_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = match &self.store.active_view {
            SidebarTarget::Project(project) => project.as_str(),
            _ => "",
        };
        let items = self.store.existing_project_picker_items(selected);
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: VIEW_PROJECT_TITLE.to_string(),
            filter: String::new(),
            selected: selected_picker_index(&items),
            items,
            multi: false,
        }));
    }

    async fn show_view(&mut self, target: ViewTarget) -> Result<()> {
        let sidebar_target = match target {
            ViewTarget::All => SidebarTarget::All,
            ViewTarget::Inbox => SidebarTarget::Inbox,
            ViewTarget::Active => SidebarTarget::Active,
            ViewTarget::Backlog => SidebarTarget::Backlog,
            ViewTarget::Todo => SidebarTarget::Todo,
            ViewTarget::Conflicts => SidebarTarget::Conflicts,
            ViewTarget::Project => {
                self.begin_view_project();
                return Ok(());
            }
        };
        let selected = self.store.show_view(sidebar_target).await?;
        self.apply_filter_selection(selected);
        self.set_message("view updated".to_string());
        Ok(())
    }

    fn apply_filter_selection(&mut self, selected: Option<usize>) {
        self.widgets.table.select(selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.focus = Focus::Tasks;
        self.overlay = None;
    }

    async fn clear_filters(&mut self) -> Result<()> {
        let selected = self.store.clear_filters().await?;
        self.apply_filter_selection(selected);
        self.set_message("filters cleared".to_string());
        Ok(())
    }

    async fn toggle_deleted_filter(&mut self) -> Result<()> {
        let selected = self.store.toggle_deleted_filter().await?;
        self.apply_filter_selection(selected);
        let message = if self.store.filters.include_deleted {
            "showing deleted tasks"
        } else {
            "hiding deleted tasks"
        };
        self.set_message(message.to_string());
        Ok(())
    }

    async fn submit_filter_project(&mut self, values: Vec<String>) -> Result<()> {
        let Some(project) = values.first().cloned() else {
            self.set_message("no matching project".to_string());
            self.begin_filter_project();
            return Ok(());
        };
        let selected = self.store.filter_project(project).await?;
        self.apply_filter_selection(selected);
        self.set_message("project filter applied".to_string());
        Ok(())
    }

    async fn submit_filter_label(&mut self, values: Vec<String>) -> Result<()> {
        let Some(label) = values.first().cloned() else {
            self.set_message("no matching label".to_string());
            self.begin_filter_label();
            return Ok(());
        };
        let selected = self.store.filter_label(label).await?;
        self.apply_filter_selection(selected);
        self.set_message("label filter applied".to_string());
        Ok(())
    }

    async fn submit_filter_status(&mut self, values: Vec<String>) -> Result<()> {
        let Some(status) = values.first().cloned() else {
            self.set_message("no matching status".to_string());
            self.begin_filter_status();
            return Ok(());
        };
        let selected = self.store.filter_status(status).await?;
        self.apply_filter_selection(selected);
        self.set_message("status filter applied".to_string());
        Ok(())
    }

    async fn submit_filter_priority(&mut self, values: Vec<String>) -> Result<()> {
        let Some(priority) = values.first().cloned() else {
            self.set_message("no matching priority".to_string());
            self.begin_filter_priority();
            return Ok(());
        };
        let selected = self.store.filter_priority(priority).await?;
        self.apply_filter_selection(selected);
        self.set_message("priority filter applied".to_string());
        Ok(())
    }

    async fn submit_view_project(&mut self, values: Vec<String>) -> Result<()> {
        let Some(project) = values.first().cloned() else {
            self.set_message("no matching project".to_string());
            self.begin_view_project();
            return Ok(());
        };
        let selected = self
            .store
            .show_view(SidebarTarget::Project(project))
            .await?;
        self.apply_filter_selection(selected);
        self.set_message("project view selected".to_string());
        Ok(())
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

fn selected_picker_index(items: &[PickerItem]) -> usize {
    items.iter().position(|item| item.selected).unwrap_or(0)
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

    fn ctrl_s() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
    }

    async fn type_chars(app: &mut App, input: &str) {
        for ch in input.chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
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
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('q')).await.unwrap();
        assert_eq!(
            app.message.as_deref(),
            Some(":order-queue is not yet implemented")
        );
    }

    #[tokio::test]
    async fn filter_project_shortcut_opens_project_picker() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, .. })) if title == FILTER_PROJECT_TITLE
        ));
    }

    #[tokio::test]
    async fn clear_filters_shortcut_resets_default_view() {
        let mut app = test_app().await;
        app.store.filters.status = Some("todo".to_string());
        app.store.active_view = SidebarTarget::Todo;

        app.handle_normal_key(KeyCode::Char('f')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

        assert_eq!(app.store.active_view, SidebarTarget::All);
        assert!(app.store.filters.status.is_none());
        assert_eq!(app.message.as_deref(), Some("filters cleared"));
    }

    #[tokio::test]
    async fn go_conflicts_shortcut_sets_conflicts_view() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

        assert_eq!(app.store.active_view, SidebarTarget::Conflicts);
        assert!(app.store.filters.conflicts_only);
    }

    #[tokio::test]
    async fn add_task_shortcut_opens_title_prompt() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if state.prompt == "title:"
        ));
    }

    #[tokio::test]
    async fn add_task_flow_creates_task_with_metadata_and_selects_it() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.store
            .create_label("Needs Review".to_string())
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.title == ADD_TASK_PROJECT_TITLE
        ));
        type_chars(&mut app, "mobile").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.title == ADD_TASK_PRIORITY_TITLE
        ));
        type_chars(&mut app, "high").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.title == ADD_TASK_LABELS_TITLE
        ));
        type_chars(&mut app, "needs").await;
        app.handle_overlay_key(key(KeyCode::Char(' ')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state)) if state.title == ADD_TASK_DESCRIPTION_TITLE
        ));
        type_chars(&mut app, "Long description").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.project_key, "mobile-app");
        assert_eq!(task.task.priority, "high");
        assert_eq!(task.task.description, "Long description");
        assert!(task.labels.iter().any(|label| label == "needs-review"));
    }

    #[tokio::test]
    async fn add_task_flow_cancels_at_title_step() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn add_task_blank_title_is_rejected() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(app.message.as_deref(), Some("task title is required"));
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if state.title == ADD_TASK_TITLE_TITLE
        ));
    }

    #[tokio::test]
    async fn add_note_requires_selected_task() {
        let mut app = test_app().await;
        app.widgets.table.select(None);
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("no selected task for note"));
    }

    #[tokio::test]
    async fn add_note_flow_creates_note_for_selected_task() {
        let mut app = test_app().await;
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Note target".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        app.widgets.table.select(selected);

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state)) if state.title == ADD_NOTE_TITLE
        ));

        type_chars(&mut app, "Important detail").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        assert!(
            app.message
                .as_deref()
                .is_some_and(|message| message.starts_with("added note "))
        );
    }

    #[tokio::test]
    async fn add_note_blank_body_is_rejected() {
        let mut app = test_app().await;
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Note target".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        app.widgets.table.select(selected);

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("note body is required"));
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
    async fn add_project_shortcut_opens_prompt_and_creates_project() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if state.prompt == "project name:"
        ));

        for ch in "Mobile App".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("created project mobile-app"));
        assert!(
            app.store
                .projects
                .iter()
                .any(|project| project.key == "mobile-app")
        );
        assert!(
            app.store
                .sidebar_entries
                .iter()
                .any(|entry| entry.label.contains("Mobile App"))
        );
    }

    #[tokio::test]
    async fn add_label_shortcut_opens_prompt_and_creates_label() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if state.prompt == "label name:"
        ));

        for ch in "Needs Review".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("created label needs-review"));
        assert!(app.store.labels.iter().any(|label| label == "needs-review"));
        assert!(
            app.store
                .label_picker_items()
                .iter()
                .any(|item| item.value == "needs-review")
        );
    }

    #[tokio::test]
    async fn edit_title_shortcut_prefills_and_updates_title() {
        let mut app = test_app().await;
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Old title".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        app.widgets.table.select(selected);

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state))
                if state.title == EDIT_TITLE_TITLE && state.input == "Old title"
        ));

        app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
        type_chars(&mut app, " updated").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "Old title updated");
    }

    #[tokio::test]
    async fn edit_description_prefills_and_ctrl_s_updates() {
        let mut app = test_app().await;
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Description target".to_string(),
                    description: "first\nsecond".to_string(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        app.widgets.table.select(selected);

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if state.title == EDIT_DESCRIPTION_TITLE
                    && state.lines == vec!["first".to_string(), "second".to_string()]
        ));

        app.handle_overlay_key(key(KeyCode::End)).await.unwrap();
        type_chars(&mut app, " updated").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(
            app.store.tasks[selected].task.description,
            "first\nsecond updated"
        );
    }

    #[tokio::test]
    async fn edit_project_picker_uses_existing_projects_only() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Project target".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        app.widgets.table.select(selected);

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state))
                if state.title == EDIT_PROJECT_TITLE
                    && !state.items.iter().any(|item| item.label == "Infer project")
        ));

        type_chars(&mut app, "mobile").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.project_key, "mobile-app");
    }

    #[tokio::test]
    async fn edit_priority_picker_prefills_current_priority() {
        let mut app = test_app().await;
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Priority target".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "high".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        app.widgets.table.select(selected);

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state))
                if state.title == EDIT_PRIORITY_TITLE
                    && state.items.iter().any(|item| item.value == "high" && item.selected)
        ));

        type_chars(&mut app, "urgent").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.priority, "urgent");
    }

    #[tokio::test]
    async fn edit_labels_picker_prefills_current_labels_and_removes_unselected() {
        let mut app = test_app().await;
        app.store.create_label("Bug".to_string()).await.unwrap();
        app.store.create_label("Docs".to_string()).await.unwrap();
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Label target".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: vec!["bug".to_string()],
                },
                None,
            )
            .await
            .unwrap();
        app.widgets.table.select(selected);

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state))
                if state.title == EDIT_LABELS_TITLE
                    && state.items.iter().any(|item| item.value == "bug" && item.selected)
        ));

        type_chars(&mut app, "bug").await;
        app.handle_overlay_key(key(KeyCode::Char(' ')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Backspace))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Backspace))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Backspace))
            .await
            .unwrap();
        type_chars(&mut app, "docs").await;
        app.handle_overlay_key(key(KeyCode::Char(' ')))
            .await
            .unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].labels, vec!["docs".to_string()]);
    }

    #[tokio::test]
    async fn exact_priority_shortcut_updates_selected_task() {
        let mut app = test_app().await;
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Priority shortcut".to_string(),
                    description: String::new(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        app.widgets.table.select(selected);

        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.priority, "urgent");
    }

    #[tokio::test]
    async fn edit_shortcuts_require_selected_task() {
        let mut app = test_app().await;
        app.widgets.table.select(None);

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("no selected task to edit"));
    }

    #[tokio::test]
    async fn edit_description_conflict_preserves_overlay() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        let mut app = App::new(pool.clone()).await.unwrap();
        let (_, selected) = app
            .store
            .create_task(
                TaskDraft {
                    title: "Conflict target".to_string(),
                    description: "old".to_string(),
                    project: None,
                    priority: "none".to_string(),
                    labels: Vec::new(),
                },
                None,
            )
            .await
            .unwrap();
        app.widgets.table.select(selected);
        let task_id = app.store.tasks[selected.unwrap()].task.id.clone();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
             local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
             VALUES (?, 'description', NULL, 'local', 'remote', NULL, ?, 'a', 'b', ?, 0)",
        )
        .bind(&task_id)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        type_chars(&mut app, " updated").await;
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(
            app.message
                .as_deref()
                .is_some_and(|message| message.contains("conflicted-field"))
        );
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::MultilineInput(state))
                if state.lines.join("\n") == "old updated"
        ));
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
