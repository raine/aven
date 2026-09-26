//! Sync dialog pages, focus and keyboard handling. The dialog presents sync
//! state and starts work; the application owns that work, so closing the
//! dialog never cancels it. Unsubmitted invitation text belongs to the dialog
//! and is discarded with it.
use std::fmt;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use zeroize::Zeroizing;

use crate::sync::encrypted::{InvitationCheck, LocalPhase, SetupPreview};
use crate::tui::store::TuiSyncStatus;
use crate::tui::sync_operations::{
    FailureSignal, OperationFailure, OperationKind, OperationResult, SyncActivity,
};

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
        /// Continues an unfinished join with an invitation it hasn't used.
        replace: bool,
    },
    Devices,
    ConfirmRemove {
        device: [u8; 32],
    },
    ConfirmAutomaticSync {
        service: AutomaticSyncService,
    },
}

/// What turning on automatic sync does besides enabling the setting.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum AutomaticSyncService {
    /// Installs the background service for this database.
    Install,
    /// The service would serve another database, so none is installed.
    OtherDatabase(std::path::PathBuf),
    /// No supported service manager exists on this platform.
    Unsupported,
}

impl SyncPage {
    /// Confirmations that change what this database is, or remove a
    /// device, start on Back or Cancel.
    fn default_focus(&self) -> usize {
        match self {
            Self::Invitation { .. } | Self::ConfirmJoin { .. } => 1,
            Self::Home
            | Self::ConfirmSetup { .. }
            | Self::Devices
            | Self::ConfirmRemove { .. }
            | Self::ConfirmAutomaticSync { .. } => 0,
        }
    }

    /// Pages whose actions render as a row of buttons.
    pub(crate) fn has_buttons(&self) -> bool {
        !matches!(self, Self::Home | Self::Devices)
    }
}

/// Invitation text typed or pasted into the dialog. It never renders, and its
/// memory is cleared when dropped. The text is checked on each edit, so
/// rendering never decodes it.
#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct SecretText(Zeroizing<String>, InvitationCheck);

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
        self.1 = InvitationCheck::of(&self.0);
    }

    /// Zeroizing also clears the spare capacity this leaves behind.
    pub(crate) fn pop(&mut self) {
        self.0.pop();
        self.1 = InvitationCheck::of(&self.0);
    }

    #[cfg(test)]
    pub(crate) fn chars(&self) -> usize {
        self.0.chars().count()
    }

    pub(crate) fn check(&self) -> &InvitationCheck {
        &self.1
    }

    #[cfg(test)]
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
    /// Turns on automatic sync and installs the background service.
    SyncAutomatically,
    AddDevice,
    CancelInvitation,
    SetUp,
    Join,
    ResumeSetup,
    ResumeJoin,
    /// Continues an unfinished join with a replacement invitation.
    NewJoinInvitation,
    Back,
    Continue,
    ConfirmSetup,
    ConfirmJoin,
    ConfirmAutomaticSync,
    ManageDevices,
    /// A device row on the device page, by listing index.
    Device(usize),
    RefreshDevices,
    CopyDeviceId,
    Cancel,
    ConfirmRemove,
    ResumeRemoval([u8; 32]),
    FinishRemoval,
}

