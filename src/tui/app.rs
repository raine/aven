#[cfg(not(test))]
use std::fs;
#[cfg(not(test))]
use std::io;
use std::process::Command;
use std::time::{Duration, Instant};
#[cfg(not(test))]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{
    self, DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEvent, KeyModifiers,
};
use crossterm::execute;
#[cfg(not(test))]
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::DefaultTerminal;
use ratatui::layout::Size;
use ratatui::widgets::{ListState, TableState};
use sqlx::SqlitePool;

use crate::operations::TaskDraft;
use crate::query::TaskSort;
use crate::tui::authoring::{
    ADD_NOTE_TITLE, ADD_TASK_TITLE_PRIORITY_TITLE, ADD_TASK_TITLE_PROJECT_TITLE, AddNoteSubmit,
    AddTaskStep, AddTaskTitleSubmit, AuthoringState,
};
use crate::tui::config_overlay::{
    config_info_overlay, config_init_overlay, config_paths_overlay, config_status_overlay,
};
use crate::tui::conflict_flow::{ConflictFlowState, ConflictResolutionChoice};
use crate::tui::event::{
    Action, CommandLookup, ShortcutLookup, lookup_command, resolve_shortcut, shortcut_label,
};
use crate::tui::navigation::{
    DetailShortcut, detail_shortcut, detail_task_delta, handle_detail_overlay_key, next_index,
    next_selectable_sidebar,
};
use crate::tui::overlay::{
    AddTaskState, ConfirmState, LineEdit, MultilineInputState, OverlayOutcome, OverlayRoute,
    OverlayState, OverlaySubmit, OverlayView, PickerItem, PickerMode, PickerState, TextInputState,
};
use crate::tui::store::{SidebarTarget, TuiStore};
use crate::tui::toast::{Toast, ToastSeverity};
use crate::tui::ui::{self, detail_help_scroll_cap, help_scroll_cap};

const ADD_PROJECT_TITLE: &str = "Add project";
const DELETE_PROJECT_TITLE: &str = "Delete project";
const DELETE_TASK_TITLE: &str = "Delete task";
const ADD_LABEL_TITLE: &str = "Add label";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TaskRefKind {
    Short,
    Durable,
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
    pub(crate) message: Option<Toast>,
    pub(crate) message_at: Option<Instant>,
    pub(super) pending_shortcut: Vec<KeyCode>,
    detail_context: bool,
    authoring: AuthoringState,
    pub(super) conflict_flow: ConflictFlowState,
    pending_delete_project: Option<String>,
    needs_terminal_clear: bool,
}

impl App {
    pub(crate) async fn new(pool: SqlitePool, project: Option<&str>) -> Result<Self> {
        let store = match project {
            Some("") => TuiStore::new_for_inferred_project(pool).await?,
            Some(project) => TuiStore::new_for_project(pool, project).await?,
            None => TuiStore::new(pool).await?,
        };
        Self::new_with_store(store)
    }

    #[cfg(test)]
    pub(crate) async fn new_for_tests(pool: SqlitePool) -> Result<Self> {
        let store = TuiStore::new(pool).await?;
        Self::new_with_store(store)
    }

