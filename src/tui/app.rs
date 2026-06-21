use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::DefaultTerminal;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;

use crate::operations::TaskDraft;
use crate::query::TaskSort;
use crate::tui::event::{
    Action, CommandLookup, ShortcutLookup, ViewTarget, lookup_command, resolve_shortcut,
    shortcut_label,
};
use crate::tui::overlay::{
    ConfirmState, LineEdit, MultilineInputState, OverlayOutcome, OverlayState, OverlaySubmit,
    OverlayView, PickerItem, PickerState, TextInputState, TextPanelState,
};
use crate::tui::store::{ConflictTarget, SidebarEntry, SidebarTarget, TuiStore};
use crate::tui::ui::{self, help_scroll_cap};

const ADD_PROJECT_TITLE: &str = "Add project";
const DELETE_PROJECT_TITLE: &str = "Delete project";
const ADD_LABEL_TITLE: &str = "Add label";
const ADD_TASK_TITLE_TITLE: &str = "Add task";
const ADD_TASK_TITLE_PROJECT_TITLE: &str = "Add task: title project";
const ADD_TASK_TITLE_PRIORITY_TITLE: &str = "Add task: title priority";
const ADD_NOTE_TITLE: &str = "Add note";
const EDIT_STATUS_TITLE: &str = "Edit task: status";
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
const SWITCH_WORKSPACE_TITLE: &str = "Switch workspace";
const CONFLICT_FIELD_TITLE: &str = "Conflict: field";
const CONFLICT_CONFIRM_LOCAL_TITLE: &str = "Resolve conflict: local";
const CONFLICT_CONFIRM_REMOTE_TITLE: &str = "Resolve conflict: remote";
const CONFLICT_MANUAL_TITLE: &str = "Resolve conflict: manual";
const CONFLICT_DETAILS_TITLE: &str = "Conflict details";
const CONFIG_STATUS_TITLE: &str = "Config status";
const CONFIG_INFO_TITLE: &str = "Configuration";
const CONFIG_PATHS_TITLE: &str = "Config paths";
const CONFIG_INIT_TITLE: &str = "Initialize configuration";

#[derive(Debug, Clone, PartialEq, Eq)]
struct AddTaskDraftState {
    title: String,
    project: Option<String>,
    inferred_project: Option<String>,
    priority: String,
}

