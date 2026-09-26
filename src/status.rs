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
    let configuration_valid = config.automatic_sync_is_enabled() && wake_address_valid;
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
                "Enable sync with `aven config set sync.enabled true`, then run `aven daemon repair`."
                    .to_string(),
            );
        }
        if service.program_matches_current == Some(false) {
            guidance.push("Repair the executable path with `aven daemon repair`.".to_string());
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
}
