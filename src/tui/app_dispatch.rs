use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Size;

use crate::tui::app::{App, Focus, TaskRefKind};
use crate::tui::authoring::AddTaskStep;
use crate::tui::conflict_flow::ConflictResolutionChoice;
use crate::tui::event::{
    Action, CommandCompletion, CommandLookup, command_cycle_options, complete_command,
    lookup_command,
};
use crate::tui::navigation::{detail_task_delta, handle_detail_overlay_key};
use crate::tui::overlay::{CommandState, OverlayOutcome, OverlayRoute, OverlayState};
use crate::tui::platform::is_editor_prefix_key;
use crate::tui::shortcut_buffer::{DetailShortcutResolution, NormalShortcutResolution};
use crate::tui::ui::{database_stats_scroll_cap, detail_help_scroll_cap, help_scroll_cap};

impl App {
    pub(super) fn dispatch_paste(&mut self, text: &str) {
        let Some(overlay) = self.overlay.take() else {
            return;
        };
        self.overlay = Some(crate::tui::overlay::handle_generic_overlay_paste(
            text, overlay,
        ));
    }

    pub(crate) async fn dispatch_key(&mut self, key: KeyEvent, terminal_size: Size) -> Result<()> {
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.handle(Action::Quit).await
        } else if key.code == KeyCode::Esc && self.pending_shortcut.cancel() {
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
        } else if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT {
            self.handle_normal_key(key.code).await
        } else {
            Ok(())
        }
    }

    pub(crate) async fn dispatch_mouse(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<()> {
        match mouse.kind {
            MouseEventKind::ScrollDown => {
                return self.handle_task_list_wheel(1, terminal_size).await;
            }
            MouseEventKind::ScrollUp => {
                return self.handle_task_list_wheel(-1, terminal_size).await;
            }
            MouseEventKind::Down(MouseButton::Left) => {}
            _ => return Ok(()),
        }

        if matches!(self.overlay, Some(OverlayState::HeaderMenu(_))) {
            let Some(OverlayState::HeaderMenu(state)) = self.overlay.take() else {
                return Ok(());
            };
            return self
                .submit_header_menu_at(state, mouse.column, mouse.row, terminal_size)
                .await;
        }
        if matches!(self.overlay, Some(OverlayState::OrderMenu(_))) {
            let Some(OverlayState::OrderMenu(state)) = self.overlay.take() else {
                return Ok(());
            };
            return self
                .submit_order_menu_at(state, mouse.column, mouse.row, terminal_size)
                .await;
        }
        if self.overlay.is_some() || terminal_size.width < 70 || terminal_size.height < 18 {
            return Ok(());
        }
        let header = ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: terminal_size.width,
            height: 2,
        };
        match crate::tui::ui::header_target_at(&self.store, header, mouse.column, mouse.row) {
            Some(crate::tui::ui::HeaderTarget::Workspace { column }) => {
                self.show_workspace_menu(column, mouse.row).await?
            }
            Some(crate::tui::ui::HeaderTarget::Scope { column }) => {
                self.show_scope_menu(column, mouse.row)
            }
            Some(crate::tui::ui::HeaderTarget::View { column }) => {
                self.show_view_menu(column, mouse.row)
            }
            Some(crate::tui::ui::HeaderTarget::MetricView(view)) => self.show_view(view).await?,
            Some(crate::tui::ui::HeaderTarget::Order { column }) => {
                self.show_order_menu(column, mouse.row)
            }
            Some(crate::tui::ui::HeaderTarget::SyncStatus) => self.show_config_status()?,
            None => {}
        }
        Ok(())
    }

    async fn handle_task_list_wheel(&mut self, delta: isize, terminal_size: Size) -> Result<()> {
        if self.overlay.is_some()
            || terminal_size.width < 70
            || terminal_size.height < 18
            || self.detail_underlay()
            || self.focus != Focus::Tasks
        {
            return Ok(());
        }

        self.move_selection(delta).await
    }

    fn overlay_captures_input(&self) -> bool {
        self.overlay
            .as_ref()
            .is_some_and(OverlayState::captures_input)
    }

    pub(crate) async fn handle_normal_key(&mut self, code: KeyCode) -> Result<()> {
        if self.overlay_captures_input()
            && (code != KeyCode::Esc || self.pending_shortcut.is_empty())
        {
            return self
                .handle_overlay_key(KeyEvent::new(code, KeyModifiers::NONE))
                .await;
        }

        if code == KeyCode::Esc {
            if !self.pending_shortcut.cancel() {
                self.handle(Action::CancelOverlay).await?;
            }
            return Ok(());
        }

        match self.pending_shortcut.resolve_normal(code) {
            NormalShortcutResolution::Action(action) => {
                self.handle(action).await?;
            }
            NormalShortcutResolution::Prefix => {}
            NormalShortcutResolution::Missing(label) => {
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
            OverlayState::Command { mut state } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Some(action) = self.accept_command_input(state.input.as_str()) {
                        self.execute(action).await?;
                    } else {
                        self.overlay = Some(OverlayState::Command { state });
                    }
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    self.complete_command_input(&mut state, key.code == KeyCode::BackTab);
                    self.overlay = Some(OverlayState::Command { state });
                }
                _ => {
                    state.input.handle_key(key);
                    state.reset_cycle();
                    self.overlay = Some(OverlayState::Command { state });
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

        let had_add_task_status_prefix = self.pending_shortcut.has_add_task_status_prefix();
        if let Some(status) = self.pending_shortcut.take_add_task_status_request(key) {
            if let OverlayState::AddTask(state) = &overlay {
                if self.capture_add_task_state(state) {
                    self.overlay = Some(overlay);
                    self.set_add_task_status(status);
                }
            } else {
                self.overlay = Some(overlay);
            }
            return Ok(());
        }
        if had_add_task_status_prefix {
            self.pending_shortcut.clear();
            self.overlay = Some(overlay);
            if key.code != KeyCode::Esc {
                self.set_warning("invalid status shortcut");
            }
            return Ok(());
        }

        let had_add_task_priority_prefix = self.pending_shortcut.has_add_task_priority_prefix();
        if let Some(priority) = self.pending_shortcut.take_add_task_priority_request(key) {
            if let OverlayState::AddTask(state) = &overlay {
                if self.capture_add_task_state(state) {
                    self.overlay = Some(overlay);
                    self.set_add_task_priority(priority);
                }
            } else {
                self.overlay = Some(overlay);
            }
            return Ok(());
        }
        if had_add_task_priority_prefix {
            self.pending_shortcut.clear();
            self.overlay = Some(overlay);
            if key.code != KeyCode::Esc {
                self.set_warning("invalid priority shortcut");
            }
            return Ok(());
        }

        if self.pending_shortcut.take_editor_open_request(key) {
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

        if is_editor_prefix_key(key)
            && matches!(
                &overlay,
                OverlayState::MultilineInput(state)
                    if state.route == OverlayRoute::EditDescription
            )
        {
            self.pending_shortcut.begin_editor_prefix();
            self.overlay = Some(overlay);
            return Ok(());
        }

        if let OverlayState::AddTask(state) = &overlay {
            if is_editor_prefix_key(key) {
                if state.focus == AddTaskStep::Description {
                    self.pending_shortcut.begin_editor_prefix();
                }
                self.overlay = Some(overlay);
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p') {
                if self.capture_add_task_state(state) {
                    self.begin_add_task_title_project();
                }
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('r') {
                self.pending_shortcut.begin_add_task_priority_prefix();
                self.overlay = Some(overlay);
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('t') {
                self.pending_shortcut.begin_add_task_status_prefix();
                self.overlay = Some(overlay);
                return Ok(());
            }
            if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('n') {
                let title = state.title.text.clone();
                let description = state.description.lines.join("\n");
                if self.capture_add_task_state(state) {
                    self.submit_add_task_title_natural(title, description)
                        .await?;
                }
                return Ok(());
            }
        }

        let scroll_cap = match overlay {
            OverlayState::DetailHelp { .. } => detail_help_scroll_cap(terminal_size.height),
            OverlayState::DatabaseStats { .. } => database_stats_scroll_cap(terminal_size.height),
            _ => help_scroll_cap(terminal_size.height),
        };
        let was_detail_help = matches!(overlay, OverlayState::DetailHelp { .. });
        let was_add_task_description_editor = matches!(
            &overlay,
            OverlayState::MultilineInput(state) if state.route == OverlayRoute::AddTaskDescription
        );
        let was_add_task_picker = matches!(
            &overlay,
            OverlayState::Picker(state)
                if matches!(
                    state.route,
                    OverlayRoute::AddTaskTitleProject | OverlayRoute::AddTaskTitlePriority
                )
        );
        let outcome = crate::tui::overlay::handle_generic_overlay_key(key, overlay, scroll_cap);
        match outcome {
            OverlayOutcome::None(overlay) => self.overlay = Some(overlay),
            OverlayOutcome::Cancelled if was_detail_help => {
                self.overlay = Some(OverlayState::Detail { scroll: 0 })
            }
            OverlayOutcome::Cancelled if was_add_task_description_editor || was_add_task_picker => {
                self.begin_add_task_step()
            }
            OverlayOutcome::Cancelled if self.add_task_only => self.should_quit = true,
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

        match self.pending_shortcut.resolve_detail(key) {
            DetailShortcutResolution::Action(action) => {
                self.detail_context = true;
                self.execute(action).await?;
                if self.detail_context && self.overlay.is_none() {
                    self.restore_detail_overlay_at_scroll(true, scroll);
                }
                Ok(Some(self.overlay.take()))
            }
            DetailShortcutResolution::Prefix => Ok(Some(Some(OverlayState::Detail { scroll }))),
            DetailShortcutResolution::MissingAfterPrefix(label) => {
                self.set_warning(format!("invalid shortcut: {label}"));
                Ok(Some(Some(OverlayState::Detail { scroll })))
            }
            DetailShortcutResolution::PassThrough => Ok(None),
        }
    }

    async fn handle(&mut self, action: Action) -> Result<()> {
        self.execute(action).await
    }

    pub(super) async fn execute(&mut self, action: Action) -> Result<()> {
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
            Action::SetOrder(order) => self.set_sort(order).await?,
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
            Action::BeginRenameProject => self.begin_rename_project(),
            Action::BeginDeleteProject => self.begin_delete_project(),
            Action::BeginAddProject => self.begin_add_project(),
            Action::BeginAddLabel => self.begin_add_label(),
            Action::BeginAddTask => self.begin_add_task().await?,
            Action::BeginAddNote => self.begin_add_note(),
            Action::BeginFilterLabel => self.begin_filter_label(),
            Action::BeginFilterPriority => self.begin_filter_priority(),
            Action::BeginScopeProject => self.begin_scope_project(),
            Action::BeginSwitchWorkspace => self.begin_switch_workspace().await?,
            Action::ClearFilters => self.clear_filters().await?,
            Action::ToggleDeletedFilter => self.toggle_deleted_filter().await?,
            Action::ShowView(view) => self.show_view(view).await?,
            Action::ShowWorkspaceScope => {
                self.show_scope(crate::tui::store::TaskScopeTarget::Workspace)
                    .await?
            }
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
            Action::ShowDatabaseStats => self.show_database_stats().await?,
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

    fn complete_command_input(&mut self, state: &mut CommandState, reverse: bool) {
        let cycle_input = state
            .cycle_input
            .clone()
            .unwrap_or_else(|| state.input.text.clone());
        let options = command_cycle_options(&cycle_input);
        if options.len() > 1 {
            state.cycle_index = if state.cycle_input.is_some() {
                if reverse {
                    state
                        .cycle_index
                        .checked_sub(1)
                        .unwrap_or(options.len().saturating_sub(1))
                } else {
                    (state.cycle_index + 1) % options.len()
                }
            } else if reverse {
                options.len().saturating_sub(1)
            } else {
                0
            };
            state.cycle_input = Some(cycle_input);
            let completion = options[state.cycle_index].to_string();
            state.input.text = completion;
            state.input.cursor = state.input.text.len();
            state.highlighted = Some(state.input.text.clone());
            self.set_info(format!(
                "command {} of {}",
                state.cycle_index + 1,
                options.len()
            ));
            return;
        }

        let highlighted = state.highlighted.clone();
        state.reset_cycle();
        state.highlighted = highlighted;
        match complete_command(state.input.as_str()) {
            CommandCompletion::Completed(completion) => {
                state.input.text = completion;
                state.input.cursor = state.input.text.len();
                state.highlighted = Some(state.input.text.clone());
            }
            CommandCompletion::Empty => self.set_info("type a command prefix to complete"),
            CommandCompletion::Missing => self.set_warning(format!(
                "no command matches: {}",
                state.input.as_str().trim()
            )),
            CommandCompletion::Unchanged => self.set_info("no further completion"),
        }
    }

    pub(super) fn toggle_help_at_height(&mut self, _terminal_height: u16) {
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
}
