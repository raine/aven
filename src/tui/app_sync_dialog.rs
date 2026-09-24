use anyhow::Result;
use crossterm::event::{KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::Size;

use crate::sync::encrypted::{self, DeviceInvitation, LocalPhase, SetupInvitation};
use crate::tui::app::App;
use crate::tui::overlay::{
    InvitationKind, OverlayState, SecretText, SyncAction, SyncDialogOutcome, SyncDialogState,
    SyncDialogView, SyncPage, handle_sync_dialog_key, paste_into_sync_dialog, sync_actions,
};
use crate::tui::sync_operations::{OperationEvent, OperationKind, OperationResult};

const NOT_A_SETUP_INVITATION: &str = "This isn't a setup invitation. Paste the invitation \
     printed by `aven server setup` on your server.";
const DEVICE_INVITATION_FOR_SETUP: &str = "This invitation adds a device to existing sync. \
     Go back and choose Join existing sync.";
const NOT_A_DEVICE_INVITATION: &str = "This isn't a device invitation. Paste the invitation \
     shown by Add device on your other device.";
const SETUP_INVITATION_FOR_JOIN: &str = "This invitation starts sync on a new server. Go \
     back and choose Set up sync.";

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
        let actions = sync_actions(&state, &self.store.sync_status, &self.sync_ops.activity);
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
                let hit = crate::tui::ui::sync_dialog_hit(
                    &self.sync_dialog_view(&state),
                    terminal_size,
                    mouse.column,
                    mouse.row,
                );
                match hit {
                    crate::tui::ui::SyncDialogHit::Action(index) => {
                        let actions =
                            sync_actions(&state, &self.store.sync_status, &self.sync_ops.activity);
                        match actions.get(index) {
                            Some(&action) => {
                                state.selected = index;
                                SyncDialogOutcome::Run(state, action)
                            }
                            None => SyncDialogOutcome::Retained(state),
                        }
                    }
                    crate::tui::ui::SyncDialogHit::Inside => SyncDialogOutcome::Retained(state),
                    // Clicking away closes the view; unsubmitted forms are
                    // discarded and running work continues.
                    crate::tui::ui::SyncDialogHit::Outside => SyncDialogOutcome::Closed,
                }
            }
            _ => SyncDialogOutcome::Retained(state),
        };
        self.apply_sync_dialog_outcome(outcome).await
    }

    pub(super) fn handle_sync_dialog_paste(&mut self, mut state: SyncDialogState, text: &str) {
        paste_into_sync_dialog(&mut state, text);
        self.overlay = Some(OverlayState::Sync(state));
    }

    async fn apply_sync_dialog_outcome(&mut self, outcome: SyncDialogOutcome) -> Result<()> {
        match outcome {
            SyncDialogOutcome::Retained(state) => self.overlay = Some(OverlayState::Sync(state)),
            SyncDialogOutcome::Closed => {}
            SyncDialogOutcome::Run(state, action) => self.run_sync_action(state, action).await?,
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

    async fn run_sync_action(&mut self, state: SyncDialogState, action: SyncAction) -> Result<()> {
        let home = SyncDialogState {
            details: state.details,
            ..SyncDialogState::default()
        };
        let next = match action {
            SyncAction::SyncNow => {
                self.begin_sync();
                state
            }
            // The invitation overlay replaces the dialog; admission keeps
            // waiting after it closes.
            SyncAction::AddDevice => {
                self.show_pairing_invitation();
                return Ok(());
            }
            SyncAction::Back => home,
            SyncAction::SetUp => invitation_page(InvitationKind::Setup),
            SyncAction::ResumeSetup if self.sync_ops.has_setup_invitation() => {
                self.start_sync_operation(OperationKind::Setup, None);
                home
            }
            SyncAction::ResumeSetup => invitation_page(InvitationKind::Setup),
            SyncAction::Join => {
                // The engine's fresh-target check reads without changing data.
                let config = self.intake.config().clone();
                match encrypted::ensure_join_available(&self.store.database(), &config).await {
                    Ok(()) => invitation_page(InvitationKind::Join),
                    Err(error) => {
                        self.sync_ops.record_refusal(OperationKind::Join, &error);
                        home
                    }
                }
            }
            SyncAction::ResumeJoin => {
                self.start_sync_operation(OperationKind::Join, None);
                home
            }
            SyncAction::Continue => self.submit_invitation(state).await?,
            SyncAction::ConfirmSetup | SyncAction::ConfirmJoin => {
                let (kind, invitation) = match state.page {
                    SyncPage::ConfirmSetup { invitation, .. } => (OperationKind::Setup, invitation),
                    SyncPage::ConfirmJoin { invitation, .. } => (OperationKind::Join, invitation),
                    _ => return Ok(()),
                };
                self.start_sync_operation(kind, Some(invitation));
                home
            }
        };
        self.overlay = Some(OverlayState::Sync(next));
        Ok(())
    }

    /// Validates the pasted invitation with the engine's parser and moves to
    /// its confirmation. Resuming an interrupted setup continues it directly,
    /// since the original attempt was confirmed.
    async fn submit_invitation(&mut self, mut state: SyncDialogState) -> Result<SyncDialogState> {
        let SyncPage::Invitation { kind, input, error } = &mut state.page else {
            return Ok(state);
        };
        if input.is_empty() {
            *error = Some("Paste the invitation first.");
            return Ok(state);
        }
        let text = input.expose();
        match kind {
            InvitationKind::Setup => {
                let server = match SetupInvitation::decode(text) {
                    Ok(invitation) => invitation.server,
                    Err(_) => {
                        *error = Some(if DeviceInvitation::decode(text).is_ok() {
                            DEVICE_INVITATION_FOR_SETUP
                        } else {
                            NOT_A_SETUP_INVITATION
                        });
                        return Ok(state);
                    }
                };
                let invitation = std::mem::take(input);
                if self.store.sync_status.phase == LocalPhase::SetupIncomplete {
                    self.start_sync_operation(OperationKind::Setup, Some(invitation));
                    return Ok(SyncDialogState::default());
                }
                let preview = encrypted::setup_preview(&self.store.database()).await?;
                Ok(SyncDialogState::page(SyncPage::ConfirmSetup {
                    server,
                    preview,
                    invitation,
                }))
            }
            InvitationKind::Join => {
                let server = match DeviceInvitation::decode(text) {
                    Ok(invitation) => invitation.server,
                    Err(_) => {
                        *error = Some(if SetupInvitation::decode(text).is_ok() {
                            SETUP_INVITATION_FOR_JOIN
                        } else {
                            NOT_A_DEVICE_INVITATION
                        });
                        return Ok(state);
                    }
                };
                Ok(SyncDialogState::page(SyncPage::ConfirmJoin {
                    server,
                    invitation: std::mem::take(input),
                }))
            }
        }
    }

    fn start_sync_operation(&mut self, kind: OperationKind, invitation: Option<SecretText>) {
        if self.sync.work_pending() {
            self.set_info("sync is running; try again when it finishes");
            return;
        }
        let database = self.store.database();
        let config = self.intake.config().clone();
        let invitation = invitation.map(SecretText::into_inner);
        let started = match kind {
            OperationKind::Setup => self.sync_ops.start_setup(&database, &config, invitation),
            OperationKind::Join => self.sync_ops.start_join(&database, &config, invitation),
        };
        if !started {
            self.set_info("another sync operation is in progress");
        }
    }

    /// Applies finished work and stage changes. Joining refreshes once tasks
    /// are installed so they can be browsed while images download.
    pub(super) async fn poll_sync_operations(&mut self) -> Result<bool> {
        let Some(event) = self.sync_ops.poll().await else {
            return Ok(false);
        };
        match event {
            OperationEvent::Stage(OperationKind::Join, encrypted::Stage::CatchingUp) => {
                self.refresh().await?;
            }
            OperationEvent::Stage(..) => {}
            OperationEvent::Finished(result) => {
                let refreshed = self.refresh().await;
                if !matches!(self.overlay, Some(OverlayState::Sync(_))) {
                    self.notify_sync_operation(&result);
                }
                refreshed?;
            }
        }
        Ok(true)
    }

    fn notify_sync_operation(&mut self, result: &OperationResult) {
        match result {
            OperationResult::SetUp { drain, .. } | OperationResult::Joined { drain, .. } => {
                let done = match result {
                    OperationResult::SetUp { .. } => "sync set up",
                    _ => "joined sync",
                };
                if drain.tasks_current && drain.images == "complete" {
                    self.set_success(done);
                } else {
                    self.set_warning(format!(
                        "{done}; some work is still waiting, open :sync for details"
                    ));
                }
            }
            OperationResult::Failed(failure) => self.set_error(format!(
                "{}: open :sync for details",
                match failure.kind {
                    OperationKind::Setup => "setup didn't finish",
                    OperationKind::Join => "joining didn't finish",
                }
            )),
        }
    }

    /// Local edits made before joined tasks are installed would make the
    /// engine refuse the installation, so joining pauses them.
    pub(in crate::tui) fn local_edits_paused_by_join(&self) -> bool {
        self.sync_ops.activity.join_awaiting_tasks()
            || self.store.sync_status.phase == LocalPhase::JoinIncomplete
    }

    pub(super) fn sync_dialog_view<'a>(&'a self, state: &'a SyncDialogState) -> SyncDialogView<'a> {
        SyncDialogView {
            state,
            status: &self.store.sync_status,
            activity: &self.sync_ops.activity,
            syncing: self.sync.work_pending(),
        }
    }

    fn sync_dialog_scroll_cap(&self, state: &SyncDialogState, terminal_size: Size) -> u16 {
        crate::tui::ui::sync_dialog_scroll_cap(&self.sync_dialog_view(state), terminal_size)
    }
}

fn invitation_page(kind: InvitationKind) -> SyncDialogState {
    SyncDialogState::page(SyncPage::Invitation {
        kind,
        input: SecretText::default(),
        error: None,
    })
}
