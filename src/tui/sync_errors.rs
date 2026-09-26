//! TUI adapter for the shared plain-language sync error explanations.
use crate::sync::error_explanations::{self, ErrorAction, ErrorSurface};
use crate::tui::sync_operations::{FailureSignal, OperationFailure, OperationKind};

pub(super) fn failure(kind: OperationKind, error: &anyhow::Error) -> OperationFailure {
    let message = error_explanations::explain(action(kind), ErrorSurface::Tui, error)
        .map(|explanation| explanation.combined())
        .unwrap_or_else(|| fallback(kind).to_string());
    OperationFailure {
        kind,
        message,
        details: format!("{error:#}"),
        signal: signal(error),
    }
}

fn signal(error: &anyhow::Error) -> Option<FailureSignal> {
    let has = |code| error_explanations::has_code(error, code);
    if has("sync-join-timeout") {
        Some(FailureSignal::JoinTimedOut)
    } else if has("management-unfinished") {
        Some(FailureSignal::RemovalUnfinished)
    } else if has("sync-setup-storage-already-claimed") {
        Some(FailureSignal::SetupStorageClaimed)
    } else if has("sync-setup-invitation-rejected") || has("sync-setup-invitation-expired") {
        Some(FailureSignal::SetupInvitationRefused)
    } else {
        None
    }
}

fn action(kind: OperationKind) -> ErrorAction {
    match kind {
        OperationKind::Setup => ErrorAction::Setup,
        OperationKind::Join => ErrorAction::Join,
        OperationKind::Sync
        | OperationKind::ListDevices
        | OperationKind::RemoveDevice(_)
        | OperationKind::FinishRemoval => ErrorAction::General,
    }
}

pub(crate) const JOIN_TIMEOUT_EXPIRED: &str = "If the invitation expired, choose Use a new \
     invitation and paste a new one from the same device. The earlier invitation still \
     counts if the other device already added this device with it.";

pub(crate) const CHANGE_LIMIT: &str = "This sync has reached its limit on device changes. \
     Start a new sync to keep changing devices; see Recover from device loss in the sync docs.";

pub(crate) fn reached_change_limit(error: &anyhow::Error) -> bool {
    error_explanations::has_code(error, "sync-device-change-limit")
        || error_explanations::has_code(error, "membership-change-limit")
}

fn fallback(kind: OperationKind) -> &'static str {
    match kind {
        OperationKind::Sync => "Sync couldn't finish. Try again.",
        OperationKind::Setup => {
            "Setup couldn't finish. Resuming continues the same setup from where it stopped."
        }
        OperationKind::Join => {
            "Joining couldn't finish. Resuming continues the same join from where it stopped."
        }
        OperationKind::ListDevices => "Couldn't check devices with the server.",
        OperationKind::RemoveDevice(_) => {
            "Removal didn't finish. Resuming continues the same removal."
        }
        OperationKind::FinishRemoval => {
            "Securing future changes didn't finish. Try again, or sync on any remaining device."
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;

    #[test]
    fn nonempty_targets_explain_that_nothing_changed() {
        let error = anyhow!("error shared-state-install target database is not empty")
            .context("error sync-join-requires-empty-database hint=\"join with a new database\"");
        let message = failure(OperationKind::Join, &error).message;
        assert!(
            message.contains("`aven --db /new/path sync join`"),
            "{message}"
        );
    }

    #[test]
    fn join_timeouts_keep_conditional_expiry_guidance() {
        let error = anyhow!("error sync-join-timeout hint=\"x\"");
        let failure = failure(OperationKind::Join, &error);
        assert!(
            failure.message.contains("Resume joining"),
            "{}",
            failure.message
        );
        assert!(failure.join_timed_out());
        assert!(JOIN_TIMEOUT_EXPIRED.starts_with("If the invitation expired"));
    }

    #[test]
    fn device_change_limit_points_to_starting_a_new_sync() {
        let error = anyhow!("error membership-change-limit")
            .context("error sync-device-change-limit hint=\"x\"");
        assert_eq!(failure(OperationKind::Sync, &error).message, CHANGE_LIMIT);
    }

    #[test]
    fn unknown_errors_offer_resuming_the_same_attempt() {
        let error = anyhow!("error snapshot-receipt-mismatch");
        let failure = failure(OperationKind::Setup, &error);
        assert!(failure.message.contains("same setup"));
        assert_eq!(failure.details, "error snapshot-receipt-mismatch");
    }
}