impl SyncAction {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::SyncNow => "Sync now",
            Self::SyncAutomatically => "Sync automatically",
            Self::AddDevice => "Add device",
            Self::CancelInvitation => "Cancel invitation",
            Self::SetUp => "Set up sync",
            Self::Join => "Join existing sync",
            Self::ResumeSetup => "Resume setup",
            Self::ResumeJoin => "Resume joining",
            Self::NewJoinInvitation => "Use a new invitation",
            Self::Back => "Back",
            Self::Continue => "Continue",
            Self::ConfirmSetup => "Set up sync",
            Self::ConfirmJoin => "Join",
            Self::ConfirmAutomaticSync => "Turn on",
            Self::ManageDevices => "Manage devices",
            Self::Device(_) => "Device",
            Self::RefreshDevices => "Refresh",
            Self::CopyDeviceId => "Copy device ID",
            Self::Cancel => "Cancel",
            Self::ConfirmRemove => "Remove device",
            Self::ResumeRemoval(_) => "Resume removal",
            Self::FinishRemoval => "Finish removal",
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
            LocalPhase::SetUp => {
                let refused = status.access_refused_at.is_some();
                let mut actions = vec![SyncAction::SyncNow];
                if !refused && !status.enabled && status.runtime_allowed {
                    actions.push(SyncAction::SyncAutomatically);
                }
                if !refused {
                    actions.push(SyncAction::AddDevice);
                }
                // Cancelling retires the invitation locally even when the
                // server refuses this device.
                if status.invitation.is_some() {
                    actions.push(SyncAction::CancelInvitation);
                }
                if !refused {
                    actions.push(SyncAction::ManageDevices);
                }
                actions
            }
            LocalPhase::NotSetUp => match setup_refusal(activity) {
                // A fresh invitation for unclaimed storage can succeed.
                Some(SetupRefusal::Invitation) => vec![SyncAction::SetUp, SyncAction::Back],
                Some(SetupRefusal::StorageClaimed) => vec![SyncAction::Back],
                None => vec![SyncAction::SetUp, SyncAction::Join],
            },
            LocalPhase::SetupIncomplete => vec![SyncAction::ResumeSetup],
            LocalPhase::SetupRecoveryRequired => vec![SyncAction::Back],
            LocalPhase::JoinIncomplete => {
                vec![SyncAction::ResumeJoin, SyncAction::NewJoinInvitation]
            }
        },
        // Resuming continues the confirmed setup directly, so the button
        // names that instead of a further step.
        SyncPage::Invitation {
            kind: InvitationKind::Setup,
            ..
        } if status.phase == LocalPhase::SetupIncomplete => {
            vec![SyncAction::Back, SyncAction::ResumeSetup]
        }
        SyncPage::Invitation { .. } => vec![SyncAction::Back, SyncAction::Continue],
        SyncPage::ConfirmSetup { .. } => vec![SyncAction::Back, SyncAction::ConfirmSetup],
        SyncPage::ConfirmJoin { .. } => vec![SyncAction::Back, SyncAction::ConfirmJoin],
        SyncPage::Devices => device_actions(activity),
        SyncPage::ConfirmRemove { .. } => vec![SyncAction::Cancel, SyncAction::ConfirmRemove],
        SyncPage::ConfirmAutomaticSync { .. } => {
            vec![SyncAction::Back, SyncAction::ConfirmAutomaticSync]
        }
    }
}

enum SetupRefusal {
    Invitation,
    StorageClaimed,
}

/// A definite server refusal of the last setup attempt, which left this
/// database local-only.
fn setup_refusal(activity: &SyncActivity) -> Option<SetupRefusal> {
    let Some(OperationResult::Failed(failure)) = &activity.last else {
        return None;
    };
    if failure.kind != OperationKind::Setup {
        return None;
    }
    match failure.signal? {
        FailureSignal::SetupStorageClaimed => Some(SetupRefusal::StorageClaimed),
        FailureSignal::SetupInvitationRefused => Some(SetupRefusal::Invitation),
        _ => None,
    }
}

/// A resumable removal first, then one row per listed device. Rows stay
/// selectable while work runs so full IDs remain readable.
fn device_actions(activity: &SyncActivity) -> Vec<SyncAction> {
    let mut actions = Vec::new();
    if activity.running.is_none() {
        let rotation_pending = activity
            .devices
            .as_ref()
            .is_some_and(|snapshot| snapshot.listing.key_rotation_pending);
        match activity.device_result() {
            Some(OperationResult::Failed(failure)) if failure.removal_unfinished() => {
                actions.push(SyncAction::FinishRemoval)
            }
            Some(OperationResult::Failed(OperationFailure {
                kind: OperationKind::RemoveDevice(device),
                ..
            })) => actions.push(SyncAction::ResumeRemoval(*device)),
            Some(OperationResult::Failed(OperationFailure {
                kind: OperationKind::FinishRemoval,
                ..
            })) => actions.push(SyncAction::FinishRemoval),
            _ if rotation_pending => actions.push(SyncAction::FinishRemoval),
            _ => {}
        }
    }
    if let Some(snapshot) = &activity.devices {
        actions.extend((0..snapshot.listing.devices.len()).map(SyncAction::Device));
    }
    actions
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
        SyncPage::Devices => handle_devices_key(state, key, actions),
        SyncPage::ConfirmSetup { .. }
        | SyncPage::ConfirmJoin { .. }
        | SyncPage::ConfirmRemove { .. }
        | SyncPage::ConfirmAutomaticSync { .. } => handle_buttons_key(state, key, actions),
    }
}

