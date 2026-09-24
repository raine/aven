use ratatui::style::Color;

use crate::tui::store::TuiSyncStatus;
use crate::tui::theme::{FG_DIM, GREEN, ORANGE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SyncHealth {
    Attention,
    RuntimeDisabled,
    NotSetUp,
    Pending(i64),
    Idle,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SyncIssue {
    pub(super) label: &'static str,
    pub(super) value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SyncStatusSummary {
    pub(super) health: SyncHealth,
    pub(super) issues: Vec<SyncIssue>,
    pub(super) can_manual_sync: bool,
}

impl SyncStatusSummary {
    pub(super) fn headline(&self) -> &'static str {
        match self.health {
            SyncHealth::Attention => "Sync needs attention",
            SyncHealth::RuntimeDisabled => "Sync disabled",
            SyncHealth::NotSetUp => "Local only",
            SyncHealth::Pending(_) => "Changes waiting",
            SyncHealth::Idle => "No changes waiting",
        }
    }

    pub(super) fn color(&self) -> Color {
        match self.health {
            SyncHealth::Attention | SyncHealth::Pending(_) => ORANGE,
            SyncHealth::Idle => GREEN,
            SyncHealth::RuntimeDisabled | SyncHealth::NotSetUp => FG_DIM,
        }
    }

    pub(super) fn badge(&self) -> (Color, String) {
        match self.health {
            SyncHealth::Attention => (ORANGE, "sync!".to_string()),
            SyncHealth::RuntimeDisabled => (FG_DIM, "sync off".to_string()),
            SyncHealth::NotSetUp => (FG_DIM, "local".to_string()),
            SyncHealth::Pending(count) => (ORANGE, format!("sync {count}")),
            SyncHealth::Idle => (GREEN, "sync".to_string()),
        }
    }
}

pub(super) fn sync_status_summary(status: &TuiSyncStatus) -> SyncStatusSummary {
    let mut issues = Vec::new();
    if status.set_up && status.enabled && !status.daemon_wake.ok {
        issues.push(SyncIssue {
            label: "wake address",
            value: status.daemon_wake.value.clone(),
        });
    }
    let health = if !status.set_up {
        SyncHealth::NotSetUp
    } else if !status.runtime_allowed {
        SyncHealth::RuntimeDisabled
    } else if status.conflicts > 0 || !issues.is_empty() {
        SyncHealth::Attention
    } else if status.pending_changes > 0 {
        SyncHealth::Pending(status.pending_changes)
    } else {
        SyncHealth::Idle
    };
    SyncStatusSummary {
        health,
        issues,
        can_manual_sync: status.set_up && status.runtime_allowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::store::SyncStatusCheck;

    fn set_up() -> TuiSyncStatus {
        TuiSyncStatus {
            set_up: true,
            ..TuiSyncStatus::default()
        }
    }

    #[test]
    fn conflicts_take_precedence_over_pending_changes() {
        let status = TuiSyncStatus {
            pending_changes: 3,
            conflicts: 2,
            ..set_up()
        };

        let summary = sync_status_summary(&status);

        assert_eq!(summary.health, SyncHealth::Attention);
        assert_eq!(summary.badge(), (ORANGE, "sync!".to_string()));
    }

    #[test]
    fn automatic_sync_wake_failures_are_visible_attention() {
        let status = TuiSyncStatus {
            enabled: true,
            daemon_wake: SyncStatusCheck::new(false, "invalid"),
            ..set_up()
        };

        let summary = sync_status_summary(&status);

        assert_eq!(summary.health, SyncHealth::Attention);
        assert_eq!(summary.issues.len(), 1);
    }

    #[test]
    fn unset_up_runtime_disabled_and_pending_states_stay_distinct() {
        let local = sync_status_summary(&TuiSyncStatus::default());
        let disabled = sync_status_summary(&TuiSyncStatus {
            runtime_allowed: false,
            ..set_up()
        });
        let pending = sync_status_summary(&TuiSyncStatus {
            pending_changes: 4,
            ..set_up()
        });

        assert_eq!(local.health, SyncHealth::NotSetUp);
        assert!(!local.can_manual_sync);
        assert_eq!(disabled.health, SyncHealth::RuntimeDisabled);
        assert!(!disabled.can_manual_sync);
        assert_eq!(pending.badge(), (ORANGE, "sync 4".to_string()));
        assert!(pending.can_manual_sync);
    }
}
