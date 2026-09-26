use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseEvent};
use ratatui::layout::Size;

use crate::tui::app::App;
use crate::tui::authoring::AddTaskStep;
use crate::tui::overlay::{
    AddTaskMode, MultilineIntent, OverlayOutcome, OverlayState, PairingOverlay, PickerIntent,
    TagComboboxIntent,
};
use crate::tui::platform::{is_editor_prefix_key, open_url_in_default_browser};
use crate::tui::ui::{
    composer_help_scroll_cap, database_stats_scroll_cap, detail_help_scroll_cap, help_scroll_cap,
};

impl App {
    pub(super) async fn handle_overlay_key_at_size(
        &mut self,
        key: KeyEvent,
        terminal_size: Size,
    ) -> Result<()> {
        let overlay = if let Some(overlay) = self.overlay.take() {
            overlay
        } else if self.detail.is_active() {
            OverlayState::Detail
        } else {
            return Ok(());
        };

        match overlay {
            OverlayState::Onboarding { persist_on_exit } => {
                self.handle_onboarding_key(key, terminal_size, persist_on_exit)
                    .await?
            }
            OverlayState::Search(state) => self.handle_search_key(state, key).await?,
            OverlayState::RecurrenceHistory(state) => {
                self.handle_recurrence_history_key(*state, key).await?
            }
            OverlayState::Update(state) => {
                self.handle_update_overlay_key(state, key, terminal_size)
                    .await
            }
            OverlayState::Changelog(state) => self.handle_changelog_key(state, key, terminal_size),
            OverlayState::Pairing(page @ PairingOverlay::Ready(_))
                if key.code == KeyCode::Char('c') && key.modifiers.is_empty() =>
            {
                self.overlay = Some(OverlayState::Pairing(page));
                self.copy_pairing_invitation();
            }
            OverlayState::Pairing(PairingOverlay::Failed(_)) if key.code == KeyCode::Enter => {
                self.show_pairing_invitation();
            }
            OverlayState::Pairing(PairingOverlay::Failed(_)) if key.code == KeyCode::Esc => {
                self.show_sync_dialog();
            }
            OverlayState::Sync(state) => {
                self.handle_sync_dialog_key(state, key, terminal_size)
                    .await?
            }
            OverlayState::Command { mut state } => match key.code {
                KeyCode::Esc => {}
                KeyCode::Enter => {
                    if let Some(command) = state
                        .highlighted
                        .and_then(|row| state.candidates.get(row))
                        .and_then(|candidate| state.catalog.command(candidate.index))
                    {
                        state.input.text = command.name().to_string();
                        state.input.cursor = state.input.text.len();
                    }
                    if !self.accept_command_input(&state).await? {
                        self.overlay = Some(OverlayState::Command { state });
                    }
                }
                KeyCode::Down | KeyCode::Up => {
                    self.move_command_selection(&mut state, key.code == KeyCode::Up);
                    self.overlay = Some(OverlayState::Command { state });
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    self.complete_command_input(&mut state, key.code == KeyCode::BackTab);
                    self.overlay = Some(OverlayState::Command { state });
                }
                _ => {
                    state.input.handle_key(key);
                    state.reset_cycle();
                    state.refresh_candidates();
                    self.overlay = Some(OverlayState::Command { state });
                }
            },
            OverlayState::Detail => self.handle_detail_overlay_key(key, terminal_size).await?,
            overlay => {
                self.handle_generic_overlay_key(key, overlay, terminal_size)
                    .await?
            }
        }

        Ok(())
    }

