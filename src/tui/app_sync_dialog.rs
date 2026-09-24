use anyhow::Result;
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Size;

use crate::tui::app::App;
use crate::tui::overlay::{
    OverlayState, SyncAction, SyncDialogOutcome, SyncDialogState, handle_sync_dialog_key,
    sync_actions,
};

impl App {
    /// Opens the Sync dialog from `:sync`, `C s` or the header indicator.
    pub(in crate::tui) fn show_sync_dialog(&mut self) {
        self.pending_shortcut.clear();
        self.overlay = Some(OverlayState::Sync(SyncDialogState::default()));
    }

    pub(super) async fn handle_sync_dialog_key(
        &mut self,
        state: SyncDialogState,
        key: KeyEvent,
        terminal_size: Size,
    ) -> Result<()> {
        let actions = sync_actions(&state, &self.store.sync_status);
        let scroll_cap = self.sync_dialog_scroll_cap(&state, terminal_size);
        let outcome = handle_sync_dialog_key(state, key, &actions, scroll_cap);
        self.apply_sync_dialog_outcome(outcome).await
    }

    pub(super) async fn handle_sync_dialog_mouse(
        &mut self,
        mut state: SyncDialogState,
        mouse: MouseEvent,
        terminal_size: Size,
    ) -> Result<()> {
        let scroll_cap = self.sync_dialog_scroll_cap(&state, terminal_size);
        let outcome = match mouse.kind {
            MouseEventKind::ScrollDown => {
                state.scroll = state.scroll.saturating_add(1).min(scroll_cap);
                SyncDialogOutcome::Retained(state)
            }
            MouseEventKind::ScrollUp => {
                state.scroll = state.scroll.saturating_sub(1);
                SyncDialogOutcome::Retained(state)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let view = self.sync_dialog_view(&state);
                match crate::tui::ui::sync_dialog_hit(&view, terminal_size, mouse.column, mouse.row)
                {
                    crate::tui::ui::SyncDialogHit::Action(index) => {
                        let actions = sync_actions(&state, &self.store.sync_status);
                        match actions.get(index) {
                            Some(&action) => {
                                state.selected = index;
                                SyncDialogOutcome::Run(state, action)
                            }
                            None => SyncDialogOutcome::Retained(state),
                        }
                    }
                    crate::tui::ui::SyncDialogHit::Inside => SyncDialogOutcome::Retained(state),
                    crate::tui::ui::SyncDialogHit::Outside => SyncDialogOutcome::Closed,
                }
            }
            _ => SyncDialogOutcome::Retained(state),
        };
        self.apply_sync_dialog_outcome(outcome).await
    }

    async fn apply_sync_dialog_outcome(&mut self, outcome: SyncDialogOutcome) -> Result<()> {
        match outcome {
            SyncDialogOutcome::Retained(state) => self.overlay = Some(OverlayState::Sync(state)),
            SyncDialogOutcome::Closed => {}
            SyncDialogOutcome::Run(state, action) => self.run_sync_action(state, action),
            SyncDialogOutcome::ShowConflicts(state) => {
                if self.store.sync_status.conflicts > 0 {
                    self.clear_detail_session();
                    self.open_conflict_list().await?;
                } else {
                    self.overlay = Some(OverlayState::Sync(state));
                    self.set_info("no unresolved conflicts");
                }
            }
        }
        Ok(())
    }

    fn run_sync_action(&mut self, state: SyncDialogState, action: SyncAction) {
        match action {
            SyncAction::SyncNow => {
                self.overlay = Some(OverlayState::Sync(state));
                self.begin_sync();
            }
            // The invitation overlay replaces the dialog; admission keeps
            // waiting after it closes.
            SyncAction::AddDevice => self.show_pairing_invitation(),
        }
    }

    fn sync_dialog_view<'a>(
        &'a self,
        state: &'a SyncDialogState,
    ) -> crate::tui::overlay::SyncDialogView<'a> {
        crate::tui::overlay::SyncDialogView {
            state,
            status: &self.store.sync_status,
            syncing: self.sync.work_pending(),
        }
    }

    fn sync_dialog_scroll_cap(&self, state: &SyncDialogState, terminal_size: Size) -> u16 {
        crate::tui::ui::sync_dialog_scroll_cap(&self.sync_dialog_view(state), terminal_size)
    }
}
