use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use semver::Version;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::tui::app::App;
use crate::tui::overlay::{ConfirmIntent, OverlayState, UpdateOverlayState};
use crate::update::{self, CheckOutcome, InstallPlan, InstallSuccess, Release, UpdateProgress};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateBadgeView {
    pub(crate) label: String,
    pub(crate) restart: bool,
}

#[derive(Debug)]
enum UpdateAvailability {
    Unknown,
    Available(Release),
    Restart(Version),
}

pub(super) struct UpdateController {
    availability: UpdateAvailability,
    check: Option<JoinHandle<Result<CheckOutcome>>>,
    check_explicit: bool,
    install: Option<JoinHandle<Result<InstallSuccess>>>,
    progress: Option<watch::Receiver<UpdateProgress>>,
    cancelled: Option<Arc<AtomicBool>>,
}

#[cfg(not(test))]
fn initial_availability() -> UpdateAvailability {
    update::cached_update()
        .map(UpdateAvailability::Available)
        .unwrap_or(UpdateAvailability::Unknown)
}

#[cfg(test)]
fn initial_availability() -> UpdateAvailability {
    UpdateAvailability::Unknown
}

impl UpdateController {
    pub(super) fn new() -> Self {
        Self {
            availability: initial_availability(),
            check: None,
            check_explicit: false,
            install: None,
            progress: None,
            cancelled: None,
        }
    }

    pub(super) fn badge(&self) -> Option<UpdateBadgeView> {
        match &self.availability {
            UpdateAvailability::Unknown => None,
            UpdateAvailability::Available(release) => Some(UpdateBadgeView {
                label: format!("update available v{}", release.version),
                restart: false,
            }),
            UpdateAvailability::Restart(version) => Some(UpdateBadgeView {
                label: format!("restart for v{version}"),
                restart: true,
            }),
        }
    }

    pub(super) fn changelog_ref(&self) -> String {
        match &self.availability {
            UpdateAvailability::Available(release) => release.tag.clone(),
            UpdateAvailability::Restart(version) => format!("v{version}"),
            UpdateAvailability::Unknown => format!("v{}", update::CURRENT_VERSION),
        }
    }

    pub(super) fn work_pending(&self) -> bool {
        self.check.is_some() || self.install.is_some()
    }
}

impl App {
    pub(crate) fn start_update_check(&mut self) {
        if update::automatic_checks_disabled() || !update::background_check_due() {
            return;
        }
        self.spawn_update_check(false);
    }

    pub(super) fn begin_update(&mut self) {
        if let UpdateAvailability::Restart(version) = &self.update.availability {
            self.overlay = Some(OverlayState::Update(UpdateOverlayState::Success {
                version: version.to_string(),
            }));
            return;
        }
        self.overlay = Some(OverlayState::Update(UpdateOverlayState::Checking));
        self.spawn_update_check(true);
    }

    fn spawn_update_check(&mut self, explicit: bool) {
        if self.update.check.is_some() {
            if explicit {
                self.update.check_explicit = true;
                self.overlay = Some(OverlayState::Update(UpdateOverlayState::Checking));
            }
            return;
        }
        self.update.check_explicit = explicit;
        self.update.check = Some(tokio::spawn(async move {
            let client = update::client()?;
            update::check_for_update(&client, explicit).await
        }));
    }

    pub(super) async fn poll_update(&mut self) -> bool {
        let mut changed = false;
        if let Some(progress) = &self.update.progress
            && let Some(OverlayState::Update(UpdateOverlayState::Progress {
                version,
                phase,
                cancelling,
            })) = self.overlay.as_mut()
        {
            let next = *progress.borrow();
            if *phase != next.phase {
                *phase = next.phase;
                *cancelling = self
                    .update
                    .cancelled
                    .as_ref()
                    .is_some_and(|flag| flag.load(Ordering::Relaxed));
                let _ = version;
                changed = true;
            }
        }

        if self
            .update
            .check
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            let explicit = self.update.check_explicit;
            let result = self.update.check.take().expect("checked above").await;
            match result {
                Ok(Ok(CheckOutcome::Available { release, cached })) => {
                    self.update.availability = UpdateAvailability::Available(release.clone());
                    if explicit {
                        self.present_update_release(release, cached);
                    }
                }
                Ok(Ok(CheckOutcome::Current { version, cached })) => {
                    self.update.availability = UpdateAvailability::Unknown;
                    if explicit {
                        self.overlay = Some(OverlayState::Update(UpdateOverlayState::Current {
                            version: version.to_string(),
                            cached,
                        }));
                    }
                }
                Ok(Err(error)) if explicit => {
                    if let Some(release) = update::cached_update() {
                        self.update.availability = UpdateAvailability::Available(release.clone());
                        self.present_update_release(release, true);
                    } else {
                        self.overlay = Some(OverlayState::Update(UpdateOverlayState::Failed {
                            message: format!("Could not check for updates: {error:#}"),
                        }));
                    }
                }
                Err(error) if explicit && !error.is_cancelled() => {
                    self.overlay = Some(OverlayState::Update(UpdateOverlayState::Failed {
                        message: format!("Update check stopped: {error}"),
                    }));
                }
                _ => {}
            }
            changed = true;
        }

