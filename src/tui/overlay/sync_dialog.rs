//! Sync dialog pages, focus and keyboard handling. The dialog presents sync
//! state and starts work; the application owns that work, so closing the
//! dialog never cancels it. Unsubmitted invitation text belongs to the dialog
//! and is discarded with it.
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use zeroize::Zeroizing;

use crate::sync::encrypted::{LocalPhase, SetupPreview};
use crate::tui::store::TuiSyncStatus;
use crate::tui::sync_operations::SyncActivity;

/// Upper bound on pasted invitation text, matching the CLI's input limit.
const INVITATION_LIMIT: usize = 8192;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SyncDialogState {
    pub(crate) page: SyncPage,
    /// Index into the actions the page offers.
    pub(crate) selected: usize,
    pub(crate) details: bool,
    pub(crate) scroll: u16,
}

impl SyncDialogState {
    pub(crate) fn page(page: SyncPage) -> Self {
        let selected = page.default_focus();
        Self {
            page,
            selected,
            details: false,
            scroll: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InvitationKind {
    Setup,
    Join,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) enum SyncPage {
    #[default]
    Home,
    Invitation {
        kind: InvitationKind,
        input: SecretText,
        error: Option<&'static str>,
    },
    ConfirmSetup {
        server: String,
        preview: SetupPreview,
        invitation: SecretText,
    },
    ConfirmJoin {
        server: String,
        invitation: SecretText,
    },
}

impl SyncPage {
    /// Confirmations that change what this database is start on Back.
    fn default_focus(&self) -> usize {
        match self {
            Self::Invitation { .. } | Self::ConfirmJoin { .. } => 1,
            Self::Home | Self::ConfirmSetup { .. } => 0,
        }
    }

    /// Pages whose actions render as a row of buttons.
    pub(crate) fn has_buttons(&self) -> bool {
        !matches!(self, Self::Home)
    }
}

/// Invitation text typed or pasted into the dialog. It never renders, and its
/// memory is cleared when dropped.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct SecretText(Zeroizing<String>);

impl fmt::Debug for SecretText {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretText([REDACTED])")
    }
}

impl SecretText {
    pub(crate) fn insert(&mut self, text: &str) {
        for character in text.chars().filter(|character| !character.is_control()) {
            if self.0.len() + character.len_utf8() > INVITATION_LIMIT {
                break;
            }
            self.0.push(character);
        }
    }

    /// Zeroizing also clears the spare capacity this leaves behind.
    pub(crate) fn pop(&mut self) {
        self.0.pop();
    }

    pub(crate) fn chars(&self) -> usize {
        self.0.chars().count()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.trim().is_empty()
    }

    pub(crate) fn expose(&self) -> &str {
        &self.0
    }

    pub(crate) fn into_inner(self) -> Zeroizing<String> {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncAction {
    SyncNow,
    AddDevice,
    SetUp,
    Join,
    ResumeSetup,
    ResumeJoin,
    Back,
    Continue,
    ConfirmSetup,
    ConfirmJoin,
}

impl SyncAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::SyncNow => "Sync now",
            Self::AddDevice => "Add device",
            Self::SetUp => "Set up sync",
            Self::Join => "Join existing sync",
            Self::ResumeSetup => "Resume setup",
            Self::ResumeJoin => "Resume joining",
            Self::Back => "Back",
            Self::Continue => "Continue",
            Self::ConfirmSetup => "Set up sync",
            Self::ConfirmJoin => "Join",
        }
    }
}

/// What the dialog offers now, in focus order. Rendering, keyboard and mouse
/// input share this list. Nothing starts while an operation runs.
pub(crate) fn sync_actions(
    state: &SyncDialogState,
    status: &TuiSyncStatus,
    activity: &SyncActivity,
) -> Vec<SyncAction> {
    match state.page {
        SyncPage::Home if activity.running.is_some() => Vec::new(),
        SyncPage::Home => match status.phase {
            LocalPhase::SetUp => vec![SyncAction::SyncNow, SyncAction::AddDevice],
            LocalPhase::NotSetUp => vec![SyncAction::SetUp, SyncAction::Join],
            LocalPhase::SetupIncomplete => vec![SyncAction::ResumeSetup],
            LocalPhase::JoinIncomplete => vec![SyncAction::ResumeJoin],
        },
        SyncPage::Invitation { .. } => vec![SyncAction::Back, SyncAction::Continue],
        SyncPage::ConfirmSetup { .. } => vec![SyncAction::Back, SyncAction::ConfirmSetup],
        SyncPage::ConfirmJoin { .. } => vec![SyncAction::Back, SyncAction::ConfirmJoin],
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
    state: SyncDialogState,
    key: KeyEvent,
    actions: &[SyncAction],
    scroll_cap: u16,
) -> SyncDialogOutcome {
    match state.page {
        SyncPage::Home => handle_home_key(state, key, actions, scroll_cap),
        SyncPage::Invitation { .. } => handle_invitation_key(state, key, actions),
        SyncPage::ConfirmSetup { .. } | SyncPage::ConfirmJoin { .. } => {
            handle_buttons_key(state, key, actions)
        }
    }
}

/// Adds pasted text to an invitation form; other pages ignore it.
pub(crate) fn paste_into_sync_dialog(state: &mut SyncDialogState, text: &str) {
    if let SyncPage::Invitation { input, error, .. } = &mut state.page {
        input.insert(text);
        *error = None;
    }
}

fn handle_home_key(
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

/// Every printable key edits the invitation; Enter submits it.
fn handle_invitation_key(
    mut state: SyncDialogState,
    key: KeyEvent,
    actions: &[SyncAction],
) -> SyncDialogOutcome {
    let SyncPage::Invitation { input, error, .. } = &mut state.page else {
        return SyncDialogOutcome::Retained(state);
    };
    match key.code {
        KeyCode::Esc => return SyncDialogOutcome::Run(state, SyncAction::Back),
        KeyCode::Enter => {
            let action = actions
                .get(state.selected)
                .copied()
                .unwrap_or(SyncAction::Continue);
            return SyncDialogOutcome::Run(state, action);
        }
        KeyCode::Tab | KeyCode::BackTab => {
            state.selected = 1 - state.selected.min(1);
            return SyncDialogOutcome::Retained(state);
        }
        KeyCode::Backspace => input.pop(),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            *input = SecretText::default();
        }
        KeyCode::Char(character) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
            input.insert(character.encode_utf8(&mut [0; 4]));
        }
        _ => return SyncDialogOutcome::Retained(state),
    }
    *error = None;
    state.selected = 1;
    SyncDialogOutcome::Retained(state)
}

fn handle_buttons_key(
    mut state: SyncDialogState,
    key: KeyEvent,
    actions: &[SyncAction],
) -> SyncDialogOutcome {
    match key.code {
        KeyCode::Esc => SyncDialogOutcome::Run(state, SyncAction::Back),
        KeyCode::Enter => match actions.get(state.selected) {
            Some(&action) => SyncDialogOutcome::Run(state, action),
            None => SyncDialogOutcome::Retained(state),
        },
        KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab | KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            SyncDialogOutcome::Retained(state)
        }
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab | KeyCode::Down => {
            state.selected = (state.selected + 1).min(actions.len().saturating_sub(1));
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
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn status(phase: LocalPhase) -> TuiSyncStatus {
        TuiSyncStatus {
            set_up: phase != LocalPhase::NotSetUp,
            phase,
            ..TuiSyncStatus::default()
        }
    }

    fn retained(outcome: SyncDialogOutcome) -> SyncDialogState {
        let SyncDialogOutcome::Retained(state) = outcome else {
            panic!("expected retained dialog, got {outcome:?}");
        };
        state
    }

    #[test]
    fn home_actions_follow_the_local_phase() {
        let state = SyncDialogState::default();
        let idle = SyncActivity::default();
        assert_eq!(
            sync_actions(&state, &status(LocalPhase::NotSetUp), &idle),
            [SyncAction::SetUp, SyncAction::Join]
        );
        assert_eq!(
            sync_actions(&state, &status(LocalPhase::SetupIncomplete), &idle),
            [SyncAction::ResumeSetup]
        );
        assert_eq!(
            sync_actions(&state, &status(LocalPhase::JoinIncomplete), &idle),
            [SyncAction::ResumeJoin]
        );
        assert_eq!(
            sync_actions(&state, &status(LocalPhase::SetUp), &idle),
            [SyncAction::SyncNow, SyncAction::AddDevice]
        );
    }

    #[test]
    fn focus_moves_within_actions_and_enter_runs_the_focused_one() {
        let actions = [SyncAction::SyncNow, SyncAction::AddDevice];
        let state = retained(handle_sync_dialog_key(
            SyncDialogState::default(),
            key(KeyCode::Down),
            &actions,
            0,
        ));
        assert_eq!(state.selected, 1);
        let state = retained(handle_sync_dialog_key(
            state,
            key(KeyCode::Char('j')),
            &actions,
            0,
        ));
        assert_eq!(state.selected, 1);
        assert!(matches!(
            handle_sync_dialog_key(state, key(KeyCode::Enter), &actions, 0),
            SyncDialogOutcome::Run(_, SyncAction::AddDevice)
        ));
    }

    #[test]
    fn without_actions_arrows_scroll_and_enter_closes() {
        let state = retained(handle_sync_dialog_key(
            SyncDialogState::default(),
            key(KeyCode::Down),
            &[],
            3,
        ));
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

    #[test]
    fn invitation_keys_edit_hidden_text_and_enter_submits() {
        let actions = [SyncAction::Back, SyncAction::Continue];
        let mut state = SyncDialogState::page(SyncPage::Invitation {
            kind: InvitationKind::Join,
            input: SecretText::default(),
            error: Some("invalid"),
        });
        for character in ['a', 'j', 'k', 'd', 'S'] {
            state = retained(handle_sync_dialog_key(
                state,
                key(KeyCode::Char(character)),
                &actions,
                0,
            ));
        }
        state = retained(handle_sync_dialog_key(
            state,
            key(KeyCode::Backspace),
            &actions,
            0,
        ));
        let SyncPage::Invitation { input, error, .. } = &state.page else {
            panic!("expected invitation page");
        };
        assert_eq!(input.expose(), "ajkd");
        assert_eq!(*error, None);
        assert_eq!(format!("{input:?}"), "SecretText([REDACTED])");
        assert!(matches!(
            handle_sync_dialog_key(state.clone(), key(KeyCode::Enter), &actions, 0),
            SyncDialogOutcome::Run(_, SyncAction::Continue)
        ));
        assert!(matches!(
            handle_sync_dialog_key(state, key(KeyCode::Esc), &actions, 0),
            SyncDialogOutcome::Run(_, SyncAction::Back)
        ));
    }

    #[test]
    fn pasted_invitations_drop_line_breaks_and_stay_bounded() {
        let mut state = SyncDialogState::page(SyncPage::Invitation {
            kind: InvitationKind::Setup,
            input: SecretText::default(),
            error: None,
        });
        paste_into_sync_dialog(&mut state, "aven-sync-setup-1:abc\r\n");
        let SyncPage::Invitation { input, .. } = &state.page else {
            panic!("expected invitation page");
        };
        assert_eq!(input.expose(), "aven-sync-setup-1:abc");
        let mut long = SecretText::default();
        long.insert(&"x".repeat(INVITATION_LIMIT + 10));
        assert_eq!(long.chars(), INVITATION_LIMIT);
    }

    #[test]
    fn confirmations_start_on_back_except_joining() {
        let setup = SyncDialogState::page(SyncPage::ConfirmSetup {
            server: "https://sync.example.com".to_string(),
            preview: SetupPreview {
                workspaces: 1,
                tasks: 2,
                missing_images: 0,
                leaves_unencrypted_server: false,
            },
            invitation: SecretText::default(),
        });
        assert_eq!(setup.selected, 0);
        let actions = [SyncAction::Back, SyncAction::ConfirmSetup];
        assert!(matches!(
            handle_sync_dialog_key(setup.clone(), key(KeyCode::Enter), &actions, 0),
            SyncDialogOutcome::Run(_, SyncAction::Back)
        ));
        let state = retained(handle_sync_dialog_key(
            setup,
            key(KeyCode::Right),
            &actions,
            0,
        ));
        assert!(matches!(
            handle_sync_dialog_key(state, key(KeyCode::Enter), &actions, 0),
            SyncDialogOutcome::Run(_, SyncAction::ConfirmSetup)
        ));
        let join = SyncDialogState::page(SyncPage::ConfirmJoin {
            server: "https://sync.example.com".to_string(),
            invitation: SecretText::default(),
        });
        assert_eq!(join.selected, 1);
    }

    #[test]
    fn running_operations_offer_no_new_actions() {
        let activity = SyncActivity {
            running: Some(crate::tui::sync_operations::RunningOperation {
                kind: crate::tui::sync_operations::OperationKind::Join,
                stage: None,
                started_at: std::time::Instant::now(),
            }),
            last: None,
        };
        assert!(
            sync_actions(
                &SyncDialogState::default(),
                &status(LocalPhase::NotSetUp),
                &activity
            )
            .is_empty()
        );
    }
}
