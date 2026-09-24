use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Size;

use crate::tui::app::App;
use crate::tui::event::Action;
use crate::tui::input::key::{
    ImagePasteTarget, KeyInput, KeyRouteState, NormalKeyInput, route_key,
    route_normal_key_in_domain,
};
use crate::tui::overlay::{MultilineIntent, OverlayState};

impl App {
    pub(super) async fn dispatch_paste(&mut self, text: &str) -> Result<()> {
        // Invitation text is secret; it must not reach any other paste target.
        if let Some(OverlayState::Sync(state)) = self
            .overlay
            .take_if(|overlay| matches!(overlay, OverlayState::Sync(_)))
        {
            self.handle_sync_dialog_paste(state, text);
            return Ok(());
        }
        if self.paste_detail_image_from_text(text).await? {
            return Ok(());
        }
        if self.paste_add_task_image_from_text(text)? {
            return Ok(());
        }
        if text.is_empty() && self.paste_image_from_empty_terminal_paste().await? {
            return Ok(());
        }
        let Some(overlay) = self.overlay.take() else {
            return Ok(());
        };
        let mut overlay = crate::tui::overlay::handle_generic_overlay_paste(text, overlay);
        if let OverlayState::Search(state) = &mut overlay {
            self.handle_search_paste(state).await?;
        }
        self.overlay = Some(overlay);
        Ok(())
    }

    pub(crate) async fn dispatch_key(&mut self, key: KeyEvent, terminal_size: Size) -> Result<()> {
        self.list.cancel_column_drag();
        let input = route_key(
            key,
            KeyRouteState {
                footer_choice: self.footer_choice.is_some(),
                shortcut_pending: !self.pending_shortcut.is_empty(),
                prefix_hints: self.prefix_hints_active(),
                overlay_captures: self.overlay_captures_input()
                    || (self.detail.is_active() && self.overlay.is_none()),
                detail_overlay: self.detail.is_active() && self.overlay.is_none(),
                add_task_image_target: matches!(
                    self.overlay,
                    Some(OverlayState::AddTask(_))
                        | Some(OverlayState::MultilineInput(
                            crate::tui::overlay::MultilineInputState {
                                intent: MultilineIntent::AddTaskNatural,
                                ..
                            }
                        ))
                ),
            },
            terminal_size.height,
        );
        match input {
            KeyInput::Action(action) => self.execute(action).await,
            KeyInput::PasteImage(ImagePasteTarget::Detail) => {
                self.paste_detail_image_from_clipboard().await
            }
            KeyInput::PasteImage(ImagePasteTarget::AddTask) => {
                self.paste_add_task_image_from_clipboard().await
            }
            KeyInput::FooterChoice(key) => self.handle_footer_choice_key(key).await,
            KeyInput::CancelShortcut => {
                self.pending_shortcut.cancel();
                self.pending_shortcut_scroll = 0;
                Ok(())
            }
            KeyInput::ToggleHelp => {
                self.toggle_help_at_height(terminal_size.height);
                Ok(())
            }
            KeyInput::ScrollPrefix(delta) => {
                self.dispatch_prefix_hint_scroll(delta, terminal_size);
                Ok(())
            }
            KeyInput::Overlay(key) => self.handle_overlay_key_at_size(key, terminal_size).await,
            KeyInput::Normal(code) => self.handle_normal_key(code).await,
            KeyInput::Ignore => Ok(()),
        }
    }

    pub(crate) async fn handle_normal_key(&mut self, code: KeyCode) -> Result<()> {
        let domain = self.current_routing_domain();
        let translation = route_normal_key_in_domain(
            &self.pending_shortcut,
            code,
            self.overlay_captures_input(),
            &self.command_catalog,
            domain,
        );
        self.pending_shortcut = translation.shortcut;
        match translation.input {
            NormalKeyInput::Overlay(key) => self.handle_overlay_key(key).await?,
            NormalKeyInput::CancelShortcut => {}
            NormalKeyInput::CancelOverlay => self.execute(Action::CancelOverlay).await?,
            NormalKeyInput::Command(handler) => self.execute_command_handler(handler).await?,
            NormalKeyInput::Prefix => {}
            NormalKeyInput::Missing(label) => {
                self.set_warning(format!("invalid shortcut: {label}"));
            }
        }
        self.pending_shortcut_scroll = 0;
        Ok(())
    }

    pub(crate) async fn handle_overlay_key(&mut self, key: KeyEvent) -> Result<()> {
        self.handle_overlay_key_at_size(key, Size::new(80, 24))
            .await
    }

    pub(super) fn overlay_captures_input(&self) -> bool {
        self.overlay
            .as_ref()
            .is_some_and(OverlayState::captures_input)
    }

    pub(super) fn toggle_help_at_height(&mut self, _terminal_height: u16) {
        match self.overlay {
            Some(OverlayState::Help { .. }) => self.overlay = None,
            Some(OverlayState::DetailHelp { .. }) => self.overlay = None,
            None if self.detail.is_active() => {
                self.overlay = Some(OverlayState::DetailHelp { scroll: 0 })
            }
            _ => self.overlay = Some(OverlayState::Help { scroll: 0 }),
        }
    }
}
