use std::path::PathBuf;

use serde::Serialize;

use crate::config::{AppConfig, BackgroundIntent};
use crate::daemon::ServiceStatus;
use crate::host::Role;

/// Stable top-level condition used by daemon status reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    Unavailable,
    /// Sync and reminders are both off, so the daemon has nothing to do.
    Off,
    Unconfigured,
    Healthy,
    /// The daemon runs but does not host the database: another process
    /// does, or none does yet.
    Waiting,
    Degraded,
    Blocked,
    Failed,
}

impl StatusState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Off => "off",
            Self::Unconfigured => "unconfigured",
            Self::Healthy => "healthy",
            Self::Waiting => "waiting",
            Self::Degraded => "degraded",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }
}

/// Versioned, presentation-independent report for `aven daemon status --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaemonStatusReport {
    pub version: u32,
    pub state: StatusState,
    pub platform_supported: bool,
    pub platform: String,
    pub installed: bool,
    pub loaded: Option<bool>,
    pub running: Option<bool>,
    pub executable_matches: Option<bool>,
    /// Aven is turned off in System Settings > General > Login Items &
    /// Extensions. `null` where there's no such setting.
    pub turned_off_in_login_items: Option<bool>,
    /// The config gives the daemon sync or reminders to do.
    pub configuration_valid: bool,
    pub sync_enabled: bool,
    pub reminders_enabled: bool,
    pub paths: DaemonPaths,
    pub guidance: Vec<String>,
    pub sync: aven_core::local_state::DaemonSyncState,
    /// The database's background host, when the database exists.
    pub host: Option<HostReport>,
    /// What the user allowed for Aven's notifications, where it can be read.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notifications: Option<crate::notify::Authorization>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostReport {
    pub state: HostReportState,
    /// The host's own record: the running host's, or the last one's.
    pub status: Option<aven_core::local_state::HostStatus>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HostReportState {
    Running,
    /// The host lock was taken moments ago and its holder is starting up.
    Starting,
    /// The host lock is held, but the host stopped recording heartbeats.
    Unresponsive,
    Stopped,
}

impl HostReportState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Starting => "starting",
            Self::Unresponsive => "not responding",
            Self::Stopped => "not running",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DaemonPaths {
    pub service: Option<PathBuf>,
    pub program: Option<PathBuf>,
    pub current_executable: Option<PathBuf>,
    pub stdout_log: Option<PathBuf>,
    pub stderr_log: Option<PathBuf>,
}

pub fn build_daemon_status(config: &AppConfig, service: ServiceStatus) -> DaemonStatusReport {
    let configuration_valid = config.daemon_has_work();
    let state = if !service.platform_supported {
        StatusState::Unavailable
    } else if !configuration_valid {
        StatusState::Off
    } else if !service.installed {
        StatusState::Unconfigured
    } else if service.program_matches_current == Some(false)
        || service.turned_off_in_login_items == Some(true)
    {
        StatusState::Blocked
    } else if service.loaded == Some(false) || service.running == Some(false) {
        StatusState::Failed
    } else if service.loaded == Some(true) && service.running == Some(true) {
        StatusState::Healthy
    } else {
        StatusState::Degraded
    };
    let mut guidance = Vec::new();
    if state == StatusState::Unavailable {
        guidance.push(
            "The managed daemon is supported on macOS and on Linux with systemd.".to_string(),
        );
    } else if state == StatusState::Off {
        guidance.push(turn_on_guidance(config, service.installed));
    } else if state == StatusState::Unconfigured {
        guidance.push("Install it with `aven daemon install`.".to_string());
    } else {
        if service.turned_off_in_login_items == Some(true) {
            guidance.push(
                "Aven is turned off in System Settings > General > Login Items & Extensions. Turn it on there to run the daemon."
                    .to_string(),
            );
        }
        if service.program_matches_current == Some(false) {
            guidance.push("Repair the executable path with `aven daemon repair`.".to_string());
        }
        if service.bundle_migration_pending == Some(true) {
            guidance.push(
                "Run `aven daemon repair` so Login Items lists the daemon as Aven.".to_string(),
            );
        }
        if state == StatusState::Failed {
            guidance.push(
                "Restart it with `aven daemon restart`; inspect the log paths if it remains stopped."
                    .to_string(),
            );
        } else if state == StatusState::Degraded {
            guidance.push("Run `aven doctor` for additional service diagnostics.".to_string());
        }
    }
    DaemonStatusReport {
        version: 1,
        sync: Default::default(),
        host: None,
        notifications: None,
        state,
        platform_supported: service.platform_supported,
        platform: std::env::consts::OS.to_string(),
        installed: service.installed,
        loaded: service.loaded,
        running: service.running,
        executable_matches: service.program_matches_current,
        turned_off_in_login_items: service.turned_off_in_login_items,
        configuration_valid,
        sync_enabled: config.automatic_sync_is_enabled(),
        reminders_enabled: config.reminders.enabled,
        paths: DaemonPaths {
            service: nonempty_path(service.service_path),
            program: service.program,
            current_executable: nonempty_path(service.current_executable),
            stdout_log: service.stdout_path,
            stderr_log: service.stderr_path,
        },
        guidance,
    }
}