fn handle_devices_key(
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
        KeyCode::Char('r') => SyncDialogOutcome::Run(state, SyncAction::RefreshDevices),
        KeyCode::Char('y') => SyncDialogOutcome::Run(state, SyncAction::CopyDeviceId),
        KeyCode::Char('d') => {
            state.details = !state.details;
            SyncDialogOutcome::Retained(state)
        }
        KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
            move_focus(&mut state, 1, actions.len(), 0);
            SyncDialogOutcome::Retained(state)
        }
        KeyCode::Up | KeyCode::Char('k') | KeyCode::BackTab => {
            move_focus(&mut state, -1, actions.len(), 0);
            SyncDialogOutcome::Retained(state)
        }
        _ => SyncDialogOutcome::Retained(state),
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

/// Every printable key edits the invitation; arrows move between the
/// buttons and Enter chooses one.
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
        KeyCode::Left | KeyCode::BackTab => {
            state.selected = state.selected.saturating_sub(1);
            return SyncDialogOutcome::Retained(state);
        }
        KeyCode::Right | KeyCode::Tab => {
            state.selected = (state.selected + 1).min(actions.len().saturating_sub(1));
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
        KeyCode::Esc => {
            SyncDialogOutcome::Run(state, actions.first().copied().unwrap_or(SyncAction::Back))
        }
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
            enabled: true,
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
            [SyncAction::ResumeJoin, SyncAction::NewJoinInvitation]
        );
        assert_eq!(
            sync_actions(&state, &status(LocalPhase::SetUp), &idle),
            [
                SyncAction::SyncNow,
                SyncAction::AddDevice,
                SyncAction::ManageDevices
            ]
        );
        let manual_only = TuiSyncStatus {
            enabled: false,
            ..status(LocalPhase::SetUp)
        };
        assert_eq!(
            sync_actions(&state, &manual_only, &idle),
            [
                SyncAction::SyncNow,
                SyncAction::SyncAutomatically,
                SyncAction::AddDevice,
                SyncAction::ManageDevices
            ]
        );
        let refused = TuiSyncStatus {
            access_refused_at: Some("2026-09-24T12:00:00Z".to_string()),
            ..status(LocalPhase::SetUp)
        };
        assert_eq!(sync_actions(&state, &refused, &idle), [SyncAction::SyncNow]);
        let refused_with_invitation = TuiSyncStatus {
            invitation: Some(crate::sync::encrypted::InvitationStatus {
                expires_at: 1,
                keys_may_have_been_sent: false,
            }),
            ..refused
        };
        assert_eq!(
            sync_actions(&state, &refused_with_invitation, &idle),
            [SyncAction::SyncNow, SyncAction::CancelInvitation]
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
    fn invitation_arrows_move_between_buttons() {
        let actions = [SyncAction::Back, SyncAction::Continue];
        let state = SyncDialogState::page(SyncPage::Invitation {
            kind: InvitationKind::Setup,
            input: SecretText::default(),
            error: None,
        });
        let state = retained(handle_sync_dialog_key(
            state,
            key(KeyCode::Left),
            &actions,
            0,
        ));
        assert_eq!(state.selected, 0);
        assert!(matches!(
            handle_sync_dialog_key(state.clone(), key(KeyCode::Enter), &actions, 0),
            SyncDialogOutcome::Run(_, SyncAction::Back)
        ));
        let state = retained(handle_sync_dialog_key(
            state,
            key(KeyCode::Right),
            &actions,
            0,
        ));
        assert_eq!(state.selected, 1);
        let SyncPage::Invitation { input, .. } = &state.page else {
            panic!("expected invitation page");
        };
        assert_eq!(input.expose(), "");
    }

    #[test]
    fn pasted_invitations_drop_line_breaks_and_stay_bounded() {
        let mut state = SyncDialogState::page(SyncPage::Invitation {
            kind: InvitationKind::Setup,
            input: SecretText::default(),
            error: None,
        });
        paste_into_sync_dialog(&mut state, "aven-setup:abc\r\n");
        let SyncPage::Invitation { input, .. } = &state.page else {
            panic!("expected invitation page");
        };
        assert_eq!(input.expose(), "aven-setup:abc");
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
            replace: false,
        });
        assert_eq!(join.selected, 1);
    }

    fn listed(rotation_pending: bool) -> SyncActivity {
        use crate::sync::encrypted::{Device, DeviceListing};
        SyncActivity {
            devices: Some(crate::tui::sync_operations::DeviceSnapshot {
                listing: DeviceListing {
                    server: "https://sync.example.com".to_string(),
                    key_rotation_pending: rotation_pending,
                    devices: vec![
                        Device {
                            id: [1; 32],
                            label: Some("Office Mac".to_string()),
                            current: true,
                            admission_sequence: 0,
                        },
                        Device {
                            id: [2; 32],
                            label: None,
                            current: false,
                            admission_sequence: 2,
                        },
                    ],
                },
                checked_at: std::time::Instant::now(),
                after_removal: false,
            }),
            ..SyncActivity::default()
        }
    }

    #[test]
    fn device_page_lists_rows_and_offers_to_finish_pending_rotation() {
        let state = SyncDialogState::page(SyncPage::Devices);
        let set_up = status(LocalPhase::SetUp);
        assert_eq!(
            sync_actions(&state, &set_up, &listed(false)),
            [SyncAction::Device(0), SyncAction::Device(1)]
        );
        assert_eq!(
            sync_actions(&state, &set_up, &listed(true)),
            [
                SyncAction::FinishRemoval,
                SyncAction::Device(0),
                SyncAction::Device(1)
            ]
        );

        let mut failed = listed(false);
        failed.last = Some(OperationResult::Failed(OperationFailure {
            kind: OperationKind::RemoveDevice([2; 32]),
            message: String::new(),
            details: "error enrollment-network outcome-unknown".to_string(),
            signal: None,
        }));
        assert_eq!(
            sync_actions(&state, &set_up, &failed)[0],
            SyncAction::ResumeRemoval([2; 32])
        );
        failed.last = Some(OperationResult::Failed(OperationFailure {
            kind: OperationKind::RemoveDevice([2; 32]),
            message: String::new(),
            details: "error management-unfinished".to_string(),
            signal: Some(FailureSignal::RemovalUnfinished),
        }));
        assert_eq!(
            sync_actions(&state, &set_up, &failed)[0],
            SyncAction::FinishRemoval
        );
    }

    #[test]
    fn device_page_keys_select_copy_refresh_and_go_back() {
        let actions = [SyncAction::Device(0), SyncAction::Device(1)];
        let state = retained(handle_sync_dialog_key(
            SyncDialogState::page(SyncPage::Devices),
            key(KeyCode::Down),
            &actions,
            0,
        ));
        assert_eq!(state.selected, 1);
        for (code, action) in [
            (KeyCode::Enter, SyncAction::Device(1)),
            (KeyCode::Char('y'), SyncAction::CopyDeviceId),
            (KeyCode::Char('r'), SyncAction::RefreshDevices),
            (KeyCode::Esc, SyncAction::Back),
        ] {
            assert_eq!(
                handle_sync_dialog_key(state.clone(), key(code), &actions, 0),
                SyncDialogOutcome::Run(state.clone(), action)
            );
        }
    }

    #[test]
    fn removal_confirmation_starts_on_cancel_and_escape_cancels() {
        let state = SyncDialogState::page(SyncPage::ConfirmRemove { device: [2; 32] });
        let actions = sync_actions(&state, &status(LocalPhase::SetUp), &listed(false));
        assert_eq!(actions, [SyncAction::Cancel, SyncAction::ConfirmRemove]);
        assert_eq!(state.selected, 0);
        assert!(matches!(
            handle_sync_dialog_key(state.clone(), key(KeyCode::Enter), &actions, 0),
            SyncDialogOutcome::Run(_, SyncAction::Cancel)
        ));
        assert!(matches!(
            handle_sync_dialog_key(state, key(KeyCode::Esc), &actions, 0),
            SyncDialogOutcome::Run(_, SyncAction::Cancel)
        ));
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
            devices: None,
            join_timed_out: false,
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
