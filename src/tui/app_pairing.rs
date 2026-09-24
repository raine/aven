use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::task::JoinHandle;

use crate::pairing::PairingPresentation;
use crate::sync::encrypted::{self, PendingInvitation};
use crate::tui::app::{App, Notification};
use crate::tui::overlay::OverlayState;

/// Creates a device invitation and waits for its admission in the background.
/// Dismissing the QR overlay keeps waiting, as `aven sync invite` does, until
/// the device joins, the invitation expires, or the TUI exits.
pub(super) struct InviteController {
    create: Option<JoinHandle<Result<PendingInvitation>>>,
    admission: Option<JoinHandle<Result<()>>>,
    presentation: Option<Arc<PairingPresentation>>,
}

impl InviteController {
    pub(super) fn new() -> Self {
        Self {
            create: None,
            admission: None,
            presentation: None,
        }
    }

    pub(super) fn work_pending(&self) -> bool {
        self.create.is_some() || self.admission.is_some()
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
                Ok(invitation) => match invitation.presentation() {
                    Ok(presentation) => {
                        let presentation = Arc::new(presentation);
                        self.invite.presentation = Some(presentation.clone());
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
                Err(error) => self.set_error(format!("invitation unavailable: {error:#}")),
            }
            return Ok(true);
        }
        let Some(task) = self.invite.admission.take_if(|task| task.is_finished()) else {
            return Ok(false);
        };
        self.invite.presentation = None;
        if matches!(self.overlay, Some(OverlayState::Pairing(_))) {
            self.overlay = None;
        }
        match task
            .await
            .context("invitation task stopped")
            .and_then(|result| result)
        {
            Ok(()) => self.set_success("device added"),
            Err(error) => self.set_warning(format!("{error:#}")),
        }
        Ok(true)
    }
}