    fn new_with_store(store: TuiStore) -> Result<Self> {
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
            detail_context: false,
            authoring: AuthoringState::default(),
            conflict_flow: ConflictFlowState::default(),
            pending_delete_project: None,
            needs_terminal_clear: false,
        };
        app.widgets.sidebar.select(app.store.sidebar_selection());
        app.widgets
            .table
            .select(Some(0).filter(|_| !app.store.tasks.is_empty()));
        Ok(app)
    }

    pub(crate) async fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        execute!(std::io::stdout(), EnableBracketedPaste)?;
        let result = self.run_loop(terminal).await;
        execute!(std::io::stdout(), DisableBracketedPaste)?;
        result
    }

    async fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.should_quit {
            let view = self.view();
            terminal.draw(|frame| ui::render(frame, &self.store, &mut self.widgets, &view))?;

            if event::poll(Duration::from_millis(250))? {
                match event::read()? {
                    Event::Key(key) => {
                        let result = self.dispatch_key(key, terminal.size()?).await;
                        if let Err(error) = result {
                            self.set_error(format!("{error:#}"));
                        }
                        if self.needs_terminal_clear {
                            self.needs_terminal_clear = false;
                            terminal.clear()?;
                        }
                    }
                    Event::Paste(text) => self.dispatch_paste(&text),
                    _ => {}
                }
            }

            if self.store.last_refresh.elapsed() >= Duration::from_secs(5)
                && let Err(error) = self.refresh().await
            {
                self.set_error(format!("refresh failed: {error:#}"));
            }

            self.clear_expired_message();
        }
        Ok(())
    }

    pub(crate) fn view(&self) -> ui::ViewState {
        ui::ViewState {
            focus: self.focus,
            overlay: self.overlay.as_ref().map(OverlayView::from),
            detail_underlay: self.detail_underlay(),
            message: self.message.clone(),
            pending_shortcut: self
                .pending_shortcut
                .iter()
                .map(|code| crate::tui::event::key_label(*code))
                .collect(),
        }
    }

    fn dispatch_paste(&mut self, text: &str) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        self.overlay = Some(crate::tui::overlay::handle_generic_overlay_paste(
            text, overlay,
        ));
    }

    async fn dispatch_key(&mut self, key: KeyEvent, terminal_size: Size) -> Result<()> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.handle(Action::Quit).await
        } else if key.code == KeyCode::Esc && !self.pending_shortcut.is_empty() {
            self.pending_shortcut.clear();
            Ok(())
        } else if self.overlay_captures_input() {
            if key.code == KeyCode::Char('?')
                && matches!(self.overlay, Some(OverlayState::Detail { .. }))
            {
                self.toggle_help_at_height(terminal_size.height);
                Ok(())
            } else {
                self.handle_overlay_key_at_size(key, terminal_size).await
            }
        } else if key.code == KeyCode::Char('?') {
            self.toggle_help_at_height(terminal_size.height);
            Ok(())
        } else {
            self.handle_normal_key(key.code).await
        }
    }

    fn overlay_captures_input(&self) -> bool {
        self.overlay
            .as_ref()
            .is_some_and(OverlayState::captures_input)
    }

    fn detail_underlay(&self) -> bool {
        self.detail_context
            || matches!(
                self.overlay,
                Some(OverlayState::Detail { .. } | OverlayState::DetailHelp { .. })
            )
            || self.authoring.detail_underlay()
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
                self.set_warning(format!("invalid shortcut: {label}"));
            }
        }
        Ok(())
    }

    pub(crate) async fn handle_overlay_key(&mut self, key: KeyEvent) -> Result<()> {
        self.handle_overlay_key_at_size(key, Size::new(80, 24))
            .await
    }

    async fn handle_overlay_key_at_size(
        &mut self,
        key: KeyEvent,
        terminal_size: Size,
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
                self.handle_generic_overlay_key(key, overlay, terminal_size)
                    .await?
            }
        }

        if self.detail_context && self.overlay.is_none() {
            self.restore_detail_overlay(true);
        }

        Ok(())
    }

    async fn handle_generic_overlay_key(
        &mut self,
        key: KeyEvent,
        overlay: OverlayState,
        terminal_size: Size,
    ) -> Result<()> {
        if let OverlayState::Detail { scroll } = overlay {
            if let Some(outcome) = self.handle_detail_shortcut(key, scroll).await? {
                self.overlay = outcome;
                return Ok(());
            }

            if let Some(delta) = detail_task_delta(key) {
                self.select_detail_task(delta);
                self.overlay = Some(OverlayState::Detail { scroll: 0 });
                return Ok(());
            }

            let overlay = OverlayState::Detail { scroll };
            let task = self.store.selected_task(self.widgets.table.selected());
            let outcome = handle_detail_overlay_key(
                key,
                overlay,
                terminal_size.width,
                terminal_size.height,
                task,
            );
            match outcome {
                OverlayOutcome::None(overlay) => self.overlay = Some(overlay),
                OverlayOutcome::Cancelled => self.cancel_authoring_overlay(),
                OverlayOutcome::Submitted(submit) => self.handle_overlay_submit(submit).await?,
            }
            return Ok(());
        }

        if self.pending_shortcut == [KeyCode::Char('x')] {
            self.pending_shortcut.clear();
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('e') {
                match &overlay {
                    OverlayState::MultilineInput(state)
                        if state.route == OverlayRoute::EditDescription =>
                    {
                        self.open_description_external_editor(state.clone());
                    }
                    OverlayState::AddTask(state) if state.focus == AddTaskStep::Description => {
                        if self.capture_add_task_state(state) {
                            self.open_add_task_description_editor();
                        }
                    }
                    _ => self.overlay = Some(overlay),
                }
                return Ok(());
            }
        }

        if is_editor_prefix_key(key)
            && matches!(
                &overlay,
                OverlayState::MultilineInput(state)
                    if state.route == OverlayRoute::EditDescription
            )
        {
            self.pending_shortcut = vec![KeyCode::Char('x')];
            self.overlay = Some(overlay);
            return Ok(());
        }

        if let OverlayState::AddTask(state) = &overlay {
            if is_editor_prefix_key(key) {
                if state.focus == AddTaskStep::Description {
                    self.pending_shortcut = vec![KeyCode::Char('x')];
                    self.overlay = Some(overlay);
                } else {
                    self.overlay = Some(overlay);
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
                if self.capture_add_task_state(state) {
                    self.begin_add_task_title_priority();
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('b') {
                if self.capture_add_task_state(state) {
                    self.begin_add_task_title_project();
                }
                return Ok(());
            }
        }

        let scroll_cap = match overlay {
            OverlayState::DetailHelp { .. } => detail_help_scroll_cap(terminal_size.height),
            _ => help_scroll_cap(terminal_size.height),
        };
        let was_detail_help = matches!(overlay, OverlayState::DetailHelp { .. });
        let was_add_task_description_editor = matches!(
            &overlay,
            OverlayState::MultilineInput(state) if state.route == OverlayRoute::AddTaskDescription
        );
        let outcome = crate::tui::overlay::handle_generic_overlay_key(key, overlay, scroll_cap);
        match outcome {
            OverlayOutcome::None(overlay) => self.overlay = Some(overlay),
            OverlayOutcome::Cancelled if was_detail_help => {
                self.overlay = Some(OverlayState::Detail { scroll: 0 })
            }
            OverlayOutcome::Cancelled if was_add_task_description_editor => {
                self.begin_add_task_step()
            }
            OverlayOutcome::Cancelled => self.cancel_authoring_overlay(),
            OverlayOutcome::Submitted(submit) => self.handle_overlay_submit(submit).await?,
        }
        Ok(())
    }

    async fn handle_detail_shortcut(
        &mut self,
        key: KeyEvent,
        scroll: u16,
    ) -> Result<Option<Option<OverlayState>>> {
        if !key.modifiers.is_empty() {
            return Ok(None);
        }

        let mut sequence = self.pending_shortcut.clone();
        sequence.push(key.code);
        match detail_shortcut(&sequence) {
            DetailShortcut::Action(action) => {
                self.pending_shortcut.clear();
                self.detail_context = true;
                self.execute(action).await?;
                if self.detail_context && self.overlay.is_none() {
                    self.restore_detail_overlay_at_scroll(true, scroll);
                }
                Ok(Some(self.overlay.take()))
            }
            DetailShortcut::Prefix => {
                self.pending_shortcut = sequence;
                Ok(Some(Some(OverlayState::Detail { scroll })))
            }
            DetailShortcut::Missing(label) if !self.pending_shortcut.is_empty() => {
                self.pending_shortcut.clear();
                self.set_warning(format!("invalid shortcut: {label}"));
                Ok(Some(Some(OverlayState::Detail { scroll })))
            }
            DetailShortcut::Missing(_) => Ok(None),
        }
    }

    async fn handle_overlay_submit(&mut self, submit: OverlaySubmit) -> Result<()> {
        match submit {
            OverlaySubmit::AddTask { title, description } => {
                self.authoring
                    .capture_add_task_fields(title, description, AddTaskStep::Title);
                self.submit_add_task_from_authoring().await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::AddTaskTitleProject,
                values,
                ..
            } => {
                if self.authoring.apply_add_task_project(values) {
                    self.begin_add_task_step();
                }
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::AddTaskTitlePriority,
                values,
                ..
            } => {
                if self.authoring.apply_add_task_priority(values) {
                    self.begin_add_task_step();
                }
            }
            OverlaySubmit::Multiline {
                route: OverlayRoute::AddTaskDescription,
                value,
                ..
            } => {
                if self.authoring.capture_add_task_fields(
                    self.authoring
                        .add_task_context()
                        .map(|context| context.title)
                        .unwrap_or_default(),
                    value,
                    AddTaskStep::Description,
                ) {
                    self.begin_add_task_step();
                }
            }
            OverlaySubmit::Multiline {
                route: OverlayRoute::AddNote,
                value,
                ..
            } => {
                self.submit_add_note(value).await?;
            }
            OverlaySubmit::Text {
                route: OverlayRoute::AddProject,
                value,
                ..
            } => {
                let message = self.store.create_project(value).await?;
                self.restore_selection_after_mutation();
                self.set_success(message);
            }
            OverlaySubmit::Text {
                route: OverlayRoute::AddLabel,
                value,
                ..
            } => {
                let message = self.store.create_label(value).await?;
                self.set_success(message);
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::EditStatus,
                values,
                ..
            } => {
                if let Some(status) = values.first() {
                    self.submit_edit_status(status.clone()).await?;
                } else {
                    self.set_warning("no matching status");
                    self.begin_status_picker();
                }
            }
            OverlaySubmit::Text {
                route: OverlayRoute::EditTitle,
                value,
                ..
            } => {
                self.submit_edit_title(value).await?;
            }
            OverlaySubmit::Multiline {
                route: OverlayRoute::EditDescription,
                value,
                ..
            } => {
                self.submit_edit_description(value).await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::EditProject,
                values,
                ..
            } => {
                if let Some(project) = values.first() {
                    self.submit_edit_project(project.clone()).await?;
                } else {
                    self.set_warning("no matching project");
                    self.begin_edit_project();
                }
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::EditPriority,
                values,
                ..
            } => {
                if let Some(priority) = values.first() {
                    self.submit_edit_priority(priority.clone()).await?;
                } else {
                    self.set_warning("no matching priority");
                    self.begin_edit_priority();
                }
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::EditLabels,
                values,
                ..
            } => {
                self.submit_edit_labels(values).await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::FilterProject,
                values,
                ..
            } => {
                self.submit_filter_project(values).await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::FilterLabel,
                values,
                ..
            } => {
                self.submit_filter_label(values).await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::FilterStatus,
                values,
                ..
            } => {
                self.submit_filter_status(values).await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::FilterPriority,
                values,
                ..
            } => {
                self.submit_filter_priority(values).await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::ViewProject,
                values,
                ..
            } => {
                self.submit_view_project(values).await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::DeleteProjectPicker,
                values,
                ..
            } => {
                self.submit_delete_project_picker(values);
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::SwitchWorkspace,
                values,
                ..
            } => {
                self.submit_switch_workspace(values).await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::ConflictField,
                values,
                ..
            } => {
                self.submit_conflict_field_picker(values).await?;
            }
            OverlaySubmit::Confirm {
                route: OverlayRoute::ConflictConfirm,
                ..
            } => {
                self.submit_confirmed_conflict_resolution().await?;
            }
            OverlaySubmit::Confirm {
                route: OverlayRoute::ConfigInit,
                ..
            } => {
                self.submit_config_init()?;
            }
            OverlaySubmit::Confirm {
                route: OverlayRoute::DeleteProjectConfirm,
                ..
            } => {
                self.submit_delete_project().await?;
            }
            OverlaySubmit::Confirm {
                route: OverlayRoute::DeleteTaskConfirm,
                ..
            } => {
                let return_to_detail = self.detail_context;
                self.update_deleted(true).await?;
                self.detail_context = false;
                self.restore_detail_overlay(return_to_detail);
            }
            OverlaySubmit::Text {
                route: OverlayRoute::ConflictManual,
                value,
                ..
            } => {
                self.submit_manual_conflict_value(value).await?;
            }
            OverlaySubmit::Multiline {
                route: OverlayRoute::ConflictManual,
                value,
                ..
            } => {
                self.submit_manual_conflict_value(value).await?;
            }
            OverlaySubmit::Picker {
                route: OverlayRoute::ConflictManual,
                values,
                ..
            } => {
                if let Some(value) = values.first() {
                    self.submit_manual_conflict_value(value.clone()).await?;
                } else {
                    self.set_warning("no value selected");
                }
            }
            other => self.set_success(other.message()),
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
            Action::ToggleHelp => self.toggle_help_at_height(24),
            Action::BeginSearch => self.begin_search(),
            Action::BeginCommand => self.begin_command(),
            Action::Refresh => self.refresh().await?,
            Action::CycleSort => {
                self.store.cycle_sort();
                self.refresh().await?;
                self.set_info(format!(
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
            Action::Delete => self.begin_delete_task(),
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
                self.set_warning(format!(":{name} is not yet implemented: {reason}"));
            }
            Action::Disabled { name, reason } => {
                self.set_warning(format!(":{name} is disabled: {reason}"));
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
            self.set_info("previous item is available in conflict flows");
        }
    }

    fn next_item(&mut self) {
        if matches!(self.store.active_view, SidebarTarget::Conflicts)
            || self.store.filters.conflicts_only
        {
            self.move_to_conflict(1);
        } else {
            self.set_info("next item is available in conflict flows");
        }
    }

    fn select_detail_task(&mut self, delta: isize) {
        let current = self.widgets.table.selected();
        let next = next_index(current, self.store.tasks.len(), delta, true);
        self.widgets.table.select(next);
        self.focus = Focus::Tasks;
        if current != next {
            let message = if delta > 0 {
                "selected next task"
            } else {
                "selected previous task"
            };
            self.set_info(message);
        }
    }

    async fn activate_or_toggle_detail(&mut self) -> Result<()> {
        if self.focus == Focus::Sidebar {
            self.apply_sidebar_selection().await?;
        } else if matches!(self.overlay, Some(OverlayState::Detail { .. })) {
            self.overlay = None;
        } else {
            self.overlay = Some(OverlayState::Detail { scroll: 0 });
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
            OverlayRoute::AddProject,
            ADD_PROJECT_TITLE,
            "project name:",
        )));
    }

    fn begin_add_label(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::TextInput(TextInputState::blank(
            OverlayRoute::AddLabel,
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
        self.authoring
            .begin_add_task(active_project, inferred_project);
        self.begin_add_task_title();
        Ok(())
    }

    fn begin_add_task_title(&mut self) {
        self.begin_add_task_overlay();
    }

    fn begin_add_task_overlay(&mut self) {
        let Some(context) = self.authoring.add_task_context() else {
            return;
        };
        self.overlay = Some(OverlayState::AddTask(AddTaskState {
            title: LineEdit::new(context.title),
            description: MultilineInputState::from_value(
                OverlayRoute::AddTaskDescription,
                "Add task: description",
                "",
                context.description,
            ),
            focus: context.step,
            project: context.project,
            priority: context.priority,
        }));
    }

    fn begin_add_task_step(&mut self) {
        self.begin_add_task_overlay();
    }

    fn open_add_task_description_editor(&mut self) {
        let Some(context) = self.authoring.add_task_context() else {
            return;
        };
        self.needs_terminal_clear = true;
        match edit_text_externally(context.description.clone(), "description.md") {
            Ok(value) => {
                self.authoring.capture_add_task_fields(
                    context.title,
                    value,
                    AddTaskStep::Description,
                );
                self.begin_add_task_step();
            }
            Err(error) => {
                self.set_error(format!("editor failed: {error:#}"));
                self.begin_add_task_step();
            }
        }
    }

    fn capture_add_task_state(&mut self, state: &AddTaskState) -> bool {
        self.authoring.capture_add_task_fields(
            state.title.text.clone(),
            state.description.lines.join("\n"),
            state.focus,
        )
    }

    async fn submit_add_task_from_authoring(&mut self) -> Result<()> {
        match self.authoring.submit_add_task() {
            AddTaskTitleSubmit::ReopenTitle { message } => {
                self.set_warning(message);
                self.begin_add_task_title();
            }
            AddTaskTitleSubmit::Create(draft) => {
                self.submit_created_task(draft).await?;
            }
            AddTaskTitleSubmit::Inactive => {}
        }
        Ok(())
    }

    fn begin_add_note(&mut self) {
        self.pending_shortcut.clear();
        let Some(item) = self
            .store
            .selected_task(self.widgets.table.selected())
            .cloned()
        else {
            self.set_info("no selected task for note");
            return;
        };
        let return_to_detail =
            self.detail_context || matches!(self.overlay, Some(OverlayState::Detail { .. }));
        self.authoring.begin_add_note(
            item.task.id.clone(),
            item.display_ref.clone(),
            return_to_detail,
        );
        self.detail_context = return_to_detail;
        self.overlay = Some(OverlayState::MultilineInput(MultilineInputState::blank(
            OverlayRoute::AddNote,
            ADD_NOTE_TITLE,
            "note body:",
        )));
    }

    fn begin_add_task_title_project(&mut self) {
        let Some(selected) = self.authoring.selected_add_task_project() else {
            return;
        };
        let items = self.store.project_picker_items(selected.as_deref());
        self.open_picker_overlay(
            OverlayRoute::AddTaskTitleProject,
            ADD_TASK_TITLE_PROJECT_TITLE,
            items,
            false,
        );
    }

    fn begin_add_task_title_priority(&mut self) {
        let Some(selected) = self.authoring.selected_add_task_priority() else {
            return;
        };
        let items = self.store.priority_picker_items(&selected);
        self.open_picker_overlay(
            OverlayRoute::AddTaskTitlePriority,
            ADD_TASK_TITLE_PRIORITY_TITLE,
            items,
            false,
        );
    }

    async fn submit_created_task(&mut self, draft: TaskDraft) -> Result<()> {
        let current_selected = self.widgets.table.selected();
        let (message, selected) = self.store.create_task(draft, current_selected).await?;
        self.widgets.table.select(selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        if selected.is_none() {
            self.restore_selection_after_mutation();
        }
        self.set_success(message);
        Ok(())
    }

    async fn submit_add_note(&mut self, body: String) -> Result<()> {
        match self.authoring.submit_add_note(body) {
            AddNoteSubmit::Create {
                task_id,
                display_ref,
                body,
                return_to_detail,
            } => {
                let note_id = self.store.add_note_to_task(&task_id, body).await?;
                self.refresh().await?;
                self.restore_detail_overlay(return_to_detail);
                self.set_success(format!("added note {note_id} to {display_ref}"));
            }
            AddNoteSubmit::Blank {
                return_to_detail,
                message,
            } => {
                self.restore_detail_overlay(return_to_detail);
                self.set_warning(message);
            }
            AddNoteSubmit::Inactive { message } => {
                self.set_info(message);
            }
        }
        Ok(())
    }

    fn cancel_authoring_overlay(&mut self) {
        self.pending_shortcut.clear();
        let return_to_detail = self.authoring.cancel() || self.detail_context;
        self.overlay = None;
        self.conflict_flow.clear();
        self.pending_delete_project = None;
        self.detail_context = false;
        self.restore_detail_overlay(return_to_detail);
    }

    fn restore_detail_overlay(&mut self, return_to_detail: bool) {
        self.restore_detail_overlay_at_scroll(return_to_detail, 0);
    }

    fn restore_detail_overlay_at_scroll(&mut self, return_to_detail: bool, scroll: u16) {
        if return_to_detail
            && self
                .store
                .selected_task(self.widgets.table.selected())
                .is_some()
        {
            self.detail_context = false;
            self.overlay = Some(OverlayState::Detail { scroll });
        }
    }

    fn accept_command_input(&mut self, input: &str) -> Option<Action> {
        match lookup_command(input) {
            CommandLookup::Found(action) => {
                self.pending_shortcut.clear();
                Some(action)
            }
            CommandLookup::Empty => {
                self.set_info("empty command");
                None
            }
            CommandLookup::Ambiguous => {
                self.set_warning(format!("ambiguous command: {}", input.trim()));
                None
            }
            CommandLookup::Missing => {
                self.set_warning(format!("unknown command: {}", input.trim()));
                None
            }
        }
    }

    fn toggle_help_at_height(&mut self, _terminal_height: u16) {
        match self.overlay {
            Some(OverlayState::Help { .. }) => self.overlay = None,
            Some(OverlayState::DetailHelp { .. }) => {
                self.overlay = Some(OverlayState::Detail { scroll: 0 })
            }
            Some(OverlayState::Detail { .. }) => {
                self.overlay = Some(OverlayState::DetailHelp { scroll: 0 })
            }
            _ => self.overlay = Some(OverlayState::Help { scroll: 0 }),
        }
    }

    fn cancel_overlay(&mut self) {
        self.pending_shortcut.clear();
        self.authoring.clear();
        self.conflict_flow.clear();
        self.pending_delete_project = None;
        let had_overlay = self.overlay.take().is_some();
        self.detail_context = false;
        if !had_overlay && self.focus == Focus::Sidebar {
            self.focus = Focus::Tasks;
            self.widgets.sidebar.select(self.store.sidebar_selection());
        }
    }

    async fn set_sort(&mut self, sort: TaskSort) -> Result<()> {
        let selected = self.store.set_sort(sort).await?;
        self.apply_filter_selection(selected);
        self.set_info(format!(
            "order {} {}",
            self.store.sort_label(),
            self.store.sort_direction_label()
        ));
        Ok(())
    }

    async fn reverse_sort(&mut self) -> Result<()> {
        let selected = self.store.reverse_sort().await?;
        self.apply_filter_selection(selected);
        self.set_info(format!(
            "order {} {}",
            self.store.sort_label(),
            self.store.sort_direction_label()
        ));
        Ok(())
    }

    pub(super) fn apply_mutation_result(&mut self, result: crate::tui::store::MutationMessage) {
        self.widgets.table.select(result.selected);
        self.widgets.sidebar.select(self.store.sidebar_selection());
        self.set_success(result.message);
    }

    pub(super) fn open_picker_overlay(
        &mut self,
        route: OverlayRoute,
        title: &str,
        items: Vec<PickerItem>,
        multi: bool,
    ) {
        self.overlay = Some(OverlayState::Picker(PickerState {
            route,
            title: title.to_string(),
            filter: LineEdit::blank(),
            selected: selected_picker_index(&items),
            items,
            multi,
            mode: PickerMode::Navigate,
        }));
    }

    pub(super) fn require_picker_value(
        &mut self,
        values: Vec<String>,
        message: &str,
    ) -> Option<String> {
        match values.first().cloned() {
            Some(value) => Some(value),
            None => {
                self.set_warning(message);
                None
            }
        }
    }

    fn begin_delete_task(&mut self) {
        self.pending_shortcut.clear();
        let Some(task) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_info("no selected task to edit");
            return;
        };
        self.detail_context =
            self.detail_context || matches!(self.overlay, Some(OverlayState::Detail { .. }));
        self.overlay = Some(OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::DeleteTaskConfirm,
            title: DELETE_TASK_TITLE.to_string(),
            prompt: format!("Delete {} {}?", task.display_ref, task.task.title),
        }));
    }

    fn begin_delete_project(&mut self) {
        self.pending_shortcut.clear();
        let selected = if self.focus == Focus::Sidebar {
            self.selected_sidebar_project()
        } else {
            None
        };
        let items = self
            .store
            .existing_project_picker_items(selected.as_deref().unwrap_or_default());
        self.open_picker_overlay(
            OverlayRoute::DeleteProjectPicker,
            DELETE_PROJECT_TITLE,
            items,
            false,
        );
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

    fn copy_selected_ref(&mut self, kind: TaskRefKind) {
        let Some(task) = self.store.selected_task(self.widgets.table.selected()) else {
            self.set_info("no selected task to copy");
            return;
        };
        let (value, message_ref) = match kind {
            TaskRefKind::Short => (task.display_ref.clone(), task.display_ref.clone()),
            TaskRefKind::Durable => (task.task.id.clone(), task.display_ref.clone()),
        };
        match copy_to_clipboard(&value) {
            Ok(()) => self.set_success(format!("copied {message_ref}")),
            Err(error) => self.set_error(format!("copy failed: {error}")),
        }
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

    pub(super) fn set_info(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Info);
    }

    pub(super) fn set_warning(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Warning);
    }

    pub(super) fn set_error(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Error);
    }

    pub(super) fn set_success(&mut self, message: impl Into<String>) {
        self.set_toast(message, ToastSeverity::Success);
    }

    fn set_toast(&mut self, message: impl Into<String>, severity: ToastSeverity) {
        self.message = Some(Toast::new(message, severity));
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

    fn show_config_status(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_status_overlay(&self.store)?);
        Ok(())
    }

    fn show_config_info(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_info_overlay(&self.store)?);
        Ok(())
    }

    fn show_config_paths(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_paths_overlay(&self.store)?);
        Ok(())
    }

    fn begin_config_init(&mut self) -> Result<()> {
        self.pending_shortcut.clear();
        self.overlay = Some(config_init_overlay()?);
        Ok(())
    }

    fn submit_config_init(&mut self) -> Result<()> {
        let message = self.store.init_config()?;
        self.set_success(message);
        Ok(())
    }

    fn open_description_external_editor(&mut self, state: MultilineInputState) {
        self.needs_terminal_clear = true;
        match edit_text_externally(state.lines.join("\n"), "description.md") {
            Ok(value) => self.overlay = Some(description_overlay_from_value(value)),
            Err(error) => {
                self.set_error(format!("editor failed: {error:#}"));
                self.overlay = Some(OverlayState::MultilineInput(state));
            }
        }
    }

    fn submit_delete_project_picker(&mut self, values: Vec<String>) {
        let Some(project) = self.require_picker_value(values, "no matching project") else {
            self.begin_delete_project();
            return;
        };
        self.pending_delete_project = Some(project.clone());
        self.overlay = Some(OverlayState::Confirm(ConfirmState {
            route: OverlayRoute::DeleteProjectConfirm,
            title: DELETE_PROJECT_TITLE.to_string(),
            prompt: format!("Delete project {project}?"),
        }));
    }

    async fn submit_delete_project(&mut self) -> Result<()> {
        let Some(project) = self.pending_delete_project.take() else {
            self.set_warning("project delete confirmation is not active");
            return Ok(());
        };
        match self.store.delete_project(&project).await {
            Ok(result) => self.apply_mutation_result(result),
            Err(error) => self.set_error(format!("{error:#}")),
        }
        Ok(())
    }
}

