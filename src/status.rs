use std::path::{Path, PathBuf};

use anyhow::Result;
use aven_core::db::Database;
use serde::Serialize;

use crate::config::{self, AppConfig};
use crate::daemon::ServiceStatus;
use crate::sync::sync_server_url_is_valid;

/// Stable top-level condition used by CLI status reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StatusState {
    Unavailable,
    Disabled,
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
            Self::Disabled => "disabled",
            Self::Unconfigured => "unconfigured",
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Blocked => "blocked",
            Self::Failed => "failed",
        }
    }
}

/// Versioned, presentation-independent report for `aven sync status --json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct SyncStatusReport {
    /// Schema version for automation consumers.
    pub(crate) version: u32,
    pub(crate) state: StatusState,
    pub(crate) enabled: bool,
    pub(crate) runtime_allowed: bool,
    pub(crate) configured: bool,
    /// Effective server after CLI environment and configuration resolution.
    pub(crate) effective_server: Option<String>,
    /// Server identity pinned in this database by its first sync.
    pub(crate) pinned_server: Option<String>,
    pub(crate) server_matches_pin: Option<bool>,
    pub(crate) auth_configured: bool,
    pub(crate) pending: SyncPendingWork,
    pub(crate) unresolved_conflicts: i64,
    pub(crate) progress: SyncProgress,
    pub(crate) last: SyncLastRun,
    pub(crate) guidance: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct SyncPendingWork {
    pub(crate) changes: i64,
    pub(crate) attachment_uploads: i64,
    pub(crate) attachment_upload_bytes: i64,
    pub(crate) attachment_downloads: u64,
    pub(crate) attachment_download_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct SyncProgress {
    pub(crate) cursor: Option<i64>,
    pub(crate) local_sequence: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub(crate) struct SyncLastRun {
    pub(crate) attempt_at: Option<String>,
    pub(crate) success_at: Option<String>,
    /// Privacy-safe failure category. Detailed transport and server text is withheld.
    pub(crate) safe_error: Option<String>,
    pub(crate) pushed: Option<i64>,
    pub(crate) pulled: Option<i64>,
    pub(crate) cursor: Option<i64>,
}

pub(crate) async fn build_sync_status(
    database: &Database,
    config: &AppConfig,
    db_path: &Path,
) -> Result<SyncStatusReport> {
    let persistence = database.sync_persistence_status().await?;
    let blob_dir = config::resolve_blob_dir(db_path, config)?;
    let missing = database.missing_sync_attachment_counts(&blob_dir).await?;
    let effective_server_value = config::resolve_sync_server(None, config)
        .ok()
        .filter(|server| sync_server_url_is_valid(server));
    let server_matches_pin = match (
        effective_server_value.as_deref(),
        persistence.pinned_server.as_deref(),
    ) {
        (Some(effective), Some(pinned)) => {
            Some(effective.trim_end_matches('/') == pinned.trim_end_matches('/'))
        }
        _ => None,
    };
    let effective_server = effective_server_value.as_deref().map(safe_server_identity);
    let pinned_server = persistence
        .pinned_server
        .as_deref()
        .map(safe_server_identity);
    let configured = effective_server.is_some();
    let safe_error = safe_sync_error(persistence.last_error.as_deref());
    let failure_is_current = safe_error.is_some()
        && persistence.last_attempt.is_some()
        && persistence.last_attempt > persistence.last_success;
    let pending = SyncPendingWork {
        changes: persistence.pending_changes,
        attachment_uploads: persistence.pending_attachment_uploads,
        attachment_upload_bytes: persistence.pending_attachment_upload_bytes,
        attachment_downloads: missing.count,
        attachment_download_bytes: missing.bytes,
    };
    let state = classify_sync_state(SyncStateInput {
        enabled: config.sync.enabled,
        runtime_allowed: config.sync_is_allowed(),
        configured,
        server_mismatch: server_matches_pin == Some(false),
        conflicts: persistence.conflicts,
        current_failure: failure_is_current,
        pending: pending.changes > 0
            || pending.attachment_uploads > 0
            || pending.attachment_downloads > 0,
        ever_succeeded: persistence.last_success.is_some(),
    });
    let guidance = sync_guidance(state, persistence.conflicts);

    Ok(SyncStatusReport {
        version: 1,
        state,
        enabled: config.sync.enabled,
        runtime_allowed: config.sync_is_allowed(),
        configured,
        effective_server,
        pinned_server,
        server_matches_pin,
        auth_configured: config.sync_auth_token().is_some(),
        pending,
        unresolved_conflicts: persistence.conflicts,
        progress: SyncProgress {
            cursor: parse_counter(persistence.sync_cursor.as_deref()),
            local_sequence: parse_counter(persistence.local_sequence.as_deref()),
        },
        last: SyncLastRun {
            attempt_at: persistence.last_attempt,
            success_at: persistence.last_success,
            safe_error,
            pushed: parse_counter(persistence.last_pushed.as_deref()),
            pulled: parse_counter(persistence.last_pulled.as_deref()),
            cursor: parse_counter(persistence.last_cursor.as_deref()),
        },
        guidance,
    })
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct SyncStateInput {
    pub(crate) enabled: bool,
    pub(crate) runtime_allowed: bool,
    pub(crate) configured: bool,
    pub(crate) server_mismatch: bool,
    pub(crate) conflicts: i64,
    pub(crate) current_failure: bool,
    pub(crate) pending: bool,
    pub(crate) ever_succeeded: bool,
}

pub(crate) fn classify_sync_state(input: SyncStateInput) -> StatusState {
    if !input.runtime_allowed || !input.enabled {
        StatusState::Disabled
    } else if !input.configured {
        StatusState::Unconfigured
    } else if input.server_mismatch || input.conflicts > 0 {
        StatusState::Blocked
    } else if input.current_failure {
        StatusState::Failed
    } else if input.pending || !input.ever_succeeded {
        StatusState::Degraded
    } else {
        StatusState::Healthy
    }
}

fn parse_counter(value: Option<&str>) -> Option<i64> {
    value?.parse().ok()
}

fn sync_guidance(state: StatusState, conflicts: i64) -> Vec<String> {
    let mut guidance = Vec::new();
    match state {
        StatusState::Disabled => guidance.push(
            "Enable sync with `aven config set sync.enabled true` when this device should sync."
                .to_string(),
        ),
        StatusState::Unconfigured => {
            guidance.push("Set a server with `aven config set sync.server_url <url>`.".to_string())
        }
        StatusState::Failed => guidance.push(
            "Run `aven sync` for detailed diagnostics after checking the server and network."
                .to_string(),
        ),
        StatusState::Degraded => {
            guidance.push("Run `aven sync` or verify that the daemon is healthy.".to_string())
        }
        StatusState::Blocked if conflicts > 0 => {
            guidance.push("Inspect unresolved conflicts with `aven conflict list`.".to_string())
        }
        StatusState::Blocked => guidance.push(
            "Use a fresh database for a different sync server, or restore the configured server."
                .to_string(),
        ),
        StatusState::Unavailable | StatusState::Healthy => {}
    }
    guidance
}

fn safe_server_identity(value: &str) -> String {
    let Ok(mut url) = url::Url::parse(value.trim_end_matches('/')) else {
        return "invalid server URL".to_string();
    };
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_path("");
    url.set_query(None);
    url.set_fragment(None);
    url.to_string().trim_end_matches('/').to_string()
}

fn safe_sync_error(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    Some(
        match value {
            "invalid sync server URL"
            | "sync request preparation failed"
            | "sync transport failed"
            | "sync response rejected" => value,
            _ => "sync failed (details withheld)",
        }
        .to_string(),
    )
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
    pub(crate) server_configured: bool,
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
    let server_configured = config
        .sync
        .server_url
        .as_deref()
        .is_some_and(sync_server_url_is_valid);
    let wake_address_valid = config.wake_addr().is_ok();
    let configuration_valid =
        config.automatic_sync_is_enabled() && server_configured && wake_address_valid;
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
        guidance.push("The managed daemon is supported on macOS.".to_string());
    } else if state == StatusState::Unconfigured {
        guidance.push("Install it with `aven daemon install`.".to_string());
    } else {
        if !configuration_valid {
            guidance.push(
                "Enable sync, configure sync.server_url, then run `aven daemon repair`."
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
            guidance.push("Run `aven doctor` for additional launchd diagnostics.".to_string());
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
        server_configured,
        wake_address_valid,
        paths: DaemonPaths {
            service: nonempty_path(service.plist_path),
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
            plist_path: PathBuf::from("/service.plist"),
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
        config.sync.server_url = Some("https://sync.example.test".to_string());
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

    #[test]
    fn server_identity_and_errors_withhold_secrets() {
        assert_eq!(
            safe_server_identity("https://user:secret@example.test/sync?token=hidden"),
            "https://example.test"
        );
        assert_eq!(
            safe_sync_error(Some("server said task=secret token=hidden")).as_deref(),
            Some("sync failed (details withheld)")
        );
    }
}
