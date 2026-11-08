//! The app's own updates, over the release checks and bundle staging that
//! `aven update` uses. The app calls these from its background executor.
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use aven_desktop::updates::{Available, Process, Progress, Relaunched, Updates};
use aven_host::config::{AppConfig, resolve_db_path};
use aven_host::running_processes::blocking_processes;

use super::bundle::{self, BundleLayout, StagedBundle};
use super::install::DESKTOP_ARCHIVE_LIMIT;
use super::relaunch::{self, LaunchOutcome, PendingUpdate, UpdateState};
use super::{
    CURRENT_VERSION, CheckOutcome, InstallMethod, REPOSITORY, Release, ReleaseChannel, UpdatePhase,
};
use crate::app_bundle::{BUNDLE_EXECUTABLE, enclosing_bundle};

pub(crate) struct DesktopUpdater {
    runtime: tokio::runtime::Runtime,
    /// The release the last check offered.
    release: Mutex<Option<Release>>,
    /// The downloaded release, waiting for a restart.
    staged: Mutex<Option<(Release, StagedBundle)>>,
}

impl DesktopUpdater {
    pub(crate) fn new() -> Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("aven-update")
            .enable_all()
            .build()
            .context("start the update runtime")?;
        Ok(Self {
            runtime,
            release: Mutex::new(None),
            staged: Mutex::new(None),
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

/// The bundle this process runs from.
fn running_bundle() -> Result<PathBuf> {
    let executable = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .context("find the running executable")?;
    enclosing_bundle(&executable).context("Aven is not running from Aven.app")
}

fn notes_url(tag: &str) -> String {
    format!("https://github.com/{REPOSITORY}/releases/tag/{tag}")
}

/// Why this installation cannot install `release` itself, in the app's words.
fn blocked_reason(release: &Release) -> Option<String> {
    let plan = super::install_plan(release.clone());
    match &plan.method {
        InstallMethod::Direct { .. } => None,
        InstallMethod::Unwritable { target } => Some(unwritable_reason(
            &enclosing_bundle(target).unwrap_or_else(|| target.clone()),
        )),
        // The first line names the versions, which the dialog shows already.
        _ => plan
            .guidance()
            .map(|lines| lines.into_iter().skip(1).collect::<Vec<_>>().join(" ")),
    }
}

fn unwritable_reason(bundle: &Path) -> String {
    let remedy = super::unwritable_bundle_remedy(bundle);
    format!(
        "Aven can\u{2019}t replace itself at {}. {remedy}",
        bundle.display()
    )
}

impl Updates for DesktopUpdater {
    fn version(&self) -> String {
        CURRENT_VERSION.to_string()
    }

    fn supported(&self) -> bool {
        super::eligibility::release_channel() == ReleaseChannel::Desktop
    }

    fn check(&self, force: bool) -> Result<Option<Available>> {
        let client = super::client()?;
        let outcome = self
            .runtime
            .block_on(super::check_for_update(&client, force))?;
        let release = match outcome {
            CheckOutcome::Current { .. } => {
                *lock(&self.release) = None;
                return Ok(None);
            }
            CheckOutcome::Available { release, .. } => release,
        };
        let available = Available {
            version: release.version.to_string(),
            notes_url: notes_url(&release.tag),
            blocked: blocked_reason(&release),
        };
        *lock(&self.release) = Some(release);
        Ok(Some(available))
    }

    fn download(
        &self,
        version: &str,
        progress: &(dyn Fn(Progress) + Send + Sync),
        cancelled: Arc<AtomicBool>,
    ) -> Result<()> {
        let release = lock(&self.release)
            .clone()
            .filter(|release| release.version.to_string() == version)
            .context("this update is no longer offered")?;
        let plan = super::install_plan(release.clone());
        let target = plan
            .direct_target()
            .context("this installation cannot update itself")?;
        let layout = match BundleLayout::for_target(target, &release, None)? {
            Some(layout @ BundleLayout::Update { .. }) => layout,
            _ => anyhow::bail!("Aven is not running from Aven.app"),
        };
        // A new download replaces one that was waiting.
        lock(&self.staged).take();
        let client = super::download_client()?;
        let (sender, mut receiver) = super::progress_channel();
        let staged = self.runtime.block_on(async {
            let stage = bundle::stage(
                &client,
                &release,
                layout,
                DESKTOP_ARCHIVE_LIMIT,
                &sender,
                &cancelled,
            );
            tokio::pin!(stage);
            loop {
                tokio::select! {
                    staged = &mut stage => break staged,
                    changed = receiver.changed() => {
                        if changed.is_err() {
                            continue;
                        }
                        let update = receiver.borrow_and_update().clone();
                        progress(Progress {
                            downloaded: update.downloaded,
                            total: update.total,
                            verifying: update.phase != UpdatePhase::Downloading,
                        });
                    }
                }
            }
        })?;
        super::ensure_not_cancelled(&cancelled)?;
        *lock(&self.staged) = Some((release, staged));
        Ok(())
    }

    fn discard(&self) {
        lock(&self.staged).take();
    }

    fn blockers(&self) -> Vec<Process> {
        let Ok(bundle) = running_bundle() else {
            return Vec::new();
        };
        blocking_processes(&bundle.join(BUNDLE_EXECUTABLE))
            .into_iter()
            .map(|process| Process {
                pid: process.pid,
                command: process.command,
            })
            .collect()
    }

    fn restart(&self) -> Result<()> {
        let (release, staged) = lock(&self.staged)
            .take()
            .context("the update is no longer downloaded")?;
        let database = AppConfig::load()
            .and_then(|config| resolve_db_path(None, &config))
            .ok();
        let schema = database.as_deref().and_then(|database| {
            self.runtime
                .block_on(relaunch::schema_mark(database))
                .inspect_err(|error| {
                    tracing::warn!(error = %format_args!("{error:#}"), "could not read the schema");
                })
                .ok()
        });
        let bundle = staged.bundle().to_path_buf();
        let update = PendingUpdate {
            from: super::current_version(),
            to: release.version,
            bundle: bundle.clone(),
            database,
            schema,
            state: UpdateState::Pending,
        };
        let marker = relaunch::activate_keeping_previous(staged, update)?;
        if let Err(error) = relaunch::spawn_finisher(&marker, &bundle, std::process::id()) {
            // Nothing would open the new version, so the old one comes back.
            return Err(match relaunch::restore_previous(&bundle) {
                Ok(()) => error,
                Err(undo) => error.context(format!(
                    "restoring the previous Aven.app also failed: {undo:#}"
                )),
            });
        }
        Ok(())
    }

    fn relaunched(&self) -> Result<Option<Relaunched>> {
        let bundle = running_bundle()?;
        let outcome = relaunch::confirm_launch(&bundle, &super::current_version())?;
        Ok(outcome.map(|outcome| match outcome {
            LaunchOutcome::Updated { to } => Relaunched::Updated {
                version: to.to_string(),
            },
            LaunchOutcome::Restored { failed, restored } => Relaunched::Restored {
                failed: failed.to_string(),
                restored: restored.to_string(),
            },
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unwritable_app_in_applications_is_not_told_to_move_there() {
        let reason = unwritable_reason(Path::new("/Applications/Aven.app"));
        assert!(!reason.contains("Move Aven to Applications"));
        assert!(reason.contains("Download the new version"));

        let elsewhere = unwritable_reason(Path::new("/Users/me/Apps/Aven.app"));
        assert!(elsewhere.contains("Move Aven to Applications"));
    }
}
