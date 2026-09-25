//! TUI adapter for the shared plain-language sync error explanations.
use crate::sync::error_explanations::{self, ErrorAction, ErrorSurface};
use crate::tui::sync_operations::{OperationFailure, OperationKind};

pub(super) fn failure(kind: OperationKind, error: &anyhow::Error) -> OperationFailure {
    let message = error_explanations::explain(action(kind), ErrorSurface::Tui, error)
        .map(|explanation| explanation.combined())
        .unwrap_or_else(|| fallback(kind).to_string());
    OperationFailure {
        kind,
        message,
        details: format!("{error:#}"),
    }
}

fn action(kind: OperationKind) -> ErrorAction {
    match kind {
        OperationKind::Sync => ErrorAction::Sync,
        OperationKind::Setup => ErrorAction::Setup,
        OperationKind::Join => ErrorAction::Join,
        OperationKind::ListDevices => ErrorAction::ListDevices,
        OperationKind::RemoveDevice(_) => ErrorAction::RemoveDevice,
        OperationKind::FinishRemoval => ErrorAction::FinishRemoval,
    }
}

/// A timeout does not show whether the invitation expired, and an admission
/// committed before expiry can still finish.
#[cfg(test)]
pub(crate) const JOIN_TIMEOUT: &str = "The other device didn't add this device in time. \
     Keep Add device open on the other device, then choose Resume joining.";

pub(crate) const JOIN_TIMEOUT_EXPIRED: &str = "If the invitation expired, choose Use a new \
     invitation and paste a new one from the same device. The earlier invitation still \
     counts if the other device already added this device with it.";

#[cfg(test)]
pub(crate) const JOIN_REQUIRES_EMPTY: &str = "This database already has tasks or other data, \
     and joining needs an empty database. Choose a new, empty database and join there. \
     Nothing here was changed.";

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
    fn network_failures_explain_connectivity_and_keep_details() {
        let error = anyhow!("error enrollment-network outcome-unknown")
            .context("error sync-join-incomplete");
        let failure = failure(OperationKind::Join, &error);
        assert!(
            failure
                .message
                .starts_with("Couldn't reach the sync server")
        );
        assert!(failure.details.contains("enrollment-network"));
    }

    #[test]
    fn nonempty_targets_explain_that_nothing_changed() {
        let error = anyhow!("error shared-state-install target database is not empty")
            .context("error sync-join-requires-empty-database hint=\"join with a new database\"");
        assert_eq!(
            failure(OperationKind::Join, &error).message,
            JOIN_REQUIRES_EMPTY
        );
    }

    #[test]
    fn access_refusals_name_removal_only_as_a_possibility() {
        let error = anyhow!("error enrollment-refused outcome-unknown")
            .context("error sync-server-refused hint=\"raw\"");
        let message = failure(OperationKind::Sync, &error).message;
        assert!(message.contains("may have been removed"), "{message}");
        assert!(!message.contains("was removed"), "{message}");
        assert!(message.contains("Local tasks and images stay here"));
    }

    #[test]
    fn join_refusal_does_not_claim_device_removal() {
        let error = anyhow!("error enrollment-refused outcome-unknown");
        let message = failure(OperationKind::Join, &error).message;
        assert!(message.contains("invitation may have expired"), "{message}");
        assert!(!message.contains("removed"), "{message}");
    }

    #[test]
    fn join_timeouts_keep_conditional_expiry_guidance() {
        let error = anyhow!("error sync-join-timeout hint=\"x\"");
        let failure = failure(OperationKind::Join, &error);
        assert_eq!(failure.message, JOIN_TIMEOUT);
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