impl Default for AddTaskDraftState {
    fn default() -> Self {
        Self {
            title: String::new(),
            project: None,
            inferred_project: None,
            priority: "none".to_string(),
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
enum ConflictResolutionChoice {
    Local,
    Remote,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskRefKind {
    Short,
    Durable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConflictFlow {
    PickVariant {
        choice: ConflictResolutionChoice,
        targets: Vec<ConflictTarget>,
    },
    ConfirmVariant {
        choice: ConflictResolutionChoice,
        target: ConflictTarget,
    },
    PickManual {
        targets: Vec<ConflictTarget>,
    },
    EditManual {
        target: ConflictTarget,
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
    conflict_flow: Option<ConflictFlow>,
    pending_delete_project: Option<String>,
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
            conflict_flow: None,
            pending_delete_project: None,
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
                let result = self.dispatch_key(key, terminal.size()?.height).await;
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

    async fn dispatch_key(&mut self, key: KeyEvent, terminal_height: u16) -> Result<()> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.handle(Action::Quit).await
        } else if key.code == KeyCode::Esc && !self.pending_shortcut.is_empty() {
            self.handle_normal_key(key.code).await
        } else if self.overlay_captures_input() {
            self.handle_overlay_key_at_height(key, terminal_height)
                .await
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
        if self.overlay_captures_input()
            && (code != KeyCode::Esc || self.pending_shortcut.is_empty())
        {
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
        self.handle_overlay_key_at_height(key, 24).await
    }

    async fn handle_overlay_key_at_height(
        &mut self,
        key: KeyEvent,
        terminal_height: u16,
    ) -> Result<()> {
        let Some(overlay) = self.overlay.take() else {
            return Ok(());
        };

        match overlay {
            OverlayState::Search { mut input } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => self.accept_search_input(input.text).await?,
                _ => {
                    input.handle_key(key);
                    self.overlay = Some(OverlayState::Search { input });
                }
            },
            OverlayState::Command { mut input } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Some(action) = self.accept_command_input(input.as_str()) {
                        self.execute(action).await?;
                    } else {
                        self.overlay = Some(OverlayState::Command { input });
                    }
                }
                _ => {
                    input.handle_key(key);
                    self.overlay = Some(OverlayState::Command { input });
                }
            },
            overlay => {
                self.handle_generic_overlay_key(key, overlay, terminal_height)
                    .await?
            }
        }

        Ok(())
    }

    async fn handle_generic_overlay_key(
        &mut self,
        key: KeyEvent,
        overlay: OverlayState,
        terminal_height: u16,
    ) -> Result<()> {
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('p')
            && let OverlayState::TextInput(state) = &overlay
            && add_task_title_overlay(&state.title)
        {
            if let Some(AuthoringFlow::AddTask(draft)) = self.authoring.as_mut() {
                draft.title = state.input.text.clone();
                self.begin_add_task_title_priority();
            }
            return Ok(());
        }

        if key.code == KeyCode::Tab
            && let OverlayState::TextInput(state) = &overlay
            && add_task_title_overlay(&state.title)
        {
            if let Some(AuthoringFlow::AddTask(draft)) = self.authoring.as_mut() {
                draft.title = state.input.text.clone();
                self.begin_add_task_title_project();
            }
            return Ok(());
        }

        let scroll_cap = help_scroll_cap(terminal_height);
        let outcome = crate::tui::overlay::handle_generic_overlay_key(key, overlay, scroll_cap);
        match outcome {
            OverlayOutcome::None(overlay) => self.overlay = Some(overlay),
            OverlayOutcome::Cancelled => self.cancel_authoring_overlay(),
            OverlayOutcome::Submitted(submit) => self.handle_overlay_submit(submit).await?,
        }
        Ok(())
    }

    async fn handle_overlay_submit(&mut self, submit: OverlaySubmit) -> Result<()> {
        match submit {
            OverlaySubmit::Text { title, value } if add_task_title_overlay(&title) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    self.set_message("task title is required".to_string());
                    self.begin_add_task_title();
                } else {
                    self.submit_add_task_with_title(trimmed.to_string()).await?;
                }
            }
            OverlaySubmit::Picker { title, values } if title == ADD_TASK_TITLE_PROJECT_TITLE => {
                if let Some(AuthoringFlow::AddTask(draft)) = self.authoring.as_mut() {
                    draft.project = values.first().filter(|value| !value.is_empty()).cloned();
                    self.begin_add_task_title();
                }
            }
            OverlaySubmit::Picker { title, values } if title == ADD_TASK_TITLE_PRIORITY_TITLE => {
                if let Some(AuthoringFlow::AddTask(draft)) = self.authoring.as_mut() {
                    draft.priority = values
                        .first()
                        .cloned()
                        .unwrap_or_else(|| "none".to_string());
                    self.begin_add_task_title();
                }
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
            OverlaySubmit::Picker { title, values } if title == EDIT_STATUS_TITLE => {
                if let Some(status) = values.first() {
                    self.submit_edit_status(status.clone()).await?;
                } else {
                    self.set_message("no matching status".to_string());
                    self.begin_status_picker();
                }
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
            OverlaySubmit::Picker { title, values } if title == DELETE_PROJECT_TITLE => {
                self.submit_delete_project_picker(values);
            }
            OverlaySubmit::Picker { title, values } if title == SWITCH_WORKSPACE_TITLE => {
                self.submit_switch_workspace(values).await?;
            }
            OverlaySubmit::Picker { title, values } if title == CONFLICT_FIELD_TITLE => {
                self.submit_conflict_field_picker(values).await?;
            }
            OverlaySubmit::Confirm { title }
                if title == CONFLICT_CONFIRM_LOCAL_TITLE
                    || title == CONFLICT_CONFIRM_REMOTE_TITLE =>
            {
                self.submit_confirmed_conflict_resolution().await?;
            }
            OverlaySubmit::Confirm { title } if title == CONFIG_INIT_TITLE => {
                self.submit_config_init()?;
            }
            OverlaySubmit::Confirm { title } if title == DELETE_PROJECT_TITLE => {
                self.submit_delete_project().await?;
            }
            OverlaySubmit::Text { title, value } if title == CONFLICT_MANUAL_TITLE => {
                self.submit_manual_conflict_value(value).await?;
            }
            OverlaySubmit::Multiline { title, value } if title == CONFLICT_MANUAL_TITLE => {
                self.submit_manual_conflict_value(value).await?;
            }
            OverlaySubmit::Picker { title, values } if title == CONFLICT_MANUAL_TITLE => {
                if let Some(value) = values.first() {
                    self.submit_manual_conflict_value(value.clone()).await?;
                } else {
                    self.set_message("no value selected".to_string());
                }
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
            Action::MoveLeft => self.move_left(),
            Action::MoveRight => self.move_right(),
            Action::PreviousItem => self.previous_item(),
            Action::NextItem => self.next_item(),
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
                self.set_message(format!(
                    "order {} {}",
                    self.store.sort_label(),
                    self.store.sort_direction_label()
                ));
            }
            Action::SetSort(sort) => self.set_sort(sort).await?,
            Action::ReverseSort => self.reverse_sort().await?,
            Action::SetStatus(status) => self.update_status(status).await?,
            Action::SetPriority(priority) => self.set_exact_priority(priority).await?,
            Action::CyclePriority(reverse) => self.update_priority(reverse).await?,
            Action::CopyShortRef => self.copy_selected_ref(TaskRefKind::Short),
            Action::CopyDurableRef => self.copy_selected_ref(TaskRefKind::Durable),
            Action::BeginEditTitle => self.begin_edit_title(),
            Action::BeginEditDescription => self.begin_edit_description(),
            Action::BeginEditProject => self.begin_edit_project(),
            Action::BeginEditPriority => self.begin_edit_priority(),
            Action::BeginEditLabels => self.begin_edit_labels(),
            Action::Delete => self.update_deleted(true).await?,
            Action::Restore => self.update_deleted(false).await?,
            Action::BeginStatusPicker => self.begin_status_picker(),
            Action::BeginDeleteProject => self.begin_delete_project(),
            Action::BeginAddProject => self.begin_add_project(),
            Action::BeginAddLabel => self.begin_add_label(),
            Action::BeginAddTask => self.begin_add_task().await?,
            Action::BeginAddNote => self.begin_add_note(),
            Action::BeginFilterProject => self.begin_filter_project(),
            Action::BeginFilterLabel => self.begin_filter_label(),
            Action::BeginFilterStatus => self.begin_filter_status(),
            Action::BeginFilterPriority => self.begin_filter_priority(),
            Action::BeginSwitchWorkspace => self.begin_switch_workspace().await?,
            Action::ClearFilters => self.clear_filters().await?,
            Action::ToggleDeletedFilter => self.toggle_deleted_filter().await?,
            Action::ShowView(target) => self.show_view(target).await?,
            Action::BeginConflictList => self.open_conflict_list().await?,
            Action::ShowConflictDetails => self.show_conflict_details().await?,
            Action::NextConflict => self.move_to_conflict(1),
            Action::PreviousConflict => self.move_to_conflict(-1),
            Action::AcceptConflictLocal => {
                self.begin_conflict_resolution(ConflictResolutionChoice::Local)
                    .await?
            }
            Action::AcceptConflictRemote => {
                self.begin_conflict_resolution(ConflictResolutionChoice::Remote)
                    .await?
            }
            Action::BeginManualConflictMerge => self.begin_manual_conflict_merge().await?,
            Action::ShowConfigStatus => self.show_config_status()?,
            Action::ShowConfigInfo => self.show_config_info()?,
            Action::ShowConfigPaths => self.show_config_paths()?,
            Action::BeginConfigInit => self.begin_config_init()?,
            Action::Undo => self.undo_last().await?,
            Action::Planned { name, reason } => {
                self.set_message(format!(":{name} is not yet implemented: {reason}"));
            }
            Action::Disabled { name, reason } => {
                self.set_message(format!(":{name} is disabled: {reason}"));
            }
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

    fn move_left(&mut self) {
        self.focus = Focus::Sidebar;
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.overlay = None;
    }

    fn move_right(&mut self) {
        self.focus = Focus::Tasks;
        self.overlay = None;
    }

    fn previous_item(&mut self) {
        if matches!(self.store.active_view, SidebarTarget::Conflicts)
            || self.store.filters.conflicts_only
        {
            self.move_to_conflict(-1);
        } else {
            self.set_message("previous item is available in conflict flows".to_string());
        }
    }

    fn next_item(&mut self) {
        if matches!(self.store.active_view, SidebarTarget::Conflicts)
            || self.store.filters.conflicts_only
        {
            self.move_to_conflict(1);
        } else {
            self.set_message("next item is available in conflict flows".to_string());
        }
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
            input: LineEdit::new(self.store.filters.search.clone().unwrap_or_default()),
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
            input: LineEdit::blank(),
        });
    }

    fn begin_add_project(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::TextInput(TextInputState::blank(
            ADD_PROJECT_TITLE,
            "project name:",
        )));
    }

    fn begin_add_label(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::TextInput(TextInputState::blank(
            ADD_LABEL_TITLE,
            "label name:",
        )));
    }

    async fn begin_add_task(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        let active_project = match &self.store.active_view {
            SidebarTarget::Project(project) => Some(project.clone()),
            _ => None,
        };
        let inferred_project = if active_project.is_none() {
            self.store.inferred_add_project().await?
        } else {
            None
        };
        self.authoring = Some(AuthoringFlow::AddTask(AddTaskDraftState {
            project: active_project,
            inferred_project,
            ..AddTaskDraftState::default()
        }));
        self.begin_add_task_title();
        Ok(())
    }

    fn begin_add_task_title(&mut self) {
        let (input, project, priority) = match &self.authoring {
            Some(AuthoringFlow::AddTask(draft)) => {
                let project = draft
                    .project
                    .as_deref()
                    .or(draft.inferred_project.as_deref())
                    .unwrap_or("no project");
                (
                    draft.title.clone(),
                    project.to_string(),
                    draft.priority.clone(),
                )
            }
            _ => return,
        };
        self.overlay = Some(OverlayState::TextInput(TextInputState::new(
            format!("Add task  project={project} priority={priority}"),
            "",
            input,
        )));
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
        self.overlay = Some(OverlayState::MultilineInput(MultilineInputState::blank(
            ADD_NOTE_TITLE,
            "note body:",
        )));
    }

    fn begin_add_task_title_project(&mut self) {
        let selected = match &self.authoring {
            Some(AuthoringFlow::AddTask(draft)) => draft.project.as_deref(),
            _ => return,
        };
        let items = self.store.project_picker_items(selected);
        self.open_picker_overlay(ADD_TASK_TITLE_PROJECT_TITLE, items, false);
    }

    fn begin_add_task_title_priority(&mut self) {
        let selected = match &self.authoring {
            Some(AuthoringFlow::AddTask(draft)) => draft.priority.as_str(),
            _ => return,
        };
        let items = self.store.priority_picker_items(selected);
        self.open_picker_overlay(ADD_TASK_TITLE_PRIORITY_TITLE, items, false);
    }

    async fn submit_add_task_with_title(&mut self, title: String) -> Result<()> {
        if let Some(AuthoringFlow::AddTask(draft)) = self.authoring.as_mut() {
            draft.title = title;
        }
        self.submit_add_task().await
    }

    async fn submit_add_task(&mut self) -> Result<()> {
        let Some(AuthoringFlow::AddTask(draft)) = self.authoring.take() else {
            return Ok(());
        };
        let current_selected = self.widgets.table.selected();
        let (message, selected) = self
            .store
            .create_task(
                TaskDraft {
                    title: draft.title,
                    description: String::new(),
                    project: draft.project,
                    priority: draft.priority,
                    labels: Vec::new(),
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
        self.conflict_flow = None;
        self.pending_delete_project = None;
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
        if matches!(self.overlay, Some(OverlayState::Help { .. })) {
            self.overlay = None;
        } else {
            self.overlay = Some(OverlayState::Help { scroll: 0 });
        }
    }

    fn cancel_overlay(&mut self) {
        self.pending_shortcut.clear();
        self.authoring = None;
        self.conflict_flow = None;
        self.pending_delete_project = None;
        let had_overlay = self.overlay.take().is_some();
        if !had_overlay && self.focus == Focus::Sidebar {
            self.focus = Focus::Tasks;
            self.widgets.sidebar.select(self.store.sidebar_selection());
        }
    }

    async fn set_sort(&mut self, sort: TaskSort) -> Result<()> {
        let selected = self.store.set_sort(sort).await?;
        self.apply_filter_selection(selected);
        self.set_message(format!(
            "order {} {}",
            self.store.sort_label(),
            self.store.sort_direction_label()
        ));
        Ok(())
    }

    async fn reverse_sort(&mut self) -> Result<()> {
        let selected = self.store.reverse_sort().await?;
        self.apply_filter_selection(selected);
        self.set_message(format!(
            "order {} {}",
            self.store.sort_label(),
            self.store.sort_direction_label()
        ));
        Ok(())
    }

    async fn update_status(&mut self, status: &'static str) -> Result<()> {
        if let Some(result) = self
            .store
            .update_status(self.widgets.table.selected(), status)
            .await?
        {
            self.apply_mutation_result(result);
        } else {
            self.set_message("no selected task to edit".to_string());
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
        } else {
            self.set_message("no selected task to edit".to_string());
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
        } else {
            self.set_message("no selected task to edit".to_string());
        }
        Ok(())
    }

    async fn undo_last(&mut self) -> Result<()> {
        match self.store.undo_last().await? {
            Some(result) => self.apply_mutation_result(result),
            None => self.set_message("nothing to undo".to_string()),
        }
        Ok(())
    }

    fn apply_mutation_result(&mut self, result: crate::tui::store::MutationMessage) {
        self.widgets.table.select(result.selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.set_message(result.message);
    }

    fn open_picker_overlay(&mut self, title: &str, items: Vec<PickerItem>, multi: bool) {
        self.overlay = Some(OverlayState::Picker(PickerState {
            title: title.to_string(),
            filter: LineEdit::blank(),
            selected: selected_picker_index(&items),
            items,
            multi,
        }));
    }

    fn guard_selected_task(&mut self) -> Option<usize> {
        self.pending_shortcut.clear();
        let index = self.widgets.table.selected();
        if index.is_some_and(|i| self.store.selected_task(Some(i)).is_some()) {
            index
        } else {
            self.set_message("no selected task to edit".to_string());
            None
        }
    }

    fn require_picker_value(&mut self, values: Vec<String>, message: &str) -> Option<String> {
        match values.first().cloned() {
            Some(value) => Some(value),
            None => {
                self.set_message(message.to_string());
                None
            }
        }
    }

    fn filter_value_or_reopen(
        &mut self,
        values: Vec<String>,
        empty_message: &str,
        reopen: fn(&mut Self),
    ) -> Option<String> {
        let Some(value) = self.require_picker_value(values, empty_message) else {
            reopen(self);
            return None;
        };
        Some(value)
    }

    fn apply_edit_mutation<F>(
        &mut self,
        result: Result<Option<crate::tui::store::MutationMessage>>,
        on_error: F,
    ) where
        F: FnOnce(&mut Self),
    {
        match result {
            Ok(Some(result)) => self.apply_mutation_result(result),
            Ok(None) => self.set_message("no selected task to edit".to_string()),
            Err(error) => {
                self.set_message(format!("error: {error:#}"));
                on_error(self);
            }
        }
    }

    fn begin_status_picker(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let selected = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .status
            .as_str();
        let items = self.store.status_picker_items(Some(selected));
        self.open_picker_overlay(EDIT_STATUS_TITLE, items, false);
    }

    fn begin_delete_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = if self.focus == Focus::Sidebar {
            self.selected_sidebar_project()
        } else {
            None
        };
        let items = self.store.existing_project_picker_items(selected.as_deref().unwrap_or_default());
        self.open_picker_overlay(DELETE_PROJECT_TITLE, items, false);
    }

    fn selected_sidebar_project(&self) -> Option<String> {
        self.widgets
            .sidebar
            .selected()
            .and_then(|index| self.store.sidebar_entries.get(index))
            .and_then(|entry| entry.target.as_ref())
            .and_then(|target| match target {
                SidebarTarget::Project(project) => Some(project.clone()),
                _ => None,
            })
    }

    fn begin_edit_title(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let title = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .title
            .clone();
        self.open_edit_title_overlay(title);
    }

    fn open_edit_title_overlay(&mut self, input: String) {
        self.overlay = Some(OverlayState::TextInput(TextInputState::new(
            EDIT_TITLE_TITLE,
            "title:",
            input,
        )));
    }

    fn open_edit_description_overlay(&mut self, value: String) {
        self.overlay = Some(OverlayState::MultilineInput(
            MultilineInputState::from_value(EDIT_DESCRIPTION_TITLE, "description:", value),
        ));
    }

    fn begin_edit_description(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let description = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .description
            .clone();
        self.open_edit_description_overlay(description);
    }

    fn begin_edit_project(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let selected = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .project_key
            .as_str();
        let items = self.store.existing_project_picker_items(selected);
        self.open_picker_overlay(EDIT_PROJECT_TITLE, items, false);
    }

    fn begin_edit_priority(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let priority = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .task
            .priority
            .as_str();
        let items = self.store.priority_picker_items(priority);
        self.open_picker_overlay(EDIT_PRIORITY_TITLE, items, false);
    }

    fn begin_edit_labels(&mut self) {
        let Some(index) = self.guard_selected_task() else {
            return;
        };
        let labels = self
            .store
            .selected_task(Some(index))
            .unwrap()
            .labels
            .clone();
        let mut items = self.store.label_picker_items();
        for picker_item in &mut items {
            picker_item.selected = labels.contains(&picker_item.value);
        }
        self.open_picker_overlay(EDIT_LABELS_TITLE, items, true);
    }

    fn copy_selected_ref(&mut self, kind: TaskRefKind) {
        let Some(task) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_message("no selected task to copy".to_string());
            return;
        };
        let (value, message_ref) = match kind {
            TaskRefKind::Short => (task.display_ref.clone(), task.display_ref.clone()),
            TaskRefKind::Durable => (task.task.id.clone(), task.display_ref.clone()),
        };
        match copy_to_clipboard(&value) {
            Ok(()) => self.set_message(format!("copied {message_ref}")),
            Err(error) => self.set_message(format!("copy failed: {error}")),
        }
    }

    async fn submit_edit_status(&mut self, status: String) -> Result<()> {
        let result = self
            .store
            .update_status(self.widgets.table.selected(), &status)
            .await;
        self.apply_edit_mutation(result, |app| app.begin_status_picker());
        Ok(())
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
        let result = self
            .store
            .update_description(self.widgets.table.selected(), value.clone())
            .await;
        self.apply_edit_mutation(result, |app| app.open_edit_description_overlay(value));
        Ok(())
    }

    async fn submit_edit_project(&mut self, project: String) -> Result<()> {
        let result = self
            .store
            .update_project(self.widgets.table.selected(), project)
            .await;
        self.apply_edit_mutation(result, |app| app.begin_edit_project());
        Ok(())
    }

    async fn submit_edit_priority(&mut self, priority: String) -> Result<()> {
        let result = self
            .store
            .set_exact_priority(self.widgets.table.selected(), &priority)
            .await;
        self.apply_edit_mutation(result, |app| app.begin_edit_priority());
        Ok(())
    }

    async fn submit_edit_labels(&mut self, labels: Vec<String>) -> Result<()> {
        let result = self
            .store
            .update_labels(self.widgets.table.selected(), labels)
            .await;
        self.apply_edit_mutation(result, |app| app.begin_edit_labels());
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
        self.open_picker_overlay(FILTER_PROJECT_TITLE, items, false);
    }

    fn begin_filter_label(&mut self) {
        self.pending_shortcut.clear();
        let mut items = self.store.label_picker_items();
        for item in &mut items {
            item.selected = Some(&item.value) == self.store.filters.label.as_ref();
        }
        self.open_picker_overlay(FILTER_LABEL_TITLE, items, false);
    }

    fn begin_filter_status(&mut self) {
        self.pending_shortcut.clear();
        let items = self
            .store
            .status_picker_items(self.store.filters.status.as_deref());
        self.open_picker_overlay(FILTER_STATUS_TITLE, items, false);
    }

    fn begin_filter_priority(&mut self) {
        self.pending_shortcut.clear();
        let selected = self.store.filters.priority.as_deref().unwrap_or_default();
        let items = self.store.priority_picker_items(selected);
        self.open_picker_overlay(FILTER_PRIORITY_TITLE, items, false);
    }

    async fn begin_switch_workspace(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.store.refresh(None).await?;
        let items = self.store.workspace_picker_items();
        self.open_picker_overlay(SWITCH_WORKSPACE_TITLE, items, false);
        Ok(())
    }

    fn begin_view_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = match &self.store.active_view {
            SidebarTarget::Project(project) => project.as_str(),
            _ => "",
        };
        let items = self.store.existing_project_picker_items(selected);
        self.open_picker_overlay(VIEW_PROJECT_TITLE, items, false);
    }

    async fn show_view(&mut self, target: ViewTarget) -> Result<()> {
        let sidebar_target = match target {
            ViewTarget::All => SidebarTarget::All,
            ViewTarget::Inbox => SidebarTarget::Inbox,
            ViewTarget::Active => SidebarTarget::Active,
            ViewTarget::Backlog => SidebarTarget::Backlog,
            ViewTarget::Todo => SidebarTarget::Todo,
            ViewTarget::Done => SidebarTarget::Done,
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
        let Some(project) =
            self.filter_value_or_reopen(values, "no matching project", Self::begin_filter_project)
        else {
            return Ok(());
        };
        let selected = self.store.filter_project(project).await?;
        self.apply_filter_selection(selected);
        self.set_message("project filter applied".to_string());
        Ok(())
    }

    async fn submit_filter_label(&mut self, values: Vec<String>) -> Result<()> {
        let Some(label) =
            self.filter_value_or_reopen(values, "no matching label", Self::begin_filter_label)
        else {
            return Ok(());
        };
        let selected = self.store.filter_label(label).await?;
        self.apply_filter_selection(selected);
        self.set_message("label filter applied".to_string());
        Ok(())
    }

    async fn submit_filter_status(&mut self, values: Vec<String>) -> Result<()> {
        let Some(status) =
            self.filter_value_or_reopen(values, "no matching status", Self::begin_filter_status)
        else {
            return Ok(());
        };
        let selected = self.store.filter_status(status).await?;
        self.apply_filter_selection(selected);
        self.set_message("status filter applied".to_string());
        Ok(())
    }

    async fn submit_filter_priority(&mut self, values: Vec<String>) -> Result<()> {
        let Some(priority) = self.filter_value_or_reopen(
            values,
            "no matching priority",
            Self::begin_filter_priority,
        ) else {
            return Ok(());
        };
        let selected = self.store.filter_priority(priority).await?;
        self.apply_filter_selection(selected);
        self.set_message("priority filter applied".to_string());
        Ok(())
    }

    async fn submit_view_project(&mut self, values: Vec<String>) -> Result<()> {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
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

    async fn submit_switch_workspace(&mut self, values: Vec<String>) -> Result<()> {
        let Some(workspace) = self.require_picker_value(values, "no matching workspace") else {
            self.begin_switch_workspace().await?;
            return Ok(());
        };
        let (message, selected) = self.store.switch_workspace(workspace).await?;
        self.apply_filter_selection(selected);
        self.set_message(message);
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

    async fn open_conflict_list(&mut self) -> Result<()> {
        let selected = self.store.show_view(SidebarTarget::Conflicts).await?;
        self.apply_filter_selection(selected);
        let count = self
            .store
            .tasks
            .iter()
            .filter(|task| task.has_conflict)
            .count();
        let message = if count == 0 {
            "no unresolved conflicts".to_string()
        } else {
            format!("showing {count} conflicted tasks")
        };
        self.set_message(message);
        Ok(())
    }

    async fn conflict_targets_for_selected(&mut self) -> Result<Option<Vec<ConflictTarget>>> {
        self.store
            .conflict_targets(self.widgets.table.selected())
            .await
    }

    async fn load_conflict_targets_for_resolution(
        &mut self,
    ) -> Result<Option<Vec<ConflictTarget>>> {
        let Some(targets) = self.conflict_targets_for_selected().await? else {
            self.set_message("no selected task for conflict resolution".to_string());
            return Ok(None);
        };
        if targets.is_empty() {
            self.set_message("selected task has no unresolved conflicts".to_string());
            return Ok(None);
        }
        Ok(Some(targets))
    }

    fn start_conflict_field_flow(
        &mut self,
        targets: Vec<ConflictTarget>,
        flow: ConflictFlow,
        on_single: impl FnOnce(&mut Self, ConflictTarget),
    ) {
        if targets.len() == 1 {
            on_single(self, targets[0].clone());
        } else {
            self.conflict_flow = Some(flow);
            self.open_conflict_field_picker(&targets);
        }
    }

    async fn show_conflict_details(&mut self) -> Result<()> {
        let Some(targets) = self.conflict_targets_for_selected().await? else {
            self.set_message("no selected task for conflicts".to_string());
            return Ok(());
        };
        if targets.is_empty() {
            let display_ref = self
                .store
                .selected_task(self.widgets.table.selected())
                .map(|item| item.display_ref.clone())
                .unwrap_or_else(|| "task".to_string());
            self.set_message(format!("{display_ref} has no unresolved conflicts"));
            return Ok(());
        }
        let mut lines = Vec::new();
        for target in &targets {
            lines.push(format!("field={}", target.field));
            lines.push(format!(
                "local {}: {}",
                target.variant_a, target.local_value
            ));
            lines.push(format!(
                "remote {}: {}",
                target.variant_b, target.remote_value
            ));
            lines.push(String::new());
        }
        if lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        self.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
            CONFLICT_DETAILS_TITLE,
            lines,
        )));
        Ok(())
    }

    fn show_config_status(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
            CONFIG_STATUS_TITLE,
            self.store.config_status_lines()?,
        )));
        Ok(())
    }

    fn show_config_info(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
            CONFIG_INFO_TITLE,
            self.store.config_info_lines()?,
        )));
        Ok(())
    }

    fn show_config_paths(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::TextPanel(TextPanelState::new(
            CONFIG_PATHS_TITLE,
            self.store.config_path_lines()?,
        )));
        Ok(())
    }

    fn begin_config_init(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        let path = crate::config::config_file_path()?;
        self.overlay = Some(OverlayState::Confirm(ConfirmState {
            title: CONFIG_INIT_TITLE.to_string(),
            prompt: format!("Create default config at {}?", path.display()),
        }));
        Ok(())
    }

    fn submit_config_init(&mut self) -> Result<()> {
        let message = self.store.init_config()?;
        self.set_message(message);
        Ok(())
    }

    fn submit_delete_project_picker(&mut self, values: Vec<String>) {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_delete_project();
            return;
        };
        self.pending_delete_project = Some(project.clone());
        self.overlay = Some(OverlayState::Confirm(ConfirmState {
            title: DELETE_PROJECT_TITLE.to_string(),
            prompt: format!("Delete project {project}?"),
        }));
    }

    async fn submit_delete_project(&mut self) -> Result<()> {
        let Some(project) = self.pending_delete_project.take() else {
            self.set_message("project delete confirmation is not active".to_string());
            return Ok(());
        };
        match self.store.delete_project(&project).await {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => self.set_message(format!("error: {error:#}")),
        }
        Ok(())
    }

    fn move_to_conflict(&mut self, delta: isize) {
        let current = self.widgets.table.selected();
        let Some(next) = self.store.next_conflict_index(current, delta) else {
            self.set_message("no conflicts in current list".to_string());
            return;
        };
        if current == Some(next) {
            self.set_message("selected only conflict".to_string());
            return;
        }
        self.widgets.table.select(Some(next));
        self.focus = Focus::Tasks;
        let message = if delta > 0 {
            "selected next conflict"
        } else {
            "selected previous conflict"
        };
        self.set_message(message.to_string());
    }

    async fn begin_conflict_resolution(&mut self, choice: ConflictResolutionChoice) -> Result<()> {
        let Some(targets) = self.load_conflict_targets_for_resolution().await? else {
            return Ok(());
        };
        self.start_conflict_field_flow(
            targets.clone(),
            ConflictFlow::PickVariant { choice, targets },
            |app, target| app.open_conflict_confirm(choice, target),
        );
        Ok(())
    }

    async fn begin_manual_conflict_merge(&mut self) -> Result<()> {
        let Some(targets) = self.load_conflict_targets_for_resolution().await? else {
            return Ok(());
        };
        self.start_conflict_field_flow(
            targets.clone(),
            ConflictFlow::PickManual { targets },
            |app, target| app.open_manual_conflict_editor(target),
        );
        Ok(())
    }

    fn open_conflict_field_picker(&mut self, targets: &[ConflictTarget]) {
        let items = targets
            .iter()
            .map(|target| PickerItem {
                label: target.field.clone(),
                value: target.field.clone(),
                selected: false,
            })
            .collect();
        self.open_picker_overlay(CONFLICT_FIELD_TITLE, items, false);
    }

    async fn submit_conflict_field_picker(&mut self, values: Vec<String>) -> Result<()> {
        let Some(field) = self.require_picker_value(values, "no conflict field selected") else {
            return Ok(());
        };
        let flow = self.conflict_flow.take();
        match flow {
            Some(ConflictFlow::PickVariant { choice, targets }) => {
                let Some(target) = targets.into_iter().find(|target| target.field == field) else {
                    self.set_message(format!("no conflict for field={field}"));
                    return Ok(());
                };
                self.open_conflict_confirm(choice, target);
            }
            Some(ConflictFlow::PickManual { targets }) => {
                let Some(target) = targets.into_iter().find(|target| target.field == field) else {
                    self.set_message(format!("no conflict for field={field}"));
                    return Ok(());
                };
                self.open_manual_conflict_editor(target);
            }
            _ => self.set_message("conflict field picker is not active".to_string()),
        }
        Ok(())
    }

    fn open_conflict_confirm(&mut self, choice: ConflictResolutionChoice, target: ConflictTarget) {
        let value = match choice {
            ConflictResolutionChoice::Local => target.local_value.as_str(),
            ConflictResolutionChoice::Remote => target.remote_value.as_str(),
        };
        let title = match choice {
            ConflictResolutionChoice::Local => CONFLICT_CONFIRM_LOCAL_TITLE,
            ConflictResolutionChoice::Remote => CONFLICT_CONFIRM_REMOTE_TITLE,
        };
        self.conflict_flow = Some(ConflictFlow::ConfirmVariant {
            choice,
            target: target.clone(),
        });
        self.overlay = Some(OverlayState::Confirm(ConfirmState {
            title: title.to_string(),
            prompt: format!(
                "Resolve field={} with {}?",
                target.field,
                truncate_value_preview(value, 60)
            ),
        }));
    }

    async fn submit_confirmed_conflict_resolution(&mut self) -> Result<()> {
        let Some(ConflictFlow::ConfirmVariant { choice, target }) = self.conflict_flow.take()
        else {
            self.set_message("conflict confirmation is not active".to_string());
            return Ok(());
        };
        let value = match choice {
            ConflictResolutionChoice::Local => target.local_value.clone(),
            ConflictResolutionChoice::Remote => target.remote_value.clone(),
        };
        match self.store.resolve_conflict_value(target, value).await {
            Ok(result) => {
                self.conflict_flow = None;
                self.apply_mutation_result(result);
            }
            Err(error) => self.set_message(format!("error: {error:#}")),
        }
        Ok(())
    }

    fn open_manual_conflict_editor(&mut self, target: ConflictTarget) {
        self.conflict_flow = Some(ConflictFlow::EditManual {
            target: target.clone(),
        });
        match target.field.as_str() {
            "description" => {
                self.overlay = Some(OverlayState::MultilineInput(
                    MultilineInputState::from_value(
                        CONFLICT_MANUAL_TITLE,
                        format!("manual value for field={}:", target.field),
                        target.local_value.clone(),
                    ),
                ));
            }
            "title" => {
                self.overlay = Some(OverlayState::TextInput(TextInputState::new(
                    CONFLICT_MANUAL_TITLE,
                    format!("manual value for field={}:", target.field),
                    target.local_value.clone(),
                )));
            }
            "status" => {
                let items = self
                    .store
                    .status_picker_items(Some(target.local_value.as_str()));
                self.open_picker_overlay(CONFLICT_MANUAL_TITLE, items, false);
            }
            "priority" => {
                let items = self
                    .store
                    .priority_picker_items(target.local_value.as_str());
                self.open_picker_overlay(CONFLICT_MANUAL_TITLE, items, false);
            }
            "project" => {
                let items = self
                    .store
                    .existing_project_picker_items(target.local_value.as_str());
                self.open_picker_overlay(CONFLICT_MANUAL_TITLE, items, false);
            }
            "deleted" => {
                let items = deleted_picker_items(&target.local_value);
                self.open_picker_overlay(CONFLICT_MANUAL_TITLE, items, false);
            }
            _ => {
                self.conflict_flow = None;
                self.overlay = None;
                self.set_message(format!(
                    "manual merge is not supported for field={}",
                    target.field
                ));
            }
        }
    }

    async fn submit_manual_conflict_value(&mut self, value: String) -> Result<()> {
        let Some(ConflictFlow::EditManual { target }) = self.conflict_flow.take() else {
            self.set_message("manual conflict edit is not active".to_string());
            return Ok(());
        };
        match self
            .store
            .resolve_conflict_value(target.clone(), value)
            .await
        {
            Ok(result) => {
                self.conflict_flow = None;
                self.apply_mutation_result(result);
            }
            Err(error) => {
                self.set_message(format!("error: {error:#}"));
                self.open_manual_conflict_editor(target);
            }
        }
        Ok(())
    }
}

