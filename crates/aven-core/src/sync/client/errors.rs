//! Stable error codes carried by client errors.
//!
//! Every engine error message starts `error <code>`, optionally followed by
//! details and a `hint="..."`. Hosts map codes, never message text.
use anyhow::Error;

/// The stable error codes in the chain, outermost first.
pub fn codes(error: &Error) -> impl Iterator<Item = String> + '_ {
    error.chain().filter_map(|cause| {
        cause
            .to_string()
            .strip_prefix("error ")
            .and_then(|rest| rest.split_whitespace().next())
            .map(str::to_string)
    })
}

/// The outermost stable error code in the chain.
pub fn code(error: &Error) -> Option<String> {
    codes(error).next()
}

pub fn has_code(error: &Error, expected: &str) -> bool {
    codes(error).any(|code| code == expected)
}

/// The server saw an older membership head than its current one.
pub fn is_stale(error: &Error) -> bool {
    error.is::<crate::sync::seed_claim::membership::StaleContext>()
}

/// The server refused this device's credential or membership, which may
/// mean another device removed it.
pub fn is_access_refusal(error: &Error) -> bool {
    has_code(error, "sync-server-refused")
        || has_code(error, "enrollment-unauthorized")
        || has_code(error, "enrollment-revoked")
        || has_code(error, "sync-device-removed")
}

/// The server reports this vault deleted for good: rerunning sync can't
/// recover it, and a new setup is required.
pub fn is_vault_deleted(error: &Error) -> bool {
    has_code(error, VAULT_DELETED)
}

const VAULT_DELETED: &str = "sync-vault-deleted";

/// Hosting replies cannot authenticate the outcome of a pending operation.
pub(crate) fn hosting_error(hosting: aven_protocol::refusal::Hosting) -> Error {
    use aven_protocol::refusal::Hosting;
    let code = match hosting {
        Hosting::Blocked => "sync-hosting-blocked",
        Hosting::Deleted => VAULT_DELETED,
        Hosting::Quota => "sync-hosting-quota",
        Hosting::Unavailable => "sync-hosting-unavailable",
    };
    anyhow::anyhow!("error {code} outcome-unknown")
}

pub(crate) fn is_hosting_refusal(error: &Error) -> bool {
    codes(error).any(|code| {
        matches!(
            code.as_str(),
            "sync-hosting-blocked"
                | "sync-hosting-quota"
                | "sync-hosting-unavailable"
                | VAULT_DELETED
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aven_protocol::refusal::Hosting;

    #[test]
    fn every_hosting_refusal_stops_transfers_and_only_deletion_is_terminal() {
        for hosting in [
            Hosting::Blocked,
            Hosting::Deleted,
            Hosting::Quota,
            Hosting::Unavailable,
        ] {
            let error = hosting_error(hosting).context("error sync-round");
            assert!(is_hosting_refusal(&error));
            assert!(!is_access_refusal(&error));
            assert_eq!(is_vault_deleted(&error), hosting == Hosting::Deleted);
        }
    }
}
