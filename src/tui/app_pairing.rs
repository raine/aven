use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use crate::pairing::PairingPresentation;
use crate::sync::encrypted::{self, PendingInvitation};
use crate::tui::app::{App, Notification};
use crate::tui::overlay::OverlayState;

/// Creates a device invitation and waits for its admission in the background.
/// Dismissing the QR overlay keeps waiting, as `aven sync invite` does, until
/// the device joins, the invitation expires, or the TUI exits. The invitation
/// text is kept in memory only while admission waits, and leaves it only when
/// the user asks to copy it.
pub(super) struct InviteController {
    create: Option<JoinHandle<Result<PendingInvitation>>>,
    admission: Option<JoinHandle<Result<()>>>,
    presentation: Option<Arc<PairingPresentation>>,
    text: Option<Zeroizing<String>>,
}

impl InviteController {
    pub(super) fn new() -> Self {
        Self {
            create: None,
            admission: None,
            presentation: None,
            text: None,
        }
    }

    pub(super) fn work_pending(&self) -> bool {
        self.create.is_some() || self.admission.is_some()
    }

    #[cfg(test)]
    pub(super) fn show_for_test(&mut self, presentation: Arc<PairingPresentation>, text: &str) {
        self.presentation = Some(presentation);
        self.text = Some(Zeroizing::new(text.to_string()));
    }
}

impl Drop for InviteController {
    fn drop(&mut self) {
        if let Some(task) = self.create.take() {
            task.abort();
        }
        if let Some(task) = self.admission.take() {
            task.abort();
        }
    }
}

impl App {
    pub(in crate::tui) fn show_pairing_invitation(&mut self) {
        if let Some(presentation) = &self.invite.presentation {
            self.overlay = Some(OverlayState::Pairing(presentation.clone()));
            return;
        }
        if self.invite.create.is_some() {
            self.set_info("invitation is being created");
            return;
        }
        if self.sync_ops.work_pending() {
            self.set_info("sync is busy; open :sync to follow its progress");
            return;
        }
        let database = self.store.database();
        let config = self.intake.config().clone();
        self.invite.create = Some(tokio::spawn(async move {
            encrypted::create_invitation(&database, &config).await
        }));
        self.notification = Some(Notification::loading("creating invitation"));
    }

    /// Copies the waiting invitation's text on explicit request, so another
    /// computer can paste it.
    pub(in crate::tui) fn copy_pairing_invitation(&mut self) {
        let Some(text) = &self.invite.text else {
            self.set_info("no invitation is waiting");
            return;
        };
        match crate::tui::platform::copy_to_clipboard(text) {
            Ok(()) => self.set_warning("invitation copied: it grants access to your synced data"),
            Err(error) => self.set_warning(format!("could not copy invitation: {error:#}")),
        }
    }

    pub(in crate::tui) fn pairing_unavailable_reason(&self) -> Option<&'static str> {
        (!self.store.sync_status.set_up).then_some("requires sync setup")
    }

    pub(super) async fn poll_invite(&mut self) -> Result<bool> {
        if let Some(task) = self.invite.create.take_if(|task| task.is_finished()) {
            if matches!(self.notification, Some(Notification::Loading { .. })) {
                self.notification = None;
            }
            match task
                .await
                .context("invitation task stopped")
                .and_then(|result| result)
            {
                Ok(invitation) => match invitation.tui_presentation() {
                    Ok(presentation) => {
                        let presentation = Arc::new(presentation);
                        self.store.sync_status.invitation = Some(encrypted::InvitationStatus {
                            expires_at: invitation.expires_at(),
                            keys_may_have_been_sent: false,
                        });
                        self.invite.presentation = Some(presentation.clone());
                        self.invite.text = Some(Zeroizing::new(invitation.text().to_string()));
                        self.overlay = Some(OverlayState::Pairing(presentation));
                        let database = self.store.database();
                        self.invite.admission = Some(tokio::spawn(async move {
                            encrypted::await_admission(&database, &invitation).await
                        }));
                    }
                    Err(error) => {
                        self.set_error(format!(
                            "QR code unavailable: {error:#}; run `aven sync invite`"
                        ));
                    }
                },
                Err(error) if crate::tui::sync_errors::reached_change_limit(&error) => {
                    self.set_error(crate::tui::sync_errors::CHANGE_LIMIT)
                }
                Err(error) => {
                    self.store.refresh_sync_status().await?;
                    let message = crate::tui::sync_errors::failure(
                        crate::tui::sync_operations::OperationKind::ListDevices,
                        &error,
                    )
                    .message;
                    self.set_error(format!("invitation unavailable: {message}"));
                }
            }
            return Ok(true);
        }
        let Some(task) = self.invite.admission.take_if(|task| task.is_finished()) else {
            return Ok(false);
        };
        self.invite.presentation = None;
        self.invite.text = None;
        if matches!(self.overlay, Some(OverlayState::Pairing(_))) {
            self.overlay = None;
        }
        match task
            .await
            .context("invitation task stopped")
            .and_then(|result| result)
        {
            Ok(()) => self.set_success("device added"),
            Err(error) => {
                self.store.refresh_sync_status().await?;
                let status = encrypted::invitation_status(&self.store.database()).await?;
                if status.is_some_and(|state| state.keys_may_have_been_sent) {
                    self.set_warning("invitation expired; the next sync changes keys");
                } else if error
                    .to_string()
                    .starts_with("error sync-invitation-unused")
                {
                    self.set_warning("invitation expired unused");
                } else {
                    let message = crate::tui::sync_errors::failure(
                        crate::tui::sync_operations::OperationKind::ListDevices,
                        &error,
                    )
                    .message;
                    self.set_warning(message);
                }
            }
        }
        self.store.sync_status.invitation =
            encrypted::invitation_status(&self.store.database()).await?;
        Ok(true)
    }

    pub(in crate::tui) async fn cancel_pairing_invitation(&mut self) -> Result<()> {
        let result =
            encrypted::cancel_invitation(&self.store.database(), self.intake.config()).await?;
        match result {
            encrypted::Cancellation::Cancelled => {
                if let Some(task) = self.invite.admission.take() {
                    task.abort();
                }
                self.invite.presentation = None;
                self.invite.text = None;
                self.store.sync_status.invitation = None;
                self.set_success("invitation cancelled");
            }
            encrypted::Cancellation::KeysMayHaveBeenSent { expires_at } => {
                self.set_warning(format!(
                    "keys may already have been sent; the invitation expires at {}, and the next sync then changes keys",
                    encrypted::format_expiry(expires_at)
                ));
            }
            encrypted::Cancellation::None => {
                self.store.sync_status.invitation = None;
                self.set_info("no invitation is open");
            }
        }
        Ok(())
    }
}
