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

/// A timeout does not show whether the invitation expired, and an admission
/// committed before expiry can still finish, so the guidance is conditional
/// on expiry and a new invitation keeps the earlier one usable.
pub(crate) const JOIN_TIMEOUT: &str = "The other device didn't add this device in time. \
     Keep Add device open on the other device, then resume joining.";

pub(crate) const JOIN_TIMEOUT_EXPIRED: &str = "If the invitation expired, choose Use a new \
     invitation and paste a new one from the same device. The earlier invitation still \
     counts if the other device already added this device with it.";

pub(crate) const JOIN_REQUIRES_EMPTY: &str = "This computer already has tasks or other data. \
     Joining needs an empty database, because existing local data can't be merged with \
     synced data yet. Nothing here was changed.";

/// Downloads continue, so the message never says sync stopped.
const KEY_CHANGE_REQUIRED: &str = "An invitation expired after keys may have \
     been sent to a device that never joined. Changes from other devices still download; \
     changes made here upload once sync changes keys. Check the connection and sync again.";

/// The vault's lifetime budget of device changes is spent.
pub(crate) const CHANGE_LIMIT: &str = "This sync has reached its limit on device changes, so \
     devices can no longer be added or removed. To keep changing devices, start a new \
     sync; see Recover from device loss in the sync docs.";

pub(crate) fn reached_change_limit(error: &anyhow::Error) -> bool {
    codes(error).any(|code| code == "sync-device-change-limit" || code == "membership-change-limit")
}

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
    if has("sync-setup-storage-already-claimed") {
        return "This server already belongs to another sync. Nothing here was changed. \
                To use that sync, join it from an empty database.";
    }
    if has("sync-setup-invitation-rejected") {
        return "This setup invitation expired, was replaced, or is for different storage. \
                Nothing here was changed. Create a current invitation for this unclaimed \
                server and try again.";
    }
    if has("sync-setup-outcome-unknown") {
        return "The server claim couldn't be confirmed. Resume continues the same setup. \
                Local work continues.";
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
        return JOIN_TIMEOUT;
    }
    if has("sync-join-server-mismatch") {
        return "This invitation is for a different server than the join this database \
                already started. Use an invitation from the device that created the first \
                one.";
    }
    if has("sync-join-invitation-conflict") || has("enrollment-invitation-conflict") {
        return "This invitation doesn't match the join this database already started. \
                Resume joining, or choose Use a new invitation.";
    }
    if has("sync-join-new-invitation-mismatch") {
        return "A new invitation must come from the device that created the first one. \
                Open Add device on that device and copy a new invitation.";
    }
    if has("sync-join-new-invitation-unavailable") {
        return "The other device already added this device, so a new invitation isn't \
                needed. Resume joining.";
    }
    if has("sync-join-new-invitation-limit") {
        return "This database has reached its invitation limit. Resume joining to finish \
                if the other device accepted an invitation it already used; otherwise \
                keep it as it is, and join from a new, empty database.";
    }
    if has("sync-key-change-required") || has("withdrawal-rotation-required") {
        return KEY_CHANGE_REQUIRED;
    }
    if has("sync-invitation-unresolved") || has("withdrawal-required-unsupported") {
        return "The last invitation may already have sent keys to a device that hasn't \
                joined. Add a device again once it joins, or once that invitation expires \
                and the next sync changes keys.";
    }
    if reached_change_limit(error) {
        return CHANGE_LIMIT;
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
            OperationKind::ListDevices | OperationKind::Sync => {
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
        OperationKind::Sync => "Sync couldn't finish. Try again.",
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
    fn definite_setup_refusals_explain_that_local_data_was_not_changed() {
        let claimed = anyhow!("error bootstrap-storage-already-claimed")
            .context("error sync-setup-storage-already-claimed");
        let claimed_failure = failure(OperationKind::Setup, &claimed);
        assert!(
            claimed_failure
                .message
                .contains("already belongs to another sync")
        );
        assert!(claimed_failure.message.contains("Nothing here was changed"));

        let rejected = anyhow!("error bootstrap-setup-invitation-rejected")
            .context("error sync-setup-invitation-rejected");
        let rejected_failure = failure(OperationKind::Setup, &rejected);
        assert!(rejected_failure.message.contains("expired, was replaced"));
        assert!(
            rejected_failure
                .message
                .contains("Nothing here was changed")
        );
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
    fn join_timeouts_do_not_claim_expiry_or_suggest_discarding_data() {
        let error = anyhow!(
            "error sync-join-timeout hint=\"the other device did not add this device in time\""
        );
        let failure = failure(OperationKind::Join, &error);
        assert_eq!(failure.message, JOIN_TIMEOUT);
        assert!(failure.join_timed_out());
        for text in [JOIN_TIMEOUT, JOIN_TIMEOUT_EXPIRED] {
            for word in ["delete", "reset", "disposable", "is empty"] {
                assert!(!text.contains(word), "{text}");
            }
        }
        assert!(JOIN_TIMEOUT_EXPIRED.starts_with("If the invitation expired"));
        assert!(JOIN_TIMEOUT_EXPIRED.contains("Use a new invitation"));
        assert!(JOIN_TIMEOUT_EXPIRED.contains("earlier invitation still counts"));
    }

    #[test]
    fn refused_new_invitations_explain_what_still_works() {
        for (code, expected) in [
            (
                "sync-join-invitation-conflict",
                "choose Use a new invitation",
            ),
            (
                "sync-join-new-invitation-mismatch",
                "device that created the first one",
            ),
            ("sync-join-new-invitation-unavailable", "Resume joining"),
            ("sync-join-new-invitation-limit", "Resume joining to finish"),
            ("sync-join-server-mismatch", "different server"),
        ] {
            let error = anyhow!("error {code} hint=\"x\"");
            let message = failure(OperationKind::Join, &error).message;
            assert!(message.contains(expected), "{code}: {message}");
        }
    }

    #[test]
    fn expired_invitation_key_changes_do_not_claim_sync_paused() {
        let error = anyhow!("error network-down")
            .context("error withdrawal-rotation-required")
            .context("error sync-key-change-required hint=\"x\"");
        let message = failure(OperationKind::Sync, &error).message;
        assert_eq!(message, KEY_CHANGE_REQUIRED);
        let error = anyhow!("error withdrawal-required-unsupported")
            .context("error sync-invitation-unresolved hint=\"x\"");
        let message = failure(OperationKind::Sync, &error).message;
        assert!(
            message.contains("once that invitation expires"),
            "{message}"
        );
        for message in [KEY_CHANGE_REQUIRED, message.as_str()] {
            assert!(!message.contains("paused"), "{message}");
        }
    }

    #[test]
    fn device_change_limit_points_to_starting_a_new_sync() {
        for kind in [
            OperationKind::Sync,
            OperationKind::RemoveDevice([1; 32]),
            OperationKind::FinishRemoval,
        ] {
            let error = anyhow!("error membership-change-limit")
                .context("error sync-device-change-limit hint=\"x\"");
            assert_eq!(failure(kind, &error).message, CHANGE_LIMIT);
        }
        let error = anyhow!("error membership-change-limit");
        assert_eq!(failure(OperationKind::Sync, &error).message, CHANGE_LIMIT);
        assert!(CHANGE_LIMIT.contains("Recover from device loss"));
    }

    #[test]
    fn unknown_errors_offer_resuming_the_same_attempt() {
        let error = anyhow!("error snapshot-receipt-mismatch");
        let failure = failure(OperationKind::Setup, &error);
        assert!(failure.message.contains("same setup"));
        assert_eq!(failure.details, "error snapshot-receipt-mismatch");
    }
}
