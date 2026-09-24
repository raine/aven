//! Plain-language explanations of sync failures. The main message says what
//! happened and what can be done; the engine's error chain stays available as
//! details. Refusals and unknown outcomes are never reported as proof of a
//! cause the engine did not establish.
use crate::tui::sync_operations::{OperationFailure, OperationKind};

pub(super) fn failure(kind: OperationKind, error: &anyhow::Error) -> OperationFailure {
    OperationFailure {
        kind,
        message: explain(kind, error).to_string(),
        details: format!("{error:#}"),
    }
}

/// The engine's error codes in the chain, outermost first.
fn codes(error: &anyhow::Error) -> impl Iterator<Item = String> + '_ {
    error.chain().filter_map(|cause| {
        cause
            .to_string()
            .strip_prefix("error ")
            .and_then(|rest| rest.split_whitespace().next())
            .map(str::to_string)
    })
}

pub(crate) const JOIN_REQUIRES_EMPTY: &str = "This computer already has tasks or other data. \
     Joining needs an empty database, because existing local data can't be merged with \
     synced data yet. Nothing here was changed.";

fn explain(kind: OperationKind, error: &anyhow::Error) -> &'static str {
    let codes = codes(error).collect::<Vec<_>>();
    let has = |code: &str| codes.iter().any(|candidate| candidate == code);
    if codes.iter().any(|code| code.ends_with("-network")) {
        return "Couldn't reach the sync server. Check the connection and try again. \
                Local work continues.";
    }
    if has("sync-disabled") {
        return "Sync is disabled in this environment.";
    }
    if has("sync-setup-invitation-mismatch") {
        return "This invitation belongs to a different setup. Paste the invitation that \
                started setup.";
    }
    if has("sync-setup-refused") {
        return "The server refused this setup invitation. If the server's setup was run \
                again or the invitation is over an hour old, use the newest one; otherwise \
                check the server and try again.";
    }
    if has("sync-already-set-up") {
        return "This database already takes part in sync.";
    }
    if has("sync-join-requires-empty-database") {
        return JOIN_REQUIRES_EMPTY;
    }
    if has("shared-state-install") || has("snapshot-target-not-fresh") {
        return "Data was added to this database while joining, so the synced tasks can't \
                be installed here. Join from a new, empty database instead.";
    }
    if has("sync-join-timeout") {
        return "The other device didn't add this device in time. Keep Add device open on \
                the other device, then resume joining.";
    }
    if has("sync-join-server-mismatch") || has("enrollment-invitation-conflict") {
        return "This invitation doesn't match the join this database already started. \
                Resume joining without a new invitation.";
    }
    if has("enrollment-busy") {
        return "The server is busy with another device. Try again in a moment.";
    }
    if has("enrollment-revoked") || has("sync-device-removed") {
        return "Another device removed this device from sync. Tasks and images here stay \
                available, but this device can no longer sync.";
    }
    if has("sync-device-removal-unfinished") || has("management-unfinished") {
        return "An earlier device removal from this device is unfinished. Finish it \
                before removing another device.";
    }
    if has("sync-device-not-found") {
        return "That device is no longer in sync. Refresh the list.";
    }
    if has("sync-device-current") {
        return "This is the current device. Remove it from another device in sync.";
    }
    // A refusal alone proves neither a server failure nor removal of this
    // device, so the message names only what could not be confirmed.
    if has("sync-server-refused") || has("enrollment-refused") || has("bootstrap-refused") {
        return match kind {
            OperationKind::Join => {
                "The server refused the request. The invitation may have expired or \
                 already been used."
            }
            OperationKind::Setup => {
                "The server refused the request, so setup couldn't be confirmed. Try \
                 again later, or check the server."
            }
            OperationKind::ListDevices => {
                "The server refused the request, so this device's access couldn't be \
                 confirmed. Try again later."
            }
            OperationKind::RemoveDevice(_) | OperationKind::FinishRemoval => {
                "The server refused the request, so the removal couldn't be confirmed. \
                 Resuming retries the same removal."
            }
        };
    }
    match kind {
        OperationKind::Setup => {
            "Setup couldn't finish. Resuming continues the same setup from where it \
             stopped."
        }
        OperationKind::Join => {
            "Joining couldn't finish. Resuming continues the same join from where it \
             stopped."
        }
        OperationKind::ListDevices => "Couldn't check devices with the server.",
        OperationKind::RemoveDevice(_) => {
            "Removal didn't finish. Resuming continues the same removal."
        }
        OperationKind::FinishRemoval => {
            "Securing future changes didn't finish. Try again, or sync on any remaining \
             device."
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
    fn refusals_do_not_claim_a_cause() {
        let error = anyhow!("error enrollment-refused outcome-unknown");
        let message = failure(OperationKind::Setup, &error).message;
        assert!(message.contains("couldn't be confirmed"), "{message}");
        assert!(!message.contains("removed"), "{message}");
    }

    #[test]
    fn removal_refusals_never_claim_the_device_was_removed() {
        for kind in [
            OperationKind::ListDevices,
            OperationKind::RemoveDevice([1; 32]),
            OperationKind::FinishRemoval,
        ] {
            let error = anyhow!("error enrollment-refused outcome-unknown")
                .context("error sync-device-removal-incomplete hint=\"rerun the same command\"");
            let message = failure(kind, &error).message;
            assert!(message.contains("couldn't be confirmed"), "{message}");
            assert!(!message.contains("removed"), "{message}");
        }
        let unfinished = anyhow!("error management-unfinished");
        assert!(
            failure(OperationKind::RemoveDevice([1; 32]), &unfinished)
                .message
                .contains("earlier device removal")
        );
    }

    #[test]
    fn unknown_errors_offer_resuming_the_same_attempt() {
        let error = anyhow!("error snapshot-receipt-mismatch");
        let failure = failure(OperationKind::Setup, &error);
        assert!(failure.message.contains("same setup"));
        assert_eq!(failure.details, "error snapshot-receipt-mismatch");
    }
}
