use ratatui::style::Color;

use crate::status::{StatusState, SyncStateInput, classify_sync_state};
use crate::tui::store::TuiSyncStatus;
use crate::tui::theme::{FG_DIM, GREEN, ORANGE, RED};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SyncHealth {
    Error,
    Attention,
    RuntimeDisabled,
    LocalOnly,
    Pending(i64),
    Synced,
    NeverSynced,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SyncIssue {
    pub(super) label: &'static str,
    pub(super) value: String,
    pub(super) error: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SyncStatusSummary {
    pub(super) health: SyncHealth,
    pub(super) server: Option<String>,
    pub(super) issues: Vec<SyncIssue>,
    pub(super) can_manual_sync: bool,
}

impl SyncStatusSummary {
    pub(super) fn headline(&self) -> &'static str {
        match self.health {
            SyncHealth::Error => "Sync needs attention",
            SyncHealth::Attention => "Sync needs attention",
            SyncHealth::RuntimeDisabled => "Sync disabled",
            SyncHealth::LocalOnly => "Local only",
            SyncHealth::Pending(_) => "Changes waiting",
            SyncHealth::Synced => "Up to date",
            SyncHealth::NeverSynced => "Ready to sync",
        }
    }

    pub(super) fn color(&self) -> Color {
        match self.health {
            SyncHealth::Error => RED,
            SyncHealth::Attention | SyncHealth::Pending(_) => ORANGE,
            SyncHealth::Synced => GREEN,
            SyncHealth::RuntimeDisabled | SyncHealth::LocalOnly | SyncHealth::NeverSynced => FG_DIM,
        }
    }

    pub(super) fn badge(&self) -> (Color, String) {
        match self.health {
            SyncHealth::Error => (RED, "sync!".to_string()),
            SyncHealth::Attention => (ORANGE, "sync!".to_string()),
            SyncHealth::RuntimeDisabled => (FG_DIM, "sync off".to_string()),
            SyncHealth::LocalOnly => (FG_DIM, "local".to_string()),
            SyncHealth::Pending(count) => (ORANGE, format!("sync {count}")),
            SyncHealth::Synced => (GREEN, "sync".to_string()),
            SyncHealth::NeverSynced => (GREEN, "sync".to_string()),
        }
    }
}

pub(super) fn sync_status_summary(status: &TuiSyncStatus) -> SyncStatusSummary {
    let mut issues = Vec::new();
    if let Some(error) = &status.config_error {
        issues.push(issue("configuration", error, true));
    }
    if status.enabled
        && let Some(check) = status.configured_server.as_ref().filter(|check| !check.ok)
    {
        issues.push(issue("server", &check.value, true));
    }
    if status.enabled
        && status.runtime_allowed
        && let Some(check) = status.server_match.as_ref().filter(|check| !check.ok)
    {
        issues.push(issue("server mismatch", &check.value, true));
    }
    if let Some(error) = status.last_error_value() {
        issues.push(issue("last error", error, true));
    }
    if status.enabled
        && let Some(check) = status.daemon_server.as_ref().filter(|check| !check.ok)
    {
        issues.push(issue("daemon server", &check.value, false));
    }
    if status.enabled && !status.daemon_wake.ok {
        issues.push(issue("wake address", &status.daemon_wake.value, false));
    }

    let configured = status.config_error.is_none()
        && status
            .configured_server
            .as_ref()
            .is_some_and(|check| check.ok);
    let state = classify_sync_state(SyncStateInput {
        enabled: status.enabled,
        runtime_allowed: status.runtime_allowed,
        configured,
        server_mismatch: status.server_match.as_ref().is_some_and(|check| !check.ok),
        conflicts: status.conflicts,
        current_failure: status.last_error_value().is_some(),
        pending: status.pending_changes > 0,
        ever_succeeded: status.last_success.is_some(),
    });
    let health = if issues.iter().any(|issue| issue.error) {
        SyncHealth::Error
    } else if status.conflicts > 0 || !issues.is_empty() {
        SyncHealth::Attention
    } else {
        match state {
            StatusState::Disabled if !status.runtime_allowed => SyncHealth::RuntimeDisabled,
            StatusState::Disabled => SyncHealth::LocalOnly,
            StatusState::Unconfigured | StatusState::Failed | StatusState::Unavailable => {
                SyncHealth::Error
            }
            StatusState::Blocked => SyncHealth::Attention,
            StatusState::Degraded if status.pending_changes > 0 => {
                SyncHealth::Pending(status.pending_changes)
            }
            StatusState::Degraded => SyncHealth::NeverSynced,
            StatusState::Healthy => SyncHealth::Synced,
        }
    };

    let server = status
        .configured_server
        .as_ref()
        .filter(|check| check.ok)
        .map(|check| check.value.clone());
    let can_manual_sync = status.runtime_allowed
        && status.config_error.is_none()
        && server.is_some()
        && status.server_match.as_ref().is_none_or(|check| check.ok);

    SyncStatusSummary {
        health,
        server,
        issues,
        can_manual_sync,
    }
}

fn issue(label: &'static str, value: &str, error: bool) -> SyncIssue {
    SyncIssue {
        label,
        value: value.to_string(),
        error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::store::SyncStatusCheck;

    #[test]
    fn blocking_errors_take_precedence_over_other_states() {
        let status = TuiSyncStatus {
            enabled: true,
            pending_changes: 3,
            conflicts: 2,
            last_error: Some("connection refused".to_string()),
            ..TuiSyncStatus::default()
        };

        let summary = sync_status_summary(&status);

        assert_eq!(summary.health, SyncHealth::Error);
        assert_eq!(summary.badge(), (RED, "sync!".to_string()));
    }

    #[test]
    fn conflicts_are_attention_without_becoming_transport_errors() {
        let status = TuiSyncStatus {
            enabled: true,
            conflicts: 2,
            ..TuiSyncStatus::default()
        };

        let summary = sync_status_summary(&status);

        assert_eq!(summary.health, SyncHealth::Attention);
        assert_eq!(summary.badge(), (ORANGE, "sync!".to_string()));
    }

    #[test]
    fn daemon_failures_are_visible_attention() {
        let status = TuiSyncStatus {
            enabled: true,
            daemon_server: Some(SyncStatusCheck::new(false, "not configured")),
            ..TuiSyncStatus::default()
        };

        let summary = sync_status_summary(&status);

        assert_eq!(summary.health, SyncHealth::Attention);
        assert!(summary.issues.iter().any(|issue| !issue.error));
    }

    #[test]
    fn disabled_sync_ignores_inactive_server_mismatches() {
        let status = TuiSyncStatus {
            server_match: Some(SyncStatusCheck::new(false, "servers differ")),
            ..TuiSyncStatus::default()
        };

        let summary = sync_status_summary(&status);

        assert_eq!(summary.health, SyncHealth::LocalOnly);
        assert!(!summary.can_manual_sync);
    }

    #[test]
    fn local_and_runtime_disabled_states_stay_distinct() {
        let local = sync_status_summary(&TuiSyncStatus::default());
        let disabled = sync_status_summary(&TuiSyncStatus {
            enabled: true,
            runtime_allowed: false,
            ..TuiSyncStatus::default()
        });

        assert_eq!(local.health, SyncHealth::LocalOnly);
        assert_eq!(disabled.health, SyncHealth::RuntimeDisabled);
    }
}