/// How to give a daemon that has nothing to do some work.
fn turn_on_guidance(config: &AppConfig, installed: bool) -> String {
    if config.sync.enabled && !config.sync_is_allowed() {
        return "Sync is turned off by AVEN_SYNC_DISABLED. Unset it to let the daemon sync."
            .to_string();
    }
    let commands =
        "`aven config set sync.enabled true` or `aven config set reminders.enabled true`";
    if installed {
        format!(
            "Sync and reminders are both turned off. Turn one on with {commands}; the daemon picks up the change itself."
        )
    } else if config.background == Some(BackgroundIntent::On) {
        format!(
            "Sync and reminders are both turned off, so Aven Desktop runs no background agent. Turn on Keep syncing after Aven quits or Reminders on this Mac in Aven Desktop's Settings, or run {commands} and then `aven daemon install`."
        )
    } else {
        format!(
            "Sync and reminders are both turned off. Turn one on with {commands}, then run `aven daemon install`."
        )
    }
}

impl DaemonStatusReport {
    pub fn with_sync(mut self, sync: aven_core::local_state::DaemonSyncState) -> Self {
        if self.sync_enabled
            && daemon_sync_blocked(&sync, chrono::Utc::now().timestamp())
            && self.state == StatusState::Healthy
        {
            self.state = StatusState::Blocked;
        }
        self.sync = sync;
        self
    }

    /// A host that holds the lock without a recent heartbeat is a fault. No
    /// other process takes over from it, so the user has to restart it. One
    /// that is still starting is not. A running daemon that isn't the host
    /// is waiting, for the process that hosts or for the lock.
    pub fn with_host(mut self, state: crate::host::HostState) -> Self {
        use crate::host::HostState;
        let daemon_runs = self.running == Some(true)
            && matches!(self.state, StatusState::Healthy | StatusState::Degraded);
        match &state {
            HostState::Running(status) if daemon_runs && status.role != Role::Daemon.as_str() => {
                self.state = StatusState::Waiting;
                self.guidance.push(format!(
                    "{} (pid {}) runs sync and reminders for this database, so the daemon waits and takes over when it stops.",
                    crate::host::describe_role(&status.role),
                    status.pid
                ));
            }
            HostState::Stopped(_) if daemon_runs => {
                self.state = StatusState::Waiting;
                self.guidance.push(
                    "The daemon is running but doesn't host this database yet. It tries again every few seconds; if this lasts, an Aven daemon older than this version may be running, and `aven daemon restart` starts the current one."
                        .to_string(),
                );
            }
            _ => {}
        }
        let (report, status) = match state {
            HostState::Running(status) => (HostReportState::Running, Some(status)),
            HostState::Starting => (HostReportState::Starting, None),
            HostState::Unresponsive(status) => (HostReportState::Unresponsive, status),
            HostState::Stopped(status) => (HostReportState::Stopped, status),
        };
        if report == HostReportState::Unresponsive {
            self.state = StatusState::Failed;
            self.guidance.push(match &status {
                Some(status) => format!(
                    "The background host, {} with pid {}, holds the database but last checked in at {}. Restart it; if it does not stop, end process {}.",
                    crate::host::describe_role(&status.role),
                    status.pid,
                    daemon_time(status.heartbeat_at),
                    status.pid
                ),
                None => "A process holds the database's host lock without recording its status. Restart `aven daemon`, Aven Desktop or `aven mcp --sync`, whichever runs.".to_string(),
            });
        }
        self.host = Some(HostReport {
            state: report,
            status,
        });
        self
    }
}

/// Key storage failures need user action immediately. Other errors must persist
/// for two minutes before being called blocked, so short network retries do not
/// replace the normal pending headline.
pub fn daemon_sync_blocked(state: &aven_core::local_state::DaemonSyncState, now: i64) -> bool {
    state.error.as_ref().is_some_and(|error| {
        error.code.starts_with("protected-key-storage-") || now.saturating_sub(error.since) >= 120
    })
}

pub fn daemon_time(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|| at.to_string())
}