fn add_task_title_overlay(title: &str) -> bool {
    title == ADD_TASK_TITLE_TITLE || title.starts_with("Add task  project=")
}

fn selected_picker_index(items: &[PickerItem]) -> usize {
    items.iter().position(|item| item.selected).unwrap_or(0)
}

fn copy_to_clipboard(value: &str) -> Result<()> {
    let mut child = Command::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(value.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("pbcopy exited with {status}");
    }
    Ok(())
}

fn deleted_picker_items(selected: &str) -> Vec<PickerItem> {
    ["0", "1"]
        .into_iter()
        .map(|value| PickerItem {
            label: if value == "1" {
                "deleted".to_string()
            } else {
                "not deleted".to_string()
            },
            value: value.to_string(),
            selected: value == selected,
        })
        .collect()
}

fn truncate_value_preview(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let truncated: String = value.chars().take(max_chars).collect();
    format!("{truncated}…")
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
    use crate::tui::overlay::{
        ConfirmState, MultilineInputState, PickerState, TextInputState, TextPanelState,
    };
    use crate::tui::store::SidebarTarget;

    async fn test_app() -> App {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        reset_default_workspace(&pool).await;
        App::new(pool).await.unwrap()
    }

    fn test_task_draft(title: &str) -> TaskDraft {
        TaskDraft {
            title: title.to_string(),
            description: String::new(),
            project: None,
            priority: "none".to_string(),
            labels: Vec::new(),
        }
    }

    async fn create_and_select_task(app: &mut App, draft: TaskDraft) -> usize {
        let (_, selected) = app.store.create_task(draft, None).await.unwrap();
        let selected = selected.unwrap();
        app.widgets.table.select(Some(selected));
        selected
    }

    async fn test_app_with_pool() -> (tempfile::TempDir, SqlitePool, App) {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::db::open_db(&dir.path().join("test.db"))
            .await
            .unwrap();
        reset_default_workspace(&pool).await;
        let app = App::new(pool.clone()).await.unwrap();
        (dir, pool, app)
    }

    async fn reset_default_workspace(pool: &SqlitePool) {
        let mut conn = pool.acquire().await.unwrap();
        let default = crate::workspaces::ensure_default_workspace(&mut conn)
            .await
            .unwrap();
        crate::workspaces::set_active_workspace(default);
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_s() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL)
    }

    fn ctrl_c() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)
    }

    fn ctrl_p() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('p'), KeyModifiers::CONTROL)
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
    async fn ctrl_c_quits_from_normal_mode() {
        let mut app = test_app().await;
        app.dispatch_key(ctrl_c(), 24).await.unwrap();
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn ctrl_c_quits_while_overlay_captures_input() {
        let mut app = test_app().await;
        app.begin_search();
        app.dispatch_key(ctrl_c(), 24).await.unwrap();
        assert!(app.should_quit);
    }

    #[tokio::test]
    async fn prefix_key_enters_prefix_mode() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        assert_eq!(app.pending_shortcut, vec![KeyCode::Char('m')]);
    }

    #[tokio::test]
    async fn add_task_alias_executes_immediately() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert!(app.pending_shortcut.is_empty());
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if add_task_title_overlay(&state.title)
        ));
    }

    #[tokio::test]
    async fn prefix_is_inactive_while_overlay_captures_input() {
        let mut app = test_app().await;
        app.begin_search();
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();

        assert!(app.pending_shortcut.is_empty());
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Search { input }) if input.as_str() == "m"
        ));
    }

    #[tokio::test]
    async fn esc_cancels_prefix_before_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Detail);
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        app.handle_normal_key(KeyCode::Esc).await.unwrap();
        assert!(app.pending_shortcut.is_empty());
        assert!(matches!(app.overlay, Some(OverlayState::Detail)));

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
        app.overlay = Some(OverlayState::Help { scroll: 0 });
        app.begin_search();
        assert!(matches!(app.overlay, Some(OverlayState::Search { .. })));
    }

    #[tokio::test]
    async fn toggle_help_closes_active_help_overlay() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::Help { scroll: 0 });
        app.toggle_help();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn help_key_opens_help_overlay() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('?')).await.unwrap();
        assert!(matches!(app.overlay, Some(OverlayState::Help { .. })));
    }

    #[tokio::test]
    async fn h_and_l_move_between_sidebar_and_tasks() {
        let mut app = test_app().await;
        app.focus = Focus::Tasks;
        app.handle_normal_key(KeyCode::Char('h')).await.unwrap();
        assert_eq!(app.focus, Focus::Sidebar);

        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert_eq!(app.focus, Focus::Tasks);
    }

    #[tokio::test]
    async fn config_info_opens_text_panel() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();

        let Some(OverlayState::TextPanel(panel)) = app.overlay else {
            panic!("expected text panel");
        };
        assert_eq!(panel.title, CONFIG_INFO_TITLE);
        assert!(panel.lines.iter().any(|line| line.contains("config path:")));
    }

    #[tokio::test]
    async fn config_status_opens_text_panel() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();

        let Some(OverlayState::TextPanel(panel)) = app.overlay else {
            panic!("expected text panel");
        };
        assert_eq!(panel.title, CONFIG_STATUS_TITLE);
        assert!(
            panel
                .lines
                .iter()
                .any(|line| line.contains("sync enabled:"))
        );
        assert!(
            panel
                .lines
                .iter()
                .any(|line| line.contains("daemon state: not checked from TUI"))
        );
    }

    #[tokio::test]
    async fn config_paths_opens_text_panel() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();

        let Some(OverlayState::TextPanel(panel)) = app.overlay else {
            panic!("expected text panel");
        };
        assert_eq!(panel.title, CONFIG_PATHS_TITLE);
        assert!(
            panel
                .lines
                .iter()
                .any(|line| line.contains("effective database:"))
        );
    }

    #[tokio::test]
    async fn config_init_requires_confirmation() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('i')).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState { ref title, .. })) if title == CONFIG_INIT_TITLE
        ));
    }

    #[tokio::test]
    async fn config_init_cancel_does_not_set_success_message() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('C')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('i')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('n')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert!(app.message.is_none());
    }

    #[tokio::test]
    async fn command_panel_runs_config_show() {
        let mut app = test_app().await;
        app.begin_command();
        type_chars(&mut app, "config-show").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::TextPanel(TextPanelState { ref title, .. })) if title == CONFIG_INFO_TITLE
        ));
    }

    #[tokio::test]
    async fn command_panel_runs_workspace_switch() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);

        app.begin_command();
        type_chars(&mut app, "workspace-switch").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, items, .. }))
                if title == SWITCH_WORKSPACE_TITLE
                    && items.iter().any(|item| item.value == "client-work")
        ));

        reset_default_workspace(&pool).await;
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
    async fn order_shortcut_sets_sort() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();
        assert_eq!(app.store.sort, TaskSort::Priority);
        assert_eq!(app.message.as_deref(), Some("order priority asc"));
    }

    #[tokio::test]
    async fn order_reverse_shortcut_toggles_direction() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
        assert_eq!(app.store.sort_direction_label(), "desc");
        assert_eq!(app.message.as_deref(), Some("order queue desc"));
    }

    #[tokio::test]
    async fn due_order_shortcut_reports_unsupported() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert_eq!(
            app.message.as_deref(),
            Some(":order-due is disabled: tasks do not have due dates")
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
    async fn switch_workspace_shortcut_opens_picker() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('w')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(PickerState { title, .. })) if title == SWITCH_WORKSPACE_TITLE
        ));

        reset_default_workspace(&pool).await;
    }

    #[tokio::test]
    async fn switch_workspace_changes_active_workspace() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        create_and_select_task(&mut app, test_task_draft("Default only")).await;

        let mut conn = pool.acquire().await.unwrap();
        crate::workspaces::create_workspace(&mut conn, "Client Work")
            .await
            .unwrap();
        drop(conn);
        app.refresh().await.unwrap();

        app.store.filters.status = Some("todo".to_string());
        app.store.active_view = SidebarTarget::Todo;

        let (message, selected) = app
            .store
            .switch_workspace("client-work".to_string())
            .await
            .unwrap();
        app.apply_filter_selection(selected);
        app.set_message(message);

        assert_eq!(app.store.active_workspace.key, "client-work");
        assert_eq!(app.store.active_view, SidebarTarget::All);
        assert!(app.store.filters.status.is_none());
        assert!(app.store.tasks.is_empty());
        assert!(app.overlay.is_none());
        assert!(
            app.message
                .as_deref()
                .is_some_and(|message| message.contains("switched workspace to client-work"))
        );

        reset_default_workspace(&pool).await;
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
        app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state))
                if add_task_title_overlay(&state.title) && state.prompt.is_empty()
        ));
    }

    #[tokio::test]
    async fn add_task_alias_creates_task_after_title() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.priority, "none");
        assert_eq!(task.task.description, "");
        assert!(task.labels.is_empty());
    }

    #[tokio::test]
    async fn add_task_uses_active_project_view() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        let selected = app
            .store
            .show_view(SidebarTarget::Project("mobile-app".to_string()))
            .await
            .unwrap();
        app.apply_filter_selection(selected);

        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.project_key, "mobile-app");
        assert_eq!(app.store.filters.project.as_deref(), Some("mobile-app"));
    }

    #[tokio::test]
    async fn add_task_flow_configures_project_and_priority_from_title() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        type_chars(&mut app, "Write docs").await;
        app.handle_overlay_key(key(KeyCode::Tab)).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.title == ADD_TASK_TITLE_PROJECT_TITLE
        ));
        type_chars(&mut app, "mobile").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        app.handle_overlay_key(ctrl_p()).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.title == ADD_TASK_TITLE_PRIORITY_TITLE
        ));
        type_chars(&mut app, "high").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(app.overlay.is_none());
        let selected = app.widgets.table.selected().unwrap();
        let task = &app.store.tasks[selected];
        assert_eq!(task.task.title, "Write docs");
        assert_eq!(task.task.project_key, "mobile-app");
        assert_eq!(task.task.priority, "high");
        assert_eq!(task.task.description, "");
        assert!(task.labels.is_empty());
    }

    #[tokio::test]
    async fn add_task_flow_cancels_at_title_step() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn add_task_blank_title_is_rejected() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(app.message.as_deref(), Some("task title is required"));
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state)) if add_task_title_overlay(&state.title)
        ));
    }

    #[tokio::test]
    async fn add_note_requires_selected_task() {
        let mut app = test_app().await;
        app.widgets.table.select(None);
        app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("no selected task for note"));
    }

    #[tokio::test]
    async fn add_note_alias_requires_selected_task() {
        let mut app = test_app().await;
        app.widgets.table.select(None);
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("no selected task for note"));
    }

    #[tokio::test]
    async fn add_note_flow_creates_note_for_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Note target")).await;

        app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
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
        create_and_select_task(&mut app, test_task_draft("Note target")).await;

        app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        app.handle_overlay_key(ctrl_s()).await.unwrap();

        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("note body is required"));
    }

    #[tokio::test]
    async fn planned_and_disabled_shortcut_and_command_report_non_executing() {
        let mut app = test_app().await;

        app.handle_normal_key(KeyCode::Char('g')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
        assert_eq!(
            app.message.as_deref(),
            Some(":view-deleted is not yet implemented: not yet implemented")
        );
        assert!(app.overlay.is_none());

        app.begin_command();
        type_chars(&mut app, "view-deleted").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            app.message.as_deref(),
            Some(":view-deleted is not yet implemented: not yet implemented")
        );
        assert!(app.overlay.is_none());

        app.handle_normal_key(KeyCode::Char('o')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        assert_eq!(
            app.message.as_deref(),
            Some(":order-due is disabled: tasks do not have due dates")
        );

        app.begin_command();
        type_chars(&mut app, "order-due").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(
            app.message.as_deref(),
            Some(":order-due is disabled: tasks do not have due dates")
        );
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn no_selected_mutating_shortcuts_report_failure() {
        let mut app = test_app().await;
        app.widgets.table.select(None);

        for sequence in [
            [KeyCode::Char('m'), KeyCode::Char('i')],
            [KeyCode::Char('m'), KeyCode::Char('h')],
            [KeyCode::Char('m'), KeyCode::Char('D')],
            [KeyCode::Char('m'), KeyCode::Char('r')],
        ] {
            app.message = None;
            app.handle_normal_key(sequence[0]).await.unwrap();
            app.handle_normal_key(sequence[1]).await.unwrap();
            assert_eq!(app.message.as_deref(), Some("no selected task to edit"));
        }
    }

    #[tokio::test]
    async fn esc_closes_every_overlay_variant() {
        let overlays = vec![
            OverlayState::Help { scroll: 0 },
            OverlayState::Detail,
            OverlayState::Search {
                input: LineEdit::new("q".to_string()),
            },
            OverlayState::Command {
                input: LineEdit::new("ref".to_string()),
            },
            OverlayState::TextInput(TextInputState::new("T", "P", "x".to_string())),
            OverlayState::MultilineInput(MultilineInputState {
                title: "M".to_string(),
                prompt: "P".to_string(),
                lines: vec!["x".to_string()],
                row: 0,
                column: 1,
            }),
            OverlayState::Picker(PickerState {
                title: "Pick".to_string(),
                filter: LineEdit::blank(),
                items: vec![PickerItem {
                    label: "One".to_string(),
                    value: "one".to_string(),
                    selected: false,
                }],
                selected: 0,
                multi: false,
            }),
            OverlayState::Confirm(ConfirmState {
                title: "C".to_string(),
                prompt: "?".to_string(),
            }),
            OverlayState::TextPanel(TextPanelState {
                title: "Panel".to_string(),
                lines: vec!["line".to_string()],
                scroll: 0,
            }),
        ];

        for overlay in overlays {
            let mut app = test_app().await;
            app.overlay = Some(overlay);
            app.dispatch_key(key(KeyCode::Esc), 24).await.unwrap();
            assert!(app.overlay.is_none());
            assert!(app.pending_shortcut.is_empty());
        }
    }

    async fn insert_title_conflict(
        pool: &SqlitePool,
        app: &mut App,
        selected: usize,
        local: &str,
        remote: &str,
    ) {
        let task_id = app.store.tasks[selected].task.id.clone();
        insert_title_conflict_for_task_id(pool, app, &task_id, local, remote).await;
    }

    async fn insert_title_conflict_for_task_id(
        pool: &SqlitePool,
        app: &mut App,
        task_id: &str,
        local: &str,
        remote: &str,
    ) {
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO conflicts(task_id, field, base_version, local_value, remote_value,
             local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved)
             VALUES (?, 'title', NULL, ?, ?, NULL, ?, 'a', 'b', ?, 0)",
        )
        .bind(task_id)
        .bind(local)
        .bind(remote)
        .bind(crate::ids::new_id())
        .bind(crate::ids::now())
        .execute(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        app.refresh().await.unwrap();
    }

    #[tokio::test]
    async fn conflict_list_shortcut_applies_conflicts_view() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('l')).await.unwrap();
        assert_eq!(app.store.active_view, SidebarTarget::Conflicts);
        assert!(app.store.filters.conflicts_only);
        assert_eq!(app.message.as_deref(), Some("no unresolved conflicts"));
    }

    #[tokio::test]
    async fn conflict_show_opens_text_panel_and_esc_closes() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Conflict show")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextPanel(state))
                if state.lines.iter().any(|line| line.contains("field=title"))
        ));

        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.overlay.is_none());
    }

    #[tokio::test]
    async fn conflict_next_selects_next_conflicted_task() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        create_and_select_task(&mut app, test_task_draft("First")).await;
        create_and_select_task(&mut app, test_task_draft("Second")).await;
        let first_id = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == "First")
            .unwrap()
            .task
            .id
            .clone();
        let second_id = app
            .store
            .tasks
            .iter()
            .find(|item| item.task.title == "Second")
            .unwrap()
            .task
            .id
            .clone();
        insert_title_conflict_for_task_id(&pool, &mut app, &first_id, "local one", "remote one")
            .await;
        insert_title_conflict_for_task_id(&pool, &mut app, &second_id, "local two", "remote two")
            .await;
        let first = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == first_id)
            .unwrap();
        let second = app
            .store
            .tasks
            .iter()
            .position(|item| item.task.id == second_id)
            .unwrap();
        app.widgets.table.select(Some(first));

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('n')).await.unwrap();
        assert_eq!(app.widgets.table.selected(), Some(second));
        assert_eq!(app.message.as_deref(), Some("selected next conflict"));
    }

    #[tokio::test]
    async fn accept_local_conflict_resolves_after_confirmation() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Confirm(state)) if state.title == CONFLICT_CONFIRM_LOCAL_TITLE
        ));

        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(app.store.tasks[selected].task.title, "local title");
        assert!(!app.store.tasks[selected].has_conflict);
        assert!(
            app.message.as_deref().is_some_and(
                |message| message.contains("resolved") && message.contains("field=title")
            )
        );
    }

    #[tokio::test]
    async fn accept_remote_conflict_resolves_after_confirmation() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('r')).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert_eq!(app.store.tasks[selected].task.title, "remote title");
        assert!(!app.store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn manual_conflict_merge_resolves_with_submitted_value() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        type_chars(&mut app, " merged").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert_eq!(app.store.tasks[selected].task.title, "local title merged");
        assert!(!app.store.tasks[selected].has_conflict);
    }

    #[tokio::test]
    async fn conflict_resolution_without_selected_task_reports_message() {
        let mut app = test_app().await;
        app.widgets.table.select(None);

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert_eq!(
            app.message.as_deref(),
            Some("no selected task for conflict resolution")
        );
    }

    #[tokio::test]
    async fn cancel_clears_conflict_flow() {
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Conflict")).await;
        insert_title_conflict(&pool, &mut app, selected, "local title", "remote title").await;

        app.handle_normal_key(KeyCode::Char('c')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('a')).await.unwrap();
        assert!(app.conflict_flow.is_some());
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();
        assert!(app.conflict_flow.is_none());
    }

    #[test]
    fn truncate_value_preview_uses_character_count() {
        assert_eq!(truncate_value_preview("abc", 5), "abc");
        assert_eq!(truncate_value_preview("abcdef", 3), "abc…");
    }

    #[tokio::test]
    async fn generic_text_input_submits_message() {
        let mut app = test_app().await;
        app.overlay = Some(OverlayState::TextInput(TextInputState::new(
            "Title",
            "Enter title",
            "done".to_string(),
        )));
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert!(app.overlay.is_none());
        assert_eq!(app.message.as_deref(), Some("submitted Title"));
    }

    #[tokio::test]
    async fn add_project_shortcut_opens_prompt_and_creates_project() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
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
        app.handle_normal_key(KeyCode::Char('A')).await.unwrap();
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
        create_and_select_task(&mut app, test_task_draft("Old title")).await;

        app.handle_normal_key(KeyCode::Char('e')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('t')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::TextInput(state))
                if state.title == EDIT_TITLE_TITLE && state.input.as_str() == "Old title"
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
        create_and_select_task(
            &mut app,
            TaskDraft {
                description: "first\nsecond".to_string(),
                ..test_task_draft("Description target")
            },
        )
        .await;

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
        create_and_select_task(&mut app, test_task_draft("Project target")).await;

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
        create_and_select_task(
            &mut app,
            TaskDraft {
                priority: "high".to_string(),
                ..test_task_draft("Priority target")
            },
        )
        .await;

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
        create_and_select_task(
            &mut app,
            TaskDraft {
                labels: vec!["bug".to_string()],
                ..test_task_draft("Label target")
            },
        )
        .await;

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
    async fn status_picker_alias_updates_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Status alias")).await;

        app.handle_normal_key(KeyCode::Char('s')).await.unwrap();
        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.title == EDIT_STATUS_TITLE
        ));
        type_chars(&mut app, "todo").await;
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, "todo");
    }

    #[tokio::test]
    async fn done_and_cancel_aliases_update_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Status alias")).await;

        app.handle_normal_key(KeyCode::Char('d')).await.unwrap();
        let selected = app.store.show_view(SidebarTarget::Done).await.unwrap();
        app.widgets.table.select(selected);
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, "done");

        app.handle_normal_key(KeyCode::Char('x')).await.unwrap();
        let selected = app
            .store
            .filter_status("canceled".to_string())
            .await
            .unwrap();
        app.widgets.table.select(selected);
        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.status, "canceled");
    }

    #[tokio::test]
    async fn exact_priority_shortcut_updates_selected_task() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Priority shortcut")).await;

        app.handle_normal_key(KeyCode::Char('m')).await.unwrap();
        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();

        let selected = app.widgets.table.selected().unwrap();
        assert_eq!(app.store.tasks[selected].task.priority, "urgent");
    }

    #[tokio::test]
    async fn priority_alias_opens_picker() {
        let mut app = test_app().await;
        create_and_select_task(&mut app, test_task_draft("Priority alias")).await;

        app.handle_normal_key(KeyCode::Char('p')).await.unwrap();

        assert!(matches!(
            &app.overlay,
            Some(OverlayState::Picker(state)) if state.title == EDIT_PRIORITY_TITLE
        ));
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
        let (_dir, pool, mut app) = test_app_with_pool().await;
        let selected = create_and_select_task(
            &mut app,
            TaskDraft {
                description: "old".to_string(),
                ..test_task_draft("Conflict target")
            },
        )
        .await;
        let task_id = app.store.tasks[selected].task.id.clone();
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
    async fn copy_requires_selected_task() {
        let mut app = test_app().await;
        app.widgets.table.select(None);

        app.copy_selected_ref(TaskRefKind::Short);

        assert_eq!(app.message.as_deref(), Some("no selected task to copy"));
    }

    #[tokio::test]
    async fn undo_shortcut_reverts_last_mutation() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        app.store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "After");

        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
        assert_eq!(app.store.tasks[selected].task.title, "Before");
        assert!(app.message.as_ref().unwrap().contains("undid"));
    }

    #[tokio::test]
    async fn undo_command_reverts_last_mutation() {
        let mut app = test_app().await;
        let selected = create_and_select_task(&mut app, test_task_draft("Before")).await;
        app.store
            .update_title(Some(selected), "After".to_string())
            .await
            .unwrap();

        app.begin_command();
        for ch in "undo".chars() {
            app.handle_overlay_key(key(KeyCode::Char(ch)))
                .await
                .unwrap();
        }
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert_eq!(app.store.tasks[selected].task.title, "Before");
    }

    #[tokio::test]
    async fn undo_reports_nothing_to_undo() {
        let mut app = test_app().await;
        app.handle_normal_key(KeyCode::Char('u')).await.unwrap();
        assert_eq!(app.message.as_deref(), Some("nothing to undo"));
    }

    #[tokio::test]
    async fn delete_project_opens_project_picker_from_task_focus() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginDeleteProject).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Picker(PickerState { ref title, .. })) if title == DELETE_PROJECT_TITLE
        ));
        assert!(app.message.is_none());
    }

    #[tokio::test]
    async fn delete_project_picker_preselects_sidebar_project() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.focus = Focus::Sidebar;
        let project_index = app
            .store
            .sidebar_entries
            .iter()
            .position(|entry| entry.target == Some(SidebarTarget::Project("mobile-app".to_string())))
            .unwrap();
        app.widgets.sidebar.select(Some(project_index));

        app.execute(Action::BeginDeleteProject).await.unwrap();

        let Some(OverlayState::Picker(state)) = &app.overlay else {
            panic!("expected project picker");
        };
        assert_eq!(state.items[state.selected].value, "mobile-app");
    }

    #[tokio::test]
    async fn delete_project_confirmation_removes_selected_project() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();

        app.execute(Action::BeginDeleteProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();

        assert!(matches!(
            app.overlay,
            Some(OverlayState::Confirm(ConfirmState { ref title, .. })) if title == DELETE_PROJECT_TITLE
        ));
        app.handle_overlay_key(key(KeyCode::Char('y')))
            .await
            .unwrap();

        assert_eq!(app.message.as_deref(), Some("deleted project mobile-app"));
        assert!(
            !app.store
                .projects
                .iter()
                .any(|project| project.key == "mobile-app")
        );
        assert!(app.pending_delete_project.is_none());
    }

    #[tokio::test]
    async fn delete_project_cancel_clears_pending_state() {
        let mut app = test_app().await;
        app.store
            .create_project("Mobile App".to_string())
            .await
            .unwrap();
        app.focus = Focus::Sidebar;
        let project_index = app
            .store
            .sidebar_entries
            .iter()
            .position(|entry| {
                entry.target == Some(SidebarTarget::Project("mobile-app".to_string()))
            })
            .unwrap();
        app.widgets.sidebar.select(Some(project_index));

        app.execute(Action::BeginDeleteProject).await.unwrap();
        app.handle_overlay_key(key(KeyCode::Enter)).await.unwrap();
        assert_eq!(app.pending_delete_project.as_deref(), Some("mobile-app"));
        app.handle_overlay_key(key(KeyCode::Esc)).await.unwrap();

        assert!(app.pending_delete_project.is_none());
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
