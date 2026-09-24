use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Size;
use semver::Version;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::tui::app::App;
use crate::tui::overlay::{OverlayState, UpdateActionFocus, UpdateNotesState, UpdateOverlayState};
use crate::update::{self, CheckOutcome, InstallPlan, InstallSuccess, Release, UpdateProgress};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateBadgeView {
    pub(crate) label: String,
    pub(crate) restart: bool,
}

#[derive(Debug)]
enum UpdateAvailability {
    Unknown,
    Available { release: Release, cached: bool },
    Restart(Version),
}

pub(super) struct UpdateController {
    availability: UpdateAvailability,
    automatic_checks: bool,
    check: Option<JoinHandle<Result<CheckOutcome>>>,
    check_explicit: bool,
    install: Option<JoinHandle<Result<InstallSuccess>>>,
    progress: Option<watch::Receiver<UpdateProgress>>,
    cancelled: Option<Arc<AtomicBool>>,
    dismissal: Option<update::UpdateDismissal>,
}

#[cfg(not(test))]
fn initial_availability() -> UpdateAvailability {
    update::cached_update()
        .map(|release| UpdateAvailability::Available {
            release,
            cached: true,
        })
        .unwrap_or(UpdateAvailability::Unknown)
}

#[cfg(test)]
fn initial_availability() -> UpdateAvailability {
    UpdateAvailability::Unknown
}

#[cfg(not(test))]
fn initial_dismissal() -> Option<update::UpdateDismissal> {
    update::cached_dismissal()
}

#[cfg(test)]
fn initial_dismissal() -> Option<update::UpdateDismissal> {
    None
}

impl UpdateController {
    pub(super) fn new(automatic_checks: bool) -> Self {
        Self {
            availability: initial_availability(),
            automatic_checks,
            check: None,
            check_explicit: false,
            install: None,
            progress: None,
            cancelled: None,
            dismissal: initial_dismissal(),
        }
    }

    pub(super) fn badge(&self) -> Option<UpdateBadgeView> {
        match &self.availability {
            UpdateAvailability::Unknown => None,
            UpdateAvailability::Available { .. } if !self.automatic_checks => None,
            UpdateAvailability::Available { release, .. }
                if !self
                    .dismissal
                    .as_ref()
                    .is_some_and(|dismissal| dismissal.applies_to(&release.version)) =>
            {
                Some(UpdateBadgeView {
                    label: format!("update available v{}", release.version),
                    restart: false,
                })
            }
            UpdateAvailability::Available { .. } => None,
            UpdateAvailability::Restart(version) => Some(UpdateBadgeView {
                label: format!("restart for v{version}"),
                restart: true,
            }),
        }
    }

    pub(super) fn work_pending(&self) -> bool {
        self.check.is_some() || self.install.is_some()
    }

    pub(super) fn set_automatic_checks(&mut self, enabled: bool) {
        self.automatic_checks = enabled;
    }
}

fn update_guidance_copy(plan: &InstallPlan) -> Option<String> {
    let lines = plan.guidance()?;
    Some(
        lines
            .iter()
            .find_map(|line| line.strip_prefix("Run: ").map(str::to_string))
            .unwrap_or_else(|| lines.join("\n")),
    )
}

fn should_start_automatic_check(enabled: bool, due: bool) -> bool {
    enabled && due
}