fn is_editor_prefix_key(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x')
}

fn selected_picker_index(items: &[PickerItem]) -> usize {
    items.iter().position(|item| item.selected).unwrap_or(0)
}

fn description_overlay_from_value(value: String) -> OverlayState {
    OverlayState::MultilineInput(MultilineInputState::from_value(
        OverlayRoute::EditDescription,
        "Edit description",
        "",
        value,
    ))
}

#[cfg(not(test))]
fn edit_text_externally(value: String, filename: &str) -> Result<String> {
    let path = temp_editor_path(filename)?;
    fs::write(&path, value)?;
    let result =
        run_external_editor(&path).and_then(|()| fs::read_to_string(&path).map_err(Into::into));
    let _ = fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir(parent);
    }
    result
}

#[cfg(test)]
fn edit_text_externally(value: String, _filename: &str) -> Result<String> {
    Ok(format!("{value} from editor"))
}

#[cfg(not(test))]
fn temp_editor_path(filename: &str) -> io::Result<std::path::PathBuf> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("aven-tui-editor-{pid}-{millis}"));
    fs::create_dir(&dir)?;
    Ok(dir.join(filename))
}

#[cfg(not(test))]
fn run_external_editor(path: &std::path::Path) -> Result<()> {
    let restore = suspend_terminal()?;
    let status = external_editor_command(path).status();
    restore()?;
    let status = status?;
    if !status.success() {
        anyhow::bail!("editor exited with {status}");
    }
    Ok(())
}

#[cfg(not(test))]
fn external_editor_command(path: &std::path::Path) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("exec ${VISUAL:-${EDITOR:-vi}} \"$1\"")
        .arg("sh")
        .arg(path);
    command
}

#[cfg(not(test))]
fn suspend_terminal() -> Result<impl FnOnce() -> Result<()>> {
    disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(|| {
        crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
        enable_raw_mode()?;
        Ok(())
    })
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

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
