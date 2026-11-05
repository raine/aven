#[cfg(target_os = "macos")]
mod bundle;
mod cache;
#[cfg(all(target_os = "macos", feature = "desktop"))]
mod desktop;
mod eligibility;
mod install;
#[cfg(target_os = "macos")]
mod processes;
// The app swaps itself in only in desktop builds; the rest keeps its tests.
#[cfg(target_os = "macos")]
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
mod relaunch;
mod release;

use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use anyhow::{Context, Result, bail};
use semver::Version;
use serde::{Deserialize, Serialize};
use tokio::sync::watch;

#[cfg(not(test))]
pub(crate) use cache::cached_dismissal;
pub(crate) use cache::{
    UpdateDismissal, background_check_due, cached_update, check_for_update, dismiss_update,
};
#[cfg(all(target_os = "macos", feature = "desktop"))]
pub(crate) use desktop::DesktopUpdater;
pub(crate) use eligibility::install_plan;
pub(crate) use install::install_direct;
#[cfg(target_os = "macos")]
pub(crate) use relaunch::finish_desktop_update;
pub(crate) use release::ReleaseChannel;

pub(crate) const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
pub(crate) const REPOSITORY: &str = "raine/aven";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Release {
    pub(crate) version: Version,
    pub(crate) tag: String,
    pub(crate) archive_name: String,
    pub(crate) archive_url: String,
    pub(crate) checksum_url: String,
    /// The macOS `Aven.app` archive. Absent on Linux and in releases without it.
    /// Boxed so a release stays small when moved through enums.
    #[serde(default)]
    pub(crate) app: Option<Box<ReleaseArchive>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReleaseArchive {
    pub(crate) name: String,
    pub(crate) url: String,
    pub(crate) checksum_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum InstallMethod {
    Direct { target: PathBuf },
    Homebrew,
    Cargo,
    Nix,
    Development,
    Unsupported { reason: String },
    Unwritable { target: PathBuf },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallPlan {
    pub(crate) release: Release,
    pub(crate) method: InstallMethod,
    pub(crate) channel: ReleaseChannel,
}

impl InstallPlan {
    pub(crate) fn direct_target(&self) -> Option<&std::path::Path> {
        match &self.method {
            InstallMethod::Direct { target } => Some(target),
            _ => None,
        }
    }

    pub(crate) fn guidance(&self) -> Option<Vec<String>> {
        let current = format!("aven v{CURRENT_VERSION}");
        let latest = format!("v{}", self.release.version);
        let lines = match &self.method {
            InstallMethod::Direct { .. } => return None,
            InstallMethod::Homebrew => vec![
                format!("{current} can update to {latest}."),
                "This installation is managed by Homebrew.".to_string(),
                "Run: brew update && brew upgrade aven".to_string(),
                "The formula may take a little longer than the GitHub release to update."
                    .to_string(),
            ],
            InstallMethod::Cargo => vec![
                format!("{current} can update to {latest}."),
                "This installation lives in Cargo's bin directory.".to_string(),
                format!(
                    "Run: cargo install --git https://github.com/{REPOSITORY} --tag {} --locked --force",
                    self.release.tag
                ),
            ],
            InstallMethod::Nix => vec![
                format!("{current} can update to {latest}."),
                "This installation is managed by Nix.".to_string(),
                "Update the flake or profile that provides aven, then rebuild it.".to_string(),
            ],
            InstallMethod::Development => vec![
                format!("{current} can update to {latest}."),
                "This executable is a development build.".to_string(),
                "Pull the repository changes and rebuild aven from source.".to_string(),
            ],
            InstallMethod::Unsupported { reason } => vec![
                format!("{current} can update to {latest}."),
                reason.clone(),
                format!("Build from source: cargo install --git https://github.com/{REPOSITORY}"),
            ],
            InstallMethod::Unwritable { target } if self.channel == ReleaseChannel::Desktop => {
                vec![
                    format!("{current} can update to {latest}."),
                    format!("Aven cannot replace {} here.", target.display()),
                    "Move Aven.app to /Applications, or download the new version and replace it."
                        .to_string(),
                ]
            }
            InstallMethod::Unwritable { target } => vec![
                format!("{current} can update to {latest}."),
                format!("Aven cannot replace {} from the TUI.", target.display()),
                "Use Homebrew or reinstall to a writable directory such as ~/.local/bin."
                    .to_string(),
            ],
        };
        Some(lines)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckOutcome {
    Current { version: Version, cached: bool },
    Available { release: Release, cached: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdatePhase {
    Downloading,
    Verifying,
    Extracting,
    Staging,
    /// Other processes still run this installation's executable.
    Waiting,
    Installing,
}

impl UpdatePhase {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Downloading => "Downloading release",
            Self::Verifying => "Verifying checksum",
            Self::Extracting => "Extracting aven",
            Self::Staging => "Validating new binary",
            Self::Waiting => "Waiting for other aven processes to exit",
            Self::Installing => "Installing update",
        }
    }

    pub(crate) fn cancellable(self) -> bool {
        !matches!(self, Self::Installing)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpdateProgress {
    pub(crate) phase: UpdatePhase,
    /// Bytes of the archive downloaded so far, and its size when known.
    pub(crate) downloaded: u64,
    pub(crate) total: Option<u64>,
    /// While waiting, the processes that must exit first.
    pub(crate) blockers: Vec<Blocker>,
}

impl UpdateProgress {
    pub(crate) fn new(phase: UpdatePhase) -> Self {
        Self {
            phase,
            downloaded: 0,
            total: None,
            blockers: Vec::new(),
        }
    }
}

/// A process that holds up an update of a Desktop install.
#[cfg(target_os = "macos")]
pub(crate) use aven_host::running_processes::RunningProcess as Blocker;
/// Only macOS Desktop installs wait for other processes.
#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Blocker {
    pub(crate) pid: u32,
    pub(crate) command: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InstallSuccess {
    pub(crate) version: Version,
}

pub(crate) fn current_version() -> Version {
    Version::parse(CURRENT_VERSION).expect("package version must be valid semver")
}

pub(crate) fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("aven/{CURRENT_VERSION}"))
        .build()
        .context("build update client")
}

/// A client for whole Desktop archives, which can take longer than a check's
/// overall timeout. Only a stalled read fails it.
pub(crate) fn download_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(5))
        .read_timeout(std::time::Duration::from_secs(30))
        .user_agent(format!("aven/{CURRENT_VERSION}"))
        .build()
        .context("build update download client")
}

pub(crate) fn ensure_not_cancelled(cancelled: &Arc<AtomicBool>) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        bail!("update cancelled");
    }
    Ok(())
}

pub(crate) fn progress_channel() -> (
    watch::Sender<UpdateProgress>,
    watch::Receiver<UpdateProgress>,
) {
    watch::channel(UpdateProgress::new(UpdatePhase::Downloading))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_reported() {
        let cancelled = Arc::new(AtomicBool::new(true));
        assert_eq!(
            ensure_not_cancelled(&cancelled).unwrap_err().to_string(),
            "update cancelled"
        );
    }
}