pub fn elapsed_text(seconds: i64) -> String {
    let seconds = seconds.max(0);
    if seconds >= 3600 {
        format!("{}h {}m", seconds / 3600, seconds % 3600 / 60)
    } else if seconds >= 60 {
        format!("{}m {}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

fn nonempty_path(path: PathBuf) -> Option<PathBuf> {
    (!path.as_os_str().is_empty()).then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(installed: bool) -> ServiceStatus {
        ServiceStatus {
            platform_supported: true,
            installed,
            loaded: Some(installed),
            running: Some(installed),
            service_path: PathBuf::from("/service.plist"),
            program: installed.then(|| PathBuf::from("/bin/aven")),
            current_executable: PathBuf::from("/bin/aven"),
            program_matches_current: installed.then_some(true),
            bundle_migration_pending: installed.then_some(false),
            turned_off_in_login_items: installed.then_some(false),
            stdout_path: Some(PathBuf::from("/logs/out")),
            stderr_path: Some(PathBuf::from("/logs/err")),
        }
    }

    fn configured() -> AppConfig {
        let mut config = AppConfig::default();
        config.sync.enabled = true;
        config
    }

    #[test]
    fn daemon_blocking_threshold_and_json_health_are_stable() {
        let mut sync = aven_core::local_state::DaemonSyncState {
            last_success_at: Some(50),
            error: Some(aven_core::local_state::DaemonSyncError {
                code: "encrypted-tail-network".to_string(),
                message: "retry".to_string(),
                since: 100,
            }),
            next_retry_at: Some(400),
        };
        assert!(!daemon_sync_blocked(&sync, 219));
        assert!(daemon_sync_blocked(&sync, 220));
        sync.error.as_mut().unwrap().code = "protected-key-storage-unavailable".to_string();
        assert!(daemon_sync_blocked(&sync, 100));
        let report = build_daemon_status(&configured(), service(true)).with_sync(sync);
        assert_eq!(report.state, StatusState::Blocked);
        let json = serde_json::to_value(report).unwrap();
        assert_eq!(json["sync"]["last_success_at"], 50);
        assert_eq!(json["sync"]["error"]["since"], 100);
        assert_eq!(json["sync"]["next_retry_at"], 400);
        assert!(!daemon_sync_blocked(&Default::default(), 1000));
    }

    #[test]
    fn daemon_states_cover_healthy_missing_mismatched_and_unsupported() {
        assert_eq!(
            build_daemon_status(&configured(), service(true)).state,
            StatusState::Healthy
        );
        assert_eq!(
            build_daemon_status(&configured(), service(false)).state,
            StatusState::Unconfigured
        );
        let mut mismatched = service(true);
        mismatched.program_matches_current = Some(false);
        assert_eq!(
            build_daemon_status(&configured(), mismatched).state,
            StatusState::Blocked
        );
        let mut unsupported = service(false);
        unsupported.platform_supported = false;
        unsupported.loaded = None;
        unsupported.running = None;
        assert_eq!(
            build_daemon_status(&configured(), unsupported).state,
            StatusState::Unavailable
        );
    }

    #[test]
    fn daemon_states_cover_stopped() {
        let mut stopped = service(true);
        stopped.running = Some(false);
        assert_eq!(
            build_daemon_status(&configured(), stopped).state,
            StatusState::Failed
        );
    }

    #[test]
    fn a_daemon_without_sync_or_reminders_is_off_and_says_how_to_turn_it_on() {
        let idle = build_daemon_status(&AppConfig::default(), service(true));
        assert_eq!(idle.state, StatusState::Off);
        assert!(!idle.configuration_valid);
        assert_eq!(idle.guidance.len(), 1, "{:?}", idle.guidance);
        assert!(idle.guidance[0].contains("aven config set sync.enabled true"));
        assert!(idle.guidance[0].contains("picks up the change"));

        // Sync turned off when joining from Aven Desktop removes the agent.
        let desktop = AppConfig {
            background: Some(BackgroundIntent::On),
            ..AppConfig::default()
        };
        let mut removed = service(false);
        removed.loaded = Some(false);
        removed.running = Some(false);
        let off = build_daemon_status(&desktop, removed.clone());
        assert_eq!(off.state, StatusState::Off);
        assert_eq!(off.guidance.len(), 1, "{:?}", off.guidance);
        assert!(off.guidance[0].contains("Keep syncing after Aven quits"));
        assert!(off.guidance[0].contains("aven daemon install"));
        assert!(!off.guidance[0].contains("restart"));

        let terminal = build_daemon_status(&AppConfig::default(), removed);
        assert_eq!(terminal.state, StatusState::Off);
        assert!(!terminal.guidance[0].contains("Aven Desktop"));
        assert!(terminal.guidance[0].contains("aven daemon install"));

        let mut overridden = configured();
        overridden.sync.disable_override = true;
        let overridden = build_daemon_status(&overridden, service(true));
        assert_eq!(overridden.state, StatusState::Off);
        assert!(overridden.guidance[0].contains("AVEN_SYNC_DISABLED"));

        let json = serde_json::to_value(off).unwrap();
        assert_eq!(json["state"], "off");
    }

    fn host_status(role: &str, pid: u32) -> aven_core::local_state::HostStatus {
        aven_core::local_state::HostStatus {
            pid,
            role: role.to_string(),
            version: "0.1.0".to_string(),
            capabilities: vec!["sync".to_string()],
            started_at: 0,
            heartbeat_at: 0,
            round_started: 0,
            round_finished: 0,
        }
    }

    #[test]
    fn a_running_daemon_that_does_not_host_is_waiting_and_says_why() {
        let running = build_daemon_status(&configured(), service(true));
        let hosting = running
            .clone()
            .with_host(crate::host::HostState::Running(host_status("daemon", 7)));
        assert_eq!(hosting.state, StatusState::Healthy);
        assert!(hosting.guidance.is_empty());

        let behind_desktop = running
            .clone()
            .with_host(crate::host::HostState::Running(host_status("desktop", 9)));
        assert_eq!(behind_desktop.state, StatusState::Waiting);
        assert_eq!(behind_desktop.guidance.len(), 1);
        assert!(behind_desktop.guidance[0].contains("Aven Desktop (pid 9)"));
        assert!(behind_desktop.guidance[0].contains("takes over"));
        assert!(!behind_desktop.guidance[0].contains("restart"));

        let without_lock = running
            .clone()
            .with_host(crate::host::HostState::Stopped(None));
        assert_eq!(without_lock.state, StatusState::Waiting);
        assert!(without_lock.guidance[0].contains("doesn't host this database"));
        assert_eq!(
            serde_json::to_value(&without_lock).unwrap()["state"],
            "waiting"
        );

        // A daemon that isn't running isn't waiting.
        let mut stopped = service(true);
        stopped.running = Some(false);
        let stopped = build_daemon_status(&configured(), stopped)
            .with_host(crate::host::HostState::Running(host_status("desktop", 9)));
        assert_eq!(stopped.state, StatusState::Failed);
        let off = build_daemon_status(&AppConfig::default(), service(true))
            .with_host(crate::host::HostState::Stopped(None));
        assert_eq!(off.state, StatusState::Off);
    }

    #[test]
    fn turned_off_in_login_items_blocks_the_daemon_and_says_where() {
        let mut turned_off = service(true);
        turned_off.loaded = Some(false);
        turned_off.running = Some(false);
        turned_off.turned_off_in_login_items = Some(true);
        let status = build_daemon_status(&configured(), turned_off);
        assert_eq!(status.state, StatusState::Blocked);
        assert_eq!(status.turned_off_in_login_items, Some(true));
        assert_eq!(status.guidance.len(), 1, "{:?}", status.guidance);
        assert!(status.guidance[0].contains("Login Items & Extensions"));
    }

    #[test]
    fn pending_bundle_migration_suggests_repair_without_changing_state() {
        let mut pending = service(true);
        pending.bundle_migration_pending = Some(true);
        let status = build_daemon_status(&configured(), pending);
        assert_eq!(status.state, StatusState::Healthy);
        assert_eq!(
            status.guidance,
            ["Run `aven daemon repair` so Login Items lists the daemon as Aven."]
        );
        assert!(
            build_daemon_status(&configured(), service(true))
                .guidance
                .is_empty()
        );
    }

    #[test]
    fn an_unresponsive_host_is_a_failure_with_guidance() {
        let status = aven_core::local_state::HostStatus {
            pid: 42,
            role: "daemon".to_string(),
            version: "0.1.0".to_string(),
            capabilities: vec!["sync".to_string()],
            started_at: 0,
            heartbeat_at: 0,
            round_started: 0,
            round_finished: 0,
        };
        let healthy = build_daemon_status(&configured(), service(true));
        let running = healthy
            .clone()
            .with_host(crate::host::HostState::Running(status.clone()));
        assert_eq!(running.state, StatusState::Healthy);
        assert_eq!(running.host.unwrap().state, HostReportState::Running);

        let starting = healthy.clone().with_host(crate::host::HostState::Starting);
        assert_eq!(starting.state, StatusState::Healthy);
        assert!(starting.guidance.is_empty());
        assert_eq!(starting.host.unwrap().state, HostReportState::Starting);

        let stuck = healthy.with_host(crate::host::HostState::Unresponsive(Some(status)));
        assert_eq!(stuck.state, StatusState::Failed);
        assert!(
            stuck
                .guidance
                .iter()
                .any(|line| line.contains("end process 42"))
        );
        assert_eq!(stuck.host.unwrap().state, HostReportState::Unresponsive);
    }
}
