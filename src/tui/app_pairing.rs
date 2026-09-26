use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

use crate::pairing::PairingPresentation;
use crate::sync::encrypted::{self, PendingInvitation};
use crate::tui::app::App;
use crate::tui::overlay::{OverlayState, PairingOverlay, SyncDialogState};

/// Creates a device invitation and waits for its admission in the background.
/// Dismissing the QR overlay keeps waiting, as `aven sync invite` does, until
/// the device joins, the invitation expires, or the TUI exits. The invitation
/// text is kept in memory only while admission waits, and leaves it only when
/// the user asks to copy it.
pub(super) struct InviteController {
    create: Option<JoinHandle<Result<PendingInvitation>>>,
    create_started: std::time::Instant,
    #[cfg(test)]
    create_for_test: Option<JoinHandle<Result<PendingInvitation>>>,
    admission: Option<JoinHandle<Result<encrypted::Admission>>>,
    presentation: Option<Arc<PairingPresentation>>,
    text: Option<Zeroizing<String>>,
    /// The Sync dialog page that Esc on Add device returns to.
    back_to: SyncDialogState,
}

impl InviteController {
    pub(super) fn new() -> Self {
        Self {
            create: None,
            create_started: std::time::Instant::now(),
            #[cfg(test)]
            create_for_test: None,
            admission: None,
            presentation: None,
            text: None,
            back_to: SyncDialogState::default(),
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

    /// Makes the next Add device use `task` instead of contacting a server.
    #[cfg(test)]
    pub(super) fn create_with_for_test(&mut self, task: JoinHandle<Result<PendingInvitation>>) {
        self.create_for_test = Some(task);
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
    /// Opens Sync › Add device at once, with Esc returning to the Sync
    /// dialog's first page.
    pub(in crate::tui) fn show_pairing_invitation(&mut self) {
        self.show_pairing_invitation_from(SyncDialogState::default());
    }

    /// Opens Sync › Add device at once, with Esc returning to `back_to`. A new
    /// invitation is created behind a loading state, and its QR code replaces
    /// it in place.
    pub(in crate::tui) fn show_pairing_invitation_from(&mut self, back_to: SyncDialogState) {
        self.invite.back_to = back_to;
        let page = if let Some(presentation) = &self.invite.presentation {
            PairingOverlay::Ready(presentation.clone())
        } else if self.invite.create.is_some() {
            PairingOverlay::Creating {
                started_at: self.invite.create_started,
            }
        } else if self.sync_ops.work_pending() {
            PairingOverlay::Failed(
                "Sync is busy. Try again when it finishes; :sync shows its progress.".into(),
            )
        } else {
            self.invite.create = Some(self.spawn_create_invitation());
            self.invite.create_started = std::time::Instant::now();
            PairingOverlay::Creating {
                started_at: self.invite.create_started,
            }
        };
        self.overlay = Some(OverlayState::Pairing(page));
    }

    /// Retries a failed Add device, keeping the page Esc returns to.
    pub(in crate::tui) fn retry_pairing_invitation(&mut self) {
        let back_to = std::mem::take(&mut self.invite.back_to);
        self.show_pairing_invitation_from(back_to);
    }

    /// Leaves Add device for the Sync dialog page it was opened from. Waiting
    /// for admission continues, as when the page is closed.
    pub(in crate::tui) fn leave_pairing_invitation(&mut self) {
        self.pending_shortcut.clear();
        let back_to = std::mem::take(&mut self.invite.back_to);
        self.overlay = Some(OverlayState::Sync(back_to));
    }

    fn spawn_create_invitation(&mut self) -> JoinHandle<Result<PendingInvitation>> {
        #[cfg(test)]
        if let Some(task) = self.invite.create_for_test.take() {
            return task;
        }
        let database = self.store.database();
        let config = self.intake.config().clone();
        tokio::spawn(async move { encrypted::create_invitation(&database, &config).await })
    }

    fn creating_page_open(&self) -> bool {
        matches!(
            self.overlay,
            Some(OverlayState::Pairing(PairingOverlay::Creating { .. }))
        )
    }

    /// Reports a creation failure on the Add device page while it waits, or
    /// as a notification once it was closed.
    fn invitation_failed(&mut self, page_message: String, notification: String) {
        if self.creating_page_open() {
            self.overlay = Some(OverlayState::Pairing(PairingOverlay::Failed(page_message)));
        } else {
            self.set_error(notification);
        }
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
            match task
                .await
                .context("invitation task stopped")
                .and_then(|result| result)
            {
                Ok(invitation) => match encrypted::tui_presentation(
                    &invitation,
                    crate::pairing::qr_glyphs(self.intake.config().sync.qr_glyphs),
                ) {
                    Ok(presentation) => {
                        let presentation = Arc::new(presentation);
                        self.store.sync_status.invitation = Some(encrypted::InvitationStatus {
                            expires_at: invitation.expires_at(),
                            keys_may_have_been_sent: false,
                        });
                        self.invite.presentation = Some(presentation.clone());
                        self.invite.text = Some(Zeroizing::new(invitation.text().to_string()));
                        // A closed page stays closed; the header shows the
                        // open invitation and Add device reopens its QR code.
                        if self.creating_page_open() {
                            self.overlay =
                                Some(OverlayState::Pairing(PairingOverlay::Ready(presentation)));
                        }
                        let database = self.store.database();
                        let config = self.intake.config().clone();
                        self.invite.admission = Some(tokio::spawn(async move {
                            encrypted::await_admission(&database, &config, &invitation).await
                        }));
                    }
                    Err(error) => {
                        let message =
                            format!("QR code unavailable: {error:#}; run `aven sync invite`");
                        self.invitation_failed(message.clone(), message);
                    }
                },
                Err(error) if crate::tui::sync_errors::reached_change_limit(&error) => {
                    let message = crate::tui::sync_errors::CHANGE_LIMIT.to_string();
                    self.invitation_failed(message.clone(), message);
                }
                Err(error) => {
                    self.store.refresh_sync_status().await?;
                    let message = crate::tui::sync_errors::failure(
                        crate::tui::sync_operations::OperationKind::ListDevices,
                        &error,
                    )
                    .message;
                    self.invitation_failed(
                        message.clone(),
                        format!("invitation unavailable: {message}"),
                    );
                }
            }
            return Ok(true);
        }
        let Some(task) = self.invite.admission.take_if(|task| task.is_finished()) else {
            return Ok(false);
        };
        self.invite.presentation = None;
        self.invite.text = None;
        if matches!(
            self.overlay,
            Some(OverlayState::Pairing(PairingOverlay::Ready(_)))
        ) {
            self.overlay = None;
        }
        match task
            .await
            .context("invitation task stopped")
            .and_then(|result| result)
        {
            Ok(encrypted::Admission::Admitted) => self.set_success("device added"),
            Ok(encrypted::Admission::Cancelled) => self.set_info("invitation cancelled"),
            Err(error) => {
                self.store.refresh_sync_status().await?;
                let status =
                    encrypted::invitation_status(&self.store.database(), self.intake.config())
                        .await?;
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
            encrypted::invitation_status(&self.store.database(), self.intake.config()).await?;
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