    pub(super) async fn dispatch_overlay_mouse(
        &mut self,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<()> {
        let detail_focus = self
            .detail
            .state()
            .and_then(|detail| detail.focused_target())
            .cloned();
        let context = crate::tui::overlay::OverlayMouseContext {
            add_task_only: self.intake.view().add_task_only,
            detail_help_scroll_cap: detail_help_scroll_cap(
                terminal_size.height,
                detail_focus.as_ref(),
            ),
        };
        let Some(overlay) = self.overlay.take() else {
            return Ok(());
        };
        let overlay = match overlay {
            OverlayState::Sync(state) => {
                return self
                    .handle_sync_dialog_mouse(state, mouse, terminal_size)
                    .await;
            }
            overlay => overlay,
        };
        let was_add_task_picker = matches!(
            &overlay,
            OverlayState::Picker(state)
                if matches!(
                    state.intent,
                    PickerIntent::AddTaskProject | PickerIntent::AddTaskPriority
                )
        ) || matches!(
            &overlay,
            OverlayState::TagCombobox(state)
                if state.intent == TagComboboxIntent::AddTaskLabels
        );
        let outcome =
            crate::tui::overlay::dispatch_overlay_mouse(overlay, mouse, terminal_size, context);
        match outcome {
            crate::tui::overlay::OverlayMouseOutcome::Retained(overlay) => {
                self.overlay = Some(overlay)
            }
            crate::tui::overlay::OverlayMouseOutcome::Closed => {}
            crate::tui::overlay::OverlayMouseOutcome::Cancelled => {
                self.apply_generic_overlay_outcome(
                    OverlayOutcome::Cancelled,
                    false,
                    false,
                    was_add_task_picker,
                )
                .await?;
            }
            crate::tui::overlay::OverlayMouseOutcome::Submitted(submit) => {
                self.handle_overlay_submit(submit).await?;
            }
            crate::tui::overlay::OverlayMouseOutcome::OpenUrl {
                overlay,
                url,
                error_context,
            } => {
                self.overlay = Some(overlay);
                if let Err(error) = open_url_in_default_browser(&url) {
                    self.set_warning(format!("{error_context}: {error:#}"));
                }
            }
            crate::tui::overlay::OverlayMouseOutcome::OpenAddTaskControl(overlay) => {
                self.overlay = Some(overlay);
                self.open_focused_add_task_control();
            }
            crate::tui::overlay::OverlayMouseOutcome::UpdateAction(state) => {
                self.handle_update_overlay_key(
                    state,
                    KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
                    terminal_size,
                )
                .await;
            }
            crate::tui::overlay::OverlayMouseOutcome::RecurrenceHistoryAction { state, action } => {
                self.run_recurrence_history_action(state, action).await?
            }
            crate::tui::overlay::OverlayMouseOutcome::Warning { overlay, message } => {
                self.overlay = Some(overlay);
                self.set_warning(message);
            }
        }
        Ok(())
    }

    pub(super) async fn handle_generic_overlay_key(
        &mut self,
        key: KeyEvent,
        overlay: OverlayState,
        terminal_size: Size,
    ) -> Result<()> {
        if let OverlayState::AttachmentPreview {
            attachment_id,
            scroll,
        } = overlay
        {
            self.handle_attachment_preview_key(key, attachment_id, scroll)
                .await?;
            return Ok(());
        }

        let Some(overlay) = self.handle_add_task_overlay_prefix_key(key, overlay)? else {
            return Ok(());
        };

        if self.pending_shortcut.take_editor_open_request(key) {
            match overlay {
                OverlayState::Metadata(state) => {
                    self.open_metadata_external_editor(state);
                }
                OverlayState::MultilineInput(state) if state.intent.is_description_edit() => {
                    self.open_description_external_editor(state);
                }
                OverlayState::MultilineInput(state)
                    if matches!(state.intent, MultilineIntent::EditNote { .. }) =>
                {
                    self.open_note_external_editor(state);
                }
                OverlayState::AddTask(state) if state.focus == AddTaskStep::Description => {
                    if self.capture_add_task_state(&state) {
                        self.open_add_task_description_editor();
                    }
                }
                overlay => self.overlay = Some(overlay),
            }
            return Ok(());
        }

        if is_editor_prefix_key(key)
            && matches!(&overlay, OverlayState::Metadata(state) if state.editor.as_ref().is_some_and(|editor| !editor.discard))
        {
            self.pending_shortcut.begin_editor_prefix();
            self.overlay = Some(overlay);
            return Ok(());
        }

        if is_editor_prefix_key(key)
            && matches!(
                &overlay,
                OverlayState::MultilineInput(state)
                    if state.intent.supports_external_editor()
            )
        {
            self.pending_shortcut.begin_editor_prefix();
            self.overlay = Some(overlay);
            return Ok(());
        }

        let Some(overlay) = self.handle_add_task_overlay_tail(key, overlay).await? else {
            return Ok(());
        };

        let scroll_cap = match &overlay {
            OverlayState::AddTask(state) if matches!(state.mode, AddTaskMode::Help { .. }) => {
                composer_help_scroll_cap(
                    terminal_size.height,
                    self.intake.view().add_task_only,
                    state.schedule_expanded,
                )
            }
            OverlayState::DetailHelp { .. } => detail_help_scroll_cap(
                terminal_size.height,
                self.detail
                    .state()
                    .and_then(|detail| detail.focused_target()),
            ),
            OverlayState::DatabaseStats { .. } => database_stats_scroll_cap(terminal_size.height),
            OverlayState::Changelog(state) => {
                crate::tui::changelog::changelog_scroll_cap(&state.markdown, terminal_size)
            }
            _ => help_scroll_cap(terminal_size.height),
        };
        let was_detail_help = matches!(overlay, OverlayState::DetailHelp { .. });
        let was_add_task_description_editor = matches!(
            &overlay,
            OverlayState::MultilineInput(state)
                if state.intent == MultilineIntent::AddTaskDescription
        );
        let was_add_task_picker = matches!(
            &overlay,
            OverlayState::Picker(state)
                if matches!(
                    state.intent,
                    PickerIntent::AddTaskProject | PickerIntent::AddTaskPriority
                )
        ) || matches!(
            &overlay,
            OverlayState::TagCombobox(state)
                if state.intent == TagComboboxIntent::AddTaskLabels
        );
        let outcome = crate::tui::overlay::handle_generic_overlay_key(key, overlay, scroll_cap);
        self.apply_generic_overlay_outcome(
            outcome,
            was_detail_help,
            was_add_task_description_editor,
            was_add_task_picker,
        )
        .await
    }

    pub(super) async fn apply_generic_overlay_outcome(
        &mut self,
        outcome: OverlayOutcome,
        was_detail_help: bool,
        was_add_task_description_editor: bool,
        was_add_task_picker: bool,
    ) -> Result<()> {
        match outcome {
            OverlayOutcome::None(overlay) => self.overlay = Some(overlay),
            OverlayOutcome::Cancelled if was_detail_help => {}
            OverlayOutcome::Cancelled if was_add_task_description_editor || was_add_task_picker => {
                self.begin_add_task_step()
            }
            OverlayOutcome::Cancelled if self.intake.view().add_task_only => {
                self.should_quit = true
            }
            OverlayOutcome::Cancelled => self.cancel_authoring_overlay(),
            OverlayOutcome::Submitted(submit) => self.handle_overlay_submit(submit).await?,
        }
        Ok(())
    }
}
