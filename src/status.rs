use std::path::PathBuf;

use serde::Serialize;

use crate::config::AppConfig;
use crate::daemon::ServiceStatus;

/// Stable top-level condition used by daemon status reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatusState {
    Unavailable,
    Unconfigured,
    Healthy,
    Degraded,
    Blocked,
    Failed,
}

impl StatusState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Unconfigured => "unconfigured",
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }
}

/// Versioned, presentation-independent report for `aven daemon status --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct DaemonStatusReport {
    pub(crate) version: u32,
    pub(crate) state: StatusState,
    pub(crate) platform_supported: bool,
    pub(crate) platform: String,
    pub(crate) installed: bool,
    pub(crate) loaded: Option<bool>,
    pub(crate) running: Option<bool>,
    pub(crate) executable_matches: Option<bool>,
    pub(crate) configuration_valid: bool,
    pub(crate) sync_enabled: bool,
    pub(crate) wake_address_valid: bool,
    pub(crate) paths: DaemonPaths,
    pub(crate) guidance: Vec<String>,
    pub(crate) sync: aven_core::local_state::DaemonSyncState,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct DaemonPaths {
    pub(crate) service: Option<PathBuf>,
    pub(crate) program: Option<PathBuf>,
    pub(crate) current_executable: Option<PathBuf>,
    pub(crate) stdout_log: Option<PathBuf>,
    pub(crate) stderr_log: Option<PathBuf>,
}

pub(crate) fn build_daemon_status(
    config: &AppConfig,
    service: ServiceStatus,
) -> DaemonStatusReport {
    let wake_address_valid = config.wake_addr().is_ok();
    let configuration_valid = config.daemon_has_work() && wake_address_valid;
    let state = if !service.platform_supported {
        StatusState::Unavailable
    } else if !service.installed {
        StatusState::Unconfigured
    } else if !configuration_valid || service.program_matches_current == Some(false) {
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
    } else if state == StatusState::Unconfigured {
        guidance.push("Install it with `aven daemon install`.".to_string());
    } else {
        if !configuration_valid {
            guidance.push(
                "Enable sync or reminders with `aven config set sync.enabled true` or `aven config set reminders.enabled true`, then run `aven daemon repair`."
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
        state,
        platform_supported: service.platform_supported,
        platform: std::env::consts::OS.to_string(),
        installed: service.installed,
        loaded: service.loaded,
        running: service.running,
        executable_matches: service.program_matches_current,
        configuration_valid,
        sync_enabled: config.automatic_sync_is_enabled(),
        wake_address_valid,
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

impl DaemonStatusReport {
    pub(crate) fn with_sync(mut self, sync: aven_core::local_state::DaemonSyncState) -> Self {
        if self.sync_enabled
            && daemon_sync_blocked(&sync, chrono::Utc::now().timestamp())
            && self.state == StatusState::Healthy
        {
            self.state = StatusState::Blocked;
        }
        self.sync = sync;
        self
    }
}

/// Key storage failures need user action immediately. Other errors must persist
/// for two minutes before being called blocked, so short network retries do not
/// replace the normal pending headline.
pub(crate) fn daemon_sync_blocked(
    state: &aven_core::local_state::DaemonSyncState,
    now: i64,
) -> bool {
    state.error.as_ref().is_some_and(|error| {
        error.code.starts_with("protected-key-storage-") || now.saturating_sub(error.since) >= 120
    })
}

pub(crate) fn daemon_time(at: i64) -> String {
    chrono::DateTime::from_timestamp(at, 0)
        .map(|time| {
            time.with_timezone(&chrono::Local)
                .format("%H:%M")
                .to_string()
        })
        .unwrap_or_else(|| at.to_string())
}

pub(crate) fn elapsed_text(seconds: i64) -> String {
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
    fn daemon_states_cover_stopped_and_invalid_configuration() {
        let mut stopped = service(true);
        stopped.running = Some(false);
        assert_eq!(
            build_daemon_status(&configured(), stopped).state,
            StatusState::Failed
        );
        assert_eq!(
            build_daemon_status(&AppConfig::default(), service(true)).state,
            StatusState::Blocked
        );
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
}
