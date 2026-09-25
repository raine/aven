use ratatui::style::Color;

use crate::sync::encrypted::LocalPhase;
use crate::tui::store::TuiSyncStatus;
use crate::tui::theme::{FG_DIM, GREEN, ORANGE, RED};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SyncHealth {
    AccessRefused,
    Attention,
    RuntimeDisabled,
    NotSetUp,
    /// Setup or joining started here and has not finished.
    Unfinished,
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
            SyncHealth::AccessRefused => "Sync access unconfirmed",
            SyncHealth::Attention => "Sync needs attention",
            SyncHealth::RuntimeDisabled => "Sync disabled",
            SyncHealth::NotSetUp => "Local only",
            SyncHealth::Unfinished => "Sync is not ready yet",
            SyncHealth::Pending(_) => "Changes waiting",
            SyncHealth::Idle => "No changes waiting",
        }
    }

    pub(super) fn color(&self) -> Color {
        match self.health {
            SyncHealth::AccessRefused => RED,
            SyncHealth::Attention | SyncHealth::Unfinished | SyncHealth::Pending(_) => ORANGE,
            SyncHealth::Idle => GREEN,
            SyncHealth::RuntimeDisabled | SyncHealth::NotSetUp => FG_DIM,
        }
    }

    pub(super) fn badge(&self, status: &TuiSyncStatus) -> (Color, String) {
        if let Some(invitation) = status.invitation {
            let remaining = invitation
                .expires_at
                .saturating_sub(crate::sync::encrypted::unix_now().unwrap_or_default());
            if remaining > 0 {
                return (
                    ORANGE,
                    format!("inviting · {}:{:02}", remaining / 60, remaining % 60),
                );
            }
        }
        match self.health {
            SyncHealth::AccessRefused => (RED, "sync error".to_string()),
            SyncHealth::Attention | SyncHealth::Unfinished => (ORANGE, "sync!".to_string()),
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
            label: "Wake address",
            value: status.daemon_wake.value.clone(),
        });
    }
    let health = if !status.set_up {
        SyncHealth::NotSetUp
    } else if status.access_refused_at.is_some() {
        SyncHealth::AccessRefused
    } else if !status.runtime_allowed {
        SyncHealth::RuntimeDisabled
    } else if status.phase != LocalPhase::SetUp {
        SyncHealth::Unfinished
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
        can_manual_sync: status.phase == LocalPhase::SetUp && status.runtime_allowed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::store::SyncStatusCheck;

    fn set_up() -> TuiSyncStatus {
        TuiSyncStatus {
            set_up: true,
            phase: LocalPhase::SetUp,
            ..TuiSyncStatus::default()
        }
    }

    #[test]
    fn unfinished_setup_or_joining_is_not_reported_as_idle() {
        for phase in [LocalPhase::SetupIncomplete, LocalPhase::JoinIncomplete] {
            let status = TuiSyncStatus { phase, ..set_up() };
            let summary = sync_status_summary(&status);
            assert_eq!(summary.health, SyncHealth::Unfinished);
            assert_eq!(summary.badge(&status), (ORANGE, "sync!".to_string()));
            assert!(!summary.can_manual_sync);
        }
    }

    #[test]
    fn open_invitation_has_a_countdown_badge() {
        let status = TuiSyncStatus {
            invitation: Some(crate::sync::encrypted::InvitationStatus {
                expires_at: crate::sync::encrypted::unix_now().unwrap() + 462,
                keys_may_have_been_sent: false,
            }),
            ..set_up()
        };

        let badge = sync_status_summary(&status).badge(&status);

        assert_eq!(badge.0, ORANGE);
        assert!(badge.1.starts_with("inviting · 7:"), "{}", badge.1);
    }

    #[test]
    fn access_refusal_is_a_red_error_that_can_be_retried() {
        let status = TuiSyncStatus {
            access_refused_at: Some("2026-09-24T12:00:00Z".to_string()),
            ..set_up()
        };

        let summary = sync_status_summary(&status);

        assert_eq!(summary.health, SyncHealth::AccessRefused);
        assert_eq!(summary.badge(&status), (RED, "sync error".to_string()));
        assert!(summary.can_manual_sync);
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
        assert_eq!(summary.badge(&status), (ORANGE, "sync!".to_string()));
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
        assert_eq!(
            pending.badge(&TuiSyncStatus {
                pending_changes: 4,
                ..set_up()
            }),
            (ORANGE, "sync 4".to_string())
        );
        assert!(pending.can_manual_sync);
    }
}