impl App {
    pub(crate) fn start_update_check(&mut self) {
        if !should_start_automatic_check(
            self.update.automatic_checks,
            update::background_check_due(),
        ) {
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
        if let UpdateAvailability::Available { release, cached } = &self.update.availability {
            self.show_update_release(release.clone(), *cached);
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
                    self.update.availability = UpdateAvailability::Available {
                        release: release.clone(),
                        cached,
                    };
                    if explicit {
                        self.show_update_release(release, cached);
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
                        self.update.availability = UpdateAvailability::Available {
                            release: release.clone(),
                            cached: true,
                        };
                        self.show_update_release(release, true);
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
        terminal_size: Size,
    ) {
        if let UpdateOverlayState::Available {
            plan,
            notes,
            mut scroll,
            mut focus,
            cached,
        } = state
        {
            match key.code {
                KeyCode::Esc => {
                    self.update.dismissal = Some(update::dismiss_update(plan.release.version));
                }
                KeyCode::Tab | KeyCode::BackTab | KeyCode::Left | KeyCode::Right => {
                    focus = match focus {
                        UpdateActionFocus::Later => UpdateActionFocus::Primary,
                        UpdateActionFocus::Primary => UpdateActionFocus::Later,
                    };
                    self.overlay = Some(OverlayState::Update(UpdateOverlayState::Available {
                        plan,
                        notes,
                        scroll,
                        focus,
                        cached,
                    }));
                }
                KeyCode::Enter | KeyCode::Char(' ') => match focus {
                    UpdateActionFocus::Later => {
                        self.update.dismissal = Some(update::dismiss_update(plan.release.version));
                    }
                    UpdateActionFocus::Primary => {
                        if let Some(command) = update_guidance_copy(&plan) {
                            match crate::tui::platform::copy_to_clipboard(&command) {
                                Ok(()) => self.set_info("update command copied"),
                                Err(error) => self.set_warning(format!(
                                    "could not copy update command: {error:#}"
                                )),
                            }
                            self.overlay =
                                Some(OverlayState::Update(UpdateOverlayState::Available {
                                    plan,
                                    notes,
                                    scroll,
                                    focus,
                                    cached,
                                }));
                        } else if let Err(error) = self.confirm_update(plan) {
                            self.set_error(format!("could not start update: {error:#}"));
                        }
                    }
                },
                KeyCode::Char('r') if matches!(notes, UpdateNotesState::Failed) => {
                    self.show_update_release(plan.release.clone(), cached);
                }
                code => {
                    let page_rows = crate::tui::ui::update_dialog_size(terminal_size)
                        .1
                        .saturating_sub(7);
                    let half_page = page_rows.saturating_add(1) / 2;
                    let delta = match code {
                        KeyCode::Char('j') | KeyCode::Down => Some(1),
                        KeyCode::Char('k') | KeyCode::Up => Some(-1),
                        KeyCode::Char('d') => Some(half_page as isize),
                        KeyCode::Char('u') => Some(-(half_page as isize)),
                        KeyCode::PageDown => Some(page_rows as isize),
                        KeyCode::PageUp => Some(-(page_rows as isize)),
                        KeyCode::Home | KeyCode::Char('g') => {
                            scroll = 0;
                            None
                        }
                        KeyCode::End | KeyCode::Char('G') => {
                            scroll = crate::tui::ui::update_notes_scroll_cap(&notes, terminal_size);
                            None
                        }
                        _ => None,
                    };
                    if let Some(delta) = delta {
                        let cap = crate::tui::ui::update_notes_scroll_cap(&notes, terminal_size);
                        scroll = crate::tui::navigation::scroll_with_delta(scroll, delta, cap);
                    }
                    self.overlay = Some(OverlayState::Update(UpdateOverlayState::Available {
                        plan,
                        notes,
                        scroll,
                        focus,
                        cached,
                    }));
                }
            }
            return;
        }

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

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: u64) -> Release {
        Release {
            version: Version::new(version, 0, 0),
            tag: format!("v{version}.0.0"),
            archive_name: "aven-test.tar.gz".to_string(),
            archive_url: "https://example.com/aven-test.tar.gz".to_string(),
            checksum_url: "https://example.com/aven-test.sha256".to_string(),
        }
    }

    #[test]
    fn disabled_automatic_updates_suppress_checks_and_badges() {
        assert!(!should_start_automatic_check(false, true));

        let mut controller = UpdateController::new(false);
        controller.availability = UpdateAvailability::Available {
            release: release(2),
            cached: false,
        };

        assert!(controller.badge().is_none());
    }

    #[test]
    fn later_hides_only_the_dismissed_update_badge() {
        let mut controller = UpdateController::new(true);
        controller.availability = UpdateAvailability::Available {
            release: release(2),
            cached: false,
        };
        assert!(controller.badge().is_some());

        controller.dismissal = Some(update::UpdateDismissal::for_test(
            Version::new(2, 0, 0),
            u64::MAX,
        ));
        assert!(controller.badge().is_none());

        controller.availability = UpdateAvailability::Available {
            release: release(3),
            cached: false,
        };
        assert!(controller.badge().is_some());
    }
}