        if self
            .update
            .install
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            let result = self.update.install.take().expect("checked above").await;
            self.update.progress = None;
            self.update.cancelled = None;
            match result {
                Ok(Ok(success)) => {
                    self.update.availability = UpdateAvailability::Restart(success.version.clone());
                    self.overlay = Some(OverlayState::Update(UpdateOverlayState::Success {
                        version: success.version.to_string(),
                    }));
                }
                Ok(Err(error)) if error.to_string() == "update cancelled" => {
                    self.overlay = Some(OverlayState::Update(UpdateOverlayState::Cancelled));
                }
                Ok(Err(error)) => {
                    self.overlay = Some(OverlayState::Update(UpdateOverlayState::Failed {
                        message: format!("Update failed: {error:#}"),
                    }));
                }
                Err(error) if error.is_cancelled() => {
                    self.overlay = Some(OverlayState::Update(UpdateOverlayState::Cancelled));
                }
                Err(error) => {
                    self.overlay = Some(OverlayState::Update(UpdateOverlayState::Failed {
                        message: format!("Update task stopped: {error}"),
                    }));
                }
            }
            changed = true;
        }
        changed
    }

    fn present_update_release(&mut self, release: Release, cached: bool) {
        let plan = update::install_plan(release.clone());
        if let Some(lines) = plan.guidance() {
            self.overlay = Some(OverlayState::Update(UpdateOverlayState::Guidance {
                version: release.version.to_string(),
                lines,
                cached,
            }));
            return;
        }
        let target = plan
            .direct_target()
            .expect("direct plan must have target")
            .display()
            .to_string();
        let cached_note = if cached {
            " Release information is cached because GitHub is unavailable."
        } else {
            ""
        };
        self.overlay = Some(OverlayState::confirm(
            ConfirmIntent::InstallUpdate { plan },
            "Update aven",
            format!(
                "Install aven v{} over v{} at {target}?{cached_note}",
                release.version,
                update::CURRENT_VERSION
            ),
        ));
    }

    pub(super) fn confirm_update(&mut self, plan: InstallPlan) -> Result<()> {
        let version = plan.release.version.to_string();
        let (progress_tx, progress_rx) = update::progress_channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        let client = update::client()?;
        self.update.progress = Some(progress_rx);
        self.update.cancelled = Some(cancelled);
        self.overlay = Some(OverlayState::Update(UpdateOverlayState::Progress {
            version,
            phase: crate::update::UpdatePhase::Downloading,
            cancelling: false,
        }));
        self.update.install = Some(tokio::spawn(async move {
            update::install_direct(client, plan, progress_tx, worker_cancelled).await
        }));
        Ok(())
    }

    pub(super) async fn handle_update_overlay_key(
        &mut self,
        state: UpdateOverlayState,
        key: KeyEvent,
    ) {
        match key.code {
            KeyCode::Esc => {
                if self.update.check.is_some() {
                    if let Some(handle) = self.update.check.take() {
                        handle.abort();
                    }
                    self.overlay = Some(OverlayState::Update(UpdateOverlayState::Cancelled));
                } else if self.update.install.is_some() {
                    let phase = self
                        .update
                        .progress
                        .as_ref()
                        .map(|progress| progress.borrow().phase)
                        .unwrap_or(crate::update::UpdatePhase::Installing);
                    if phase.cancellable() {
                        if let Some(cancelled) = &self.update.cancelled {
                            cancelled.store(true, Ordering::Relaxed);
                        }
                        if let Some(handle) = self.update.install.take() {
                            handle.abort();
                        }
                        self.update.progress = None;
                        self.update.cancelled = None;
                        self.overlay = Some(OverlayState::Update(UpdateOverlayState::Cancelled));
                    } else {
                        self.overlay = Some(OverlayState::Update(state));
                    }
                } else {
                    self.overlay = None;
                }
            }
            KeyCode::Enter
                if matches!(
                    state,
                    UpdateOverlayState::Failed { .. } | UpdateOverlayState::Cancelled
                ) =>
            {
                self.begin_update();
            }
            KeyCode::Char('q') if matches!(state, UpdateOverlayState::Success { .. }) => {
                self.should_quit = true;
                self.overlay = None;
            }
            _ => self.overlay = Some(OverlayState::Update(state)),
        }
    }
}
