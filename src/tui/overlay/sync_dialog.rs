//! Sync dialog focus and keyboard handling. The dialog presents sync state and
//! starts work; the application owns that work, so closing the dialog never
//! cancels it.
use crossterm::event::{KeyCode, KeyEvent};

use crate::sync::encrypted::LocalPhase;
use crate::tui::store::TuiSyncStatus;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SyncDialogState {
    pub(crate) page: SyncPage,
    /// Index into the actions the page offers.
    pub(crate) selected: usize,
    pub(crate) details: bool,
    pub(crate) scroll: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum SyncPage {
    #[default]
    Home,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncAction {
    SyncNow,
    AddDevice,
}

impl SyncAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::SyncNow => "Sync now",
            Self::AddDevice => "Add device",
        }
    }
}

/// What the dialog offers now, in focus order. Rendering, keyboard and mouse
/// input share this list.
pub(crate) fn sync_actions(state: &SyncDialogState, status: &TuiSyncStatus) -> Vec<SyncAction> {
    match state.page {
        SyncPage::Home => match status.phase {
            LocalPhase::SetUp => vec![SyncAction::SyncNow, SyncAction::AddDevice],
            LocalPhase::NotSetUp | LocalPhase::SetupIncomplete | LocalPhase::JoinIncomplete => {
                Vec::new()
            }
        },
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SyncDialogOutcome {
    Retained(SyncDialogState),
    Closed,
    Run(SyncDialogState, SyncAction),
    ShowConflicts(SyncDialogState),
}

pub(crate) fn handle_sync_dialog_key(
    mut state: SyncDialogState,
    key: KeyEvent,
    actions: &[SyncAction],
    scroll_cap: u16,
) -> SyncDialogOutcome {
    match key.code {
        KeyCode::Esc => SyncDialogOutcome::Closed,
        KeyCode::Enter => match actions.get(state.selected) {
            Some(&action) => SyncDialogOutcome::Run(state, action),
            None => SyncDialogOutcome::Closed,
        },
        KeyCode::Char('S') => SyncDialogOutcome::Run(state, SyncAction::SyncNow),
        KeyCode::Char('c') => SyncDialogOutcome::ShowConflicts(state),
        KeyCode::Char('d') => {
            state.details = !state.details;
            state.scroll = 0;
            SyncDialogOutcome::Retained(state)
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            move_focus(&mut state, 1, actions.len(), scroll_cap);
            SyncDialogOutcome::Retained(state)
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
            move_focus(&mut state, -1, actions.len(), scroll_cap);
            SyncDialogOutcome::Retained(state)
        }
        KeyCode::PageDown => {
            state.scroll = state.scroll.saturating_add(5).min(scroll_cap);
            SyncDialogOutcome::Retained(state)
        }
        KeyCode::PageUp => {
            state.scroll = state.scroll.saturating_sub(5);
            SyncDialogOutcome::Retained(state)
        }
        _ => SyncDialogOutcome::Retained(state),
    }
}

/// Moves action focus, or scrolls when the page has no actions.
fn move_focus(state: &mut SyncDialogState, delta: isize, actions: usize, scroll_cap: u16) {
    if actions == 0 {
        state.scroll = state
            .scroll
            .saturating_add_signed(delta as i16)
            .min(scroll_cap);
        return;
    }
    state.selected = state.selected.saturating_add_signed(delta).min(actions - 1);
}

#[cfg(test)]
mod tests {
    use crossterm::event::KeyModifiers;

    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn set_up() -> TuiSyncStatus {
        TuiSyncStatus {
            set_up: true,
            phase: LocalPhase::SetUp,
            ..TuiSyncStatus::default()
        }
    }

    #[test]
    fn home_offers_sync_and_add_device_only_once_set_up() {
        let state = SyncDialogState::default();
        assert!(sync_actions(&state, &TuiSyncStatus::default()).is_empty());
        assert_eq!(
            sync_actions(&state, &set_up()),
            [SyncAction::SyncNow, SyncAction::AddDevice]
        );
    }

    #[test]
    fn focus_moves_within_actions_and_enter_runs_the_focused_one() {
        let actions = [SyncAction::SyncNow, SyncAction::AddDevice];
        let SyncDialogOutcome::Retained(state) =
            handle_sync_dialog_key(SyncDialogState::default(), key(KeyCode::Down), &actions, 0)
        else {
            panic!("expected retained dialog");
        };
        assert_eq!(state.selected, 1);
        let SyncDialogOutcome::Retained(state) =
            handle_sync_dialog_key(state, key(KeyCode::Char('j')), &actions, 0)
        else {
            panic!("expected retained dialog");
        };
        assert_eq!(state.selected, 1);
        assert!(matches!(
            handle_sync_dialog_key(state, key(KeyCode::Enter), &actions, 0),
            SyncDialogOutcome::Run(_, SyncAction::AddDevice)
        ));
    }

    #[test]
    fn without_actions_arrows_scroll_and_enter_closes() {
        let SyncDialogOutcome::Retained(state) =
            handle_sync_dialog_key(SyncDialogState::default(), key(KeyCode::Down), &[], 3)
        else {
            panic!("expected retained dialog");
        };
        assert_eq!((state.selected, state.scroll), (0, 1));
        assert_eq!(
            handle_sync_dialog_key(state, key(KeyCode::Enter), &[], 3),
            SyncDialogOutcome::Closed
        );
    }

    #[test]
    fn details_toggle_resets_scroll() {
        let state = SyncDialogState {
            scroll: 2,
            ..SyncDialogState::default()
        };
        assert_eq!(
            handle_sync_dialog_key(state, key(KeyCode::Char('d')), &[], 4),
            SyncDialogOutcome::Retained(SyncDialogState {
                details: true,
                scroll: 0,
                ..SyncDialogState::default()
            })
        );
    }
}
