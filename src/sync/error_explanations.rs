//! Shared plain-language explanations for sync-related errors.
//!
//! Stable engine codes remain available to scripts and diagnostics, while the
//! user-facing message and next step avoid exposing internal error chains.
use anyhow::Error;

use crate::protected_local_keys::{ProtectedLocalKeyStoreError, ProtectedLocalKeyStoreErrorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorAction {
    General,
    Setup,
    Join,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorSurface {
    Cli,
    Tui,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Explanation {
    pub(crate) code: &'static str,
    pub(crate) message: &'static str,
    pub(crate) next_step: &'static str,
}

impl Explanation {
    pub(crate) fn combined(self) -> String {
        match self.next_step {
            "" => self.message.to_string(),
            next => format!("{} {next}", self.message),
        }
    }
}

pub(crate) use aven_core::sync::client::errors::{codes, has_code};

pub(crate) fn explain(
    action: ErrorAction,
    surface: ErrorSurface,
    error: &Error,
) -> Option<Explanation> {
    let all = codes(error).collect::<Vec<_>>();
    let has = |expected: &str| all.iter().any(|code| code == expected);
    // Codes that share one explanation; the first present one is displayed.
    let first = |family: &[&'static str]| family.iter().copied().find(|code| has(code));

    if all.iter().any(|code| code.ends_with("-tls")) {
        let code =
            first(&["enrollment-tls", "bootstrap-tls", "encrypted-tail-tls"]).unwrap_or("sync-tls");
        return Some(Explanation {
            code,
            message: "Couldn't establish a secure connection to the sync server.",
            next_step: "Check that the server is reachable and its TLS certificate is valid for this host and trusted by this device. Local work continues.",
        });
    }
    if has("sync-request-body-limit") {
        return Some(Explanation {
            code: "sync-request-body-limit",
            message: "The sync request was rejected as too large.",
            next_step: "If a reverse proxy is in use, raise its request body limit to at least 8 MB, then try again. Local work continues.",
        });
    }
    if all.iter().any(|code| code.ends_with("-network")) {
        let code = first(&[
            "enrollment-network",
            "bootstrap-network",
            "encrypted-tail-network",
        ])
        .unwrap_or("sync-network");
        return Some(Explanation {
            code,
            message: "Couldn't reach the sync server.",
            next_step: "Check the connection and try again. Local work continues.",
        });
    }
    if has("sync-device-invitation-setup") {
        return Some(Explanation {
            code: "sync-device-invitation-invalid",
            message: "This invitation starts sync on a new server; it doesn't add a device.",
            next_step: match surface {
                ErrorSurface::Cli => "Run `aven sync setup` with this invitation.",
                ErrorSurface::Tui => "Go back and choose Set up sync.",
            },
        });
    }
    if has("sync-setup-invitation-device") {
        return Some(Explanation {
            code: "sync-setup-invitation-invalid",
            message: "This invitation adds a device to existing sync; it doesn't set up a server.",
            next_step: match surface {
                ErrorSurface::Cli => "Run `aven sync join` with this invitation.",
                ErrorSurface::Tui => "Go back and choose Join existing sync.",
            },
        });
    }
    if has("sync-device-invitation-invalid") {
        return Some(Explanation {
            code: "sync-device-invitation-invalid",
            message: "This isn't a valid device invitation.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Run `aven sync invite` on a device already in sync, then paste the complete invitation."
                }
                ErrorSurface::Tui => {
                    "Go back, choose Add device on a device already in sync, and paste the complete invitation."
                }
            },
        });
    }
    if has("sync-setup-invitation-invalid") {
        return Some(Explanation {
            code: "sync-setup-invitation-invalid",
            message: "This isn't a valid setup invitation.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Copy the complete setup invitation from your hosting provider, or run `aven server setup` if you host the server yourself, then paste it into `aven sync setup`."
                }
                ErrorSurface::Tui => {
                    "Copy the complete setup invitation from your hosting provider, or from `aven server setup` if you host the server yourself, then paste it into Set up sync."
                }
            },
        });
    }
    if has("e2ee-server-already-claimed") {
        return Some(Explanation {
            code: "e2ee-server-already-claimed",
            message: "This server storage already belongs to an encrypted sync.",
            next_step: "Start the existing server normally. For a new sync, stop the server and move its storage aside, or pass a new --data path.",
        });
    }
    if has("e2ee-data-only-import-unavailable") {
        return Some(Explanation {
            code: "e2ee-data-only-import-unavailable",
            message: "This export came from a database that takes part in encrypted sync, so it \
                      can't be imported.",
            next_step: "Restore an aven backup archive into a new database path instead.",
        });
    }
    if has("e2ee-installation-fenced") {
        return Some(Explanation {
            code: "e2ee-installation-fenced",
            message: "This database takes part in encrypted sync, so restore and import can't \
                      replace its data without breaking sync with other devices.",
            next_step: "Restore or import into a new database path instead, for example \
                        `aven --db <new-path> backup restore <archive> --yes`.",
        });
    }
    if has("sync-not-set-up") {
        return Some(Explanation {
            code: "sync-not-set-up",
            message: "Sync isn't set up for this database.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Run `aven sync setup` or use `aven sync join` with a new database."
                }
                ErrorSurface::Tui => "Choose Set up sync or Join existing sync.",
            },
        });
    }
    if has("sync-disabled-by-environment") {
        return Some(Explanation {
            code: "sync-disabled",
            message: "Sync is disabled by the AVEN_SYNC_DISABLED environment variable.",
            next_step: "Unset AVEN_SYNC_DISABLED and try again.",
        });
    }
    if has("sync-disabled") {
        return Some(Explanation {
            code: "sync-disabled",
            message: "Automatic sync is turned off in config.yaml.",
            next_step: "Run `aven config set sync.enabled true` and try again.",
        });
    }
    if has("sync-setup-confirmation-required") {
        return Some(Explanation {
            code: "sync-setup-confirmation-required",
            message: "Setup needs confirmation, and standard input isn't a terminal.",
            next_step: "Review the summary above, then rerun `aven sync setup` with --yes.",
        });
    }
    if has("sync-join-confirmation-required") {
        return Some(Explanation {
            code: "sync-join-confirmation-required",
            message: "Joining needs confirmation, and standard input isn't a terminal.",
            next_step: "Check the server above, then rerun `aven sync join` with --yes.",
        });
    }
    if has("sync-reset-setup-in-progress") {
        return Some(Explanation {
            code: "sync-reset-setup-in-progress",
            message: "Setup is unfinished, so reset stopped to protect its keys.",
            next_step: "Resume with `aven sync setup`. If the original setup can no longer be resumed, run `aven sync reset --force`.",
        });
    }
    if has("sync-setup-invitation-mismatch") {
        return Some(Explanation {
            code: "sync-setup-invitation-mismatch",
            message: "This invitation is for different server storage than the setup started here.",
            next_step: SETUP_MISMATCH_NEXT_STEP,
        });
    }
    if has("sync-setup-server-mismatch") {
        return Some(Explanation {
            code: "sync-setup-server-mismatch",
            message: "This invitation is for a different server than the setup started here.",
            next_step: SETUP_MISMATCH_NEXT_STEP,
        });
    }
    if let Some(explanation) = protected_key_store_explanation(surface, error) {
        return Some(explanation);
    }
    if has("enrollment-protected-missing") {
        return Some(protected_key_store_kind_explanation(
            ProtectedLocalKeyStoreErrorKind::MissingAuthority,
            surface,
        ));
    }
    if first(&[
        "enrollment-protected-framing",
        "enrollment-protected-corrupt",
        "enrollment-protected-limit",
        "enrollment-protected-conflict",
    ])
    .is_some()
    {
        return Some(protected_key_store_kind_explanation(
            ProtectedLocalKeyStoreErrorKind::Corrupt,
            surface,
        ));
    }
    if has("enrollment-protected-write") {
        return Some(protected_key_store_kind_explanation(
            ProtectedLocalKeyStoreErrorKind::WriteFailed,
            surface,
        ));
    }
    if has("sync-setup-storage-already-claimed") {
        return Some(Explanation {
            code: "sync-setup-storage-already-claimed",
            message: "This server storage already belongs to another sync.",
            next_step: "Join that sync from an empty database, or set up sync with empty server storage. Nothing here was changed.",
        });
    }
    if has("sync-setup-fenced-invitation-rejected") {
        return Some(Explanation {
            code: "sync-setup-fenced-invitation-rejected",
            message: "This frozen setup couldn't use that invitation.",
            next_step: "Resume with the invitation that started setup, or the newest invitation for the same server storage. If neither is available, back up and restore to a new path.",
        });
    }
    if has("sync-setup-invitation-expired") {
        return Some(Explanation {
            code: "sync-setup-invitation-expired",
            message: "This setup invitation expired.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Get a new setup invitation from your hosting provider, or run `aven server setup` again if you host the server yourself, then rerun `aven sync setup` with it. Nothing here was changed."
                }
                ErrorSurface::Tui => {
                    "Get a new setup invitation from your hosting provider, or run `aven server setup` again if you host the server yourself, then choose Set up sync and paste it. Nothing here was changed."
                }
            },
        });
    }
    if has("sync-setup-invitation-rejected") {
        return Some(Explanation {
            code: "sync-setup-invitation-rejected",
            message: "This setup invitation expired, was replaced, or belongs to different storage.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Get the current setup invitation from your hosting provider, or run `aven server setup` if you host the server yourself, then rerun `aven sync setup` with it. Nothing here was changed."
                }
                ErrorSurface::Tui => {
                    "Get the current setup invitation from your hosting provider, or run `aven server setup` if you host the server yourself, then choose Set up sync and paste it. Nothing here was changed."
                }
            },
        });
    }
    if has("sync-setup-outcome-unknown") {
        return Some(Explanation {
            code: "sync-setup-outcome-unknown",
            message: "The server claim couldn't be confirmed.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Rerun `aven sync setup` to resume the same setup. Local work continues."
                }
                ErrorSurface::Tui => "Choose Resume setup. Local work continues.",
            },
        });
    }
    if has("sync-setup-fenced-storage-claimed") {
        return Some(Explanation {
            code: "sync-setup-fenced-storage-claimed",
            message: "The server reported that this storage belongs to another sync.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Rerun `aven sync setup` to retry the same setup. If it keeps failing, back up and restore to a new path. Local work continues."
                }
                ErrorSurface::Tui => {
                    "Choose Resume setup to retry. If it keeps failing, back up and restore to a new path. Local work continues."
                }
            },
        });
    }
    if has("sync-setup-refused") {
        return Some(Explanation {
            code: "sync-setup-refused",
            message: "The server refused this setup invitation.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Run `aven server setup` for empty server storage and retry `aven sync setup` with its invitation."
                }
                ErrorSurface::Tui => {
                    "Use a setup invitation for empty server storage, then retry Set up sync."
                }
            },
        });
    }
    if has("sync-already-set-up") {
        return Some(Explanation {
            code: "sync-already-set-up",
            message: "This database already takes part in sync.",
            next_step: match surface {
                ErrorSurface::Cli => "Run `aven sync` or `aven sync status`.",
                ErrorSurface::Tui => "Choose Sync now.",
            },
        });
    }
    if has("sync-join-requires-empty-database") {
        return Some(Explanation {
            code: "sync-join-requires-empty-database",
            message: "This database already has tasks or other data, and joining needs an empty database.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Join from a new database with `aven --db PATH sync join`. Nothing here was changed."
                }
                ErrorSurface::Tui => {
                    "Start aven with `aven --db /new/path sync join` to use a new, empty database. Nothing here was changed."
                }
            },
        });
    }
    if let Some(code) = first(&[
        "sync-join-target-not-empty",
        "shared-state-install",
        "snapshot-target-not-fresh",
    ]) {
        return Some(Explanation {
            code,
            message: "Data was added to this database while joining, so synced tasks can't be installed here.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Join from a new, empty database with `aven --db PATH sync join`."
                }
                ErrorSurface::Tui => {
                    "Start aven with `aven --db /new/path sync join` to use a new, empty database."
                }
            },
        });
    }
    if has("sync-join-timeout") {
        return Some(Explanation {
            code: "sync-join-timeout",
            message: "The other device didn't add this device in time.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Keep `aven sync invite` running on the other device, then rerun `aven sync join`."
                }
                ErrorSurface::Tui => {
                    "Keep Add device or `aven sync invite` open on the other device, then choose Resume joining."
                }
            },
        });
    }
    if has("sync-join-server-mismatch") {
        return Some(Explanation {
            code: "sync-join-server-mismatch",
            message: "This invitation is for a different server than the join already started here.",
            next_step: "Use an invitation from the device that created the first one.",
        });
    }
    if let Some(code) = first(&[
        "sync-join-invitation-conflict",
        "enrollment-invitation-conflict",
    ]) {
        return Some(Explanation {
            code,
            message: "This invitation doesn't match the join already started here.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Resume with the earlier invitation, or run `aven sync join --new-invitation`."
                }
                ErrorSurface::Tui => "Choose Resume joining or Use a new invitation.",
            },
        });
    }
    for (code, message, cli, tui) in [
        (
            "sync-join-new-invitation-mismatch",
            "A new invitation must come from the device that created the first one.",
            "Run `aven sync invite` on that device.",
            "Open Add device on that device and copy a new invitation.",
        ),
        (
            "sync-join-new-invitation-unavailable",
            "The other device already added this device, so a new invitation isn't needed.",
            "Rerun `aven sync join` to resume.",
            "Choose Resume joining.",
        ),
        (
            "sync-join-new-invitation-limit",
            "This database has reached its invitation limit.",
            "Rerun `aven sync join` to finish an accepted attempt; otherwise join from a new empty database.",
            "Resume joining to finish an accepted attempt; otherwise join from a new empty database.",
        ),
    ] {
        if has(code) {
            return Some(Explanation {
                code,
                message,
                next_step: if surface == ErrorSurface::Cli {
                    cli
                } else {
                    tui
                },
            });
        }
    }
    if let Some(code) = first(&["sync-key-change-required", "withdrawal-rotation-required"]) {
        return Some(Explanation {
            code,
            message: "An invitation expired after keys may have been sent to a device that never joined.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Check the connection and run `aven sync` again; downloads continue and uploads resume after keys change."
                }
                ErrorSurface::Tui => {
                    "Check the connection and choose Sync now again; downloads continue and uploads resume after keys change."
                }
            },
        });
    }
    if let Some(code) = first(&[
        "sync-invitation-unresolved",
        "withdrawal-required-unsupported",
    ]) {
        return Some(Explanation {
            code,
            message: "The last invitation may have sent keys to a device that hasn't joined.",
            next_step: "Add a device again after it joins, or after the invitation expires and the next sync changes keys.",
        });
    }
    if let Some(code) = first(&["sync-device-change-limit", "membership-change-limit"]) {
        return Some(Explanation {
            code,
            message: "This sync has reached its limit on device changes.",
            next_step: "Start a new sync to keep changing devices; see Recover from device loss in the sync docs.",
        });
    }
    if has("sync-invitation-cancelled") {
        return Some(Explanation {
            code: "sync-invitation-cancelled",
            message: "Invitation cancelled by another command.",
            next_step: match surface {
                ErrorSurface::Cli => "Run `aven sync invite` to add a device.",
                ErrorSurface::Tui => "Choose Add device to create a new invitation.",
            },
        });
    }
    if has("enrollment-busy") {
        return Some(Explanation {
            code: "enrollment-busy",
            message: "The server is busy with another device.",
            next_step: "Try again in a moment.",
        });
    }
    if let Some(code) = first(&["sync-device-removed", "enrollment-revoked"]) {
        return Some(access_refused(code));
    }
    if has("sync-join-command") && has("enrollment-refused") {
        return Some(Explanation {
            code: "enrollment-refused",
            message: "The server refused the join request.",
            next_step: "Sync hosting may be temporarily unavailable, or the invitation may have expired, been cancelled, or already been used. Retry `aven sync join` with the same invitation later; if it's still refused, run `aven sync invite` on the other device for a new one.",
        });
    }
    // Setup and join run before this device has access to refuse.
    let refused: &[&'static str] = if action == ErrorAction::General {
        &["sync-server-refused", "enrollment-unauthorized"]
    } else {
        &["sync-server-refused"]
    };
    if let Some(code) = first(refused) {
        return Some(access_refused(code));
    }
    if let Some(code) = first(&["enrollment-timeout", "enrollment-server"]) {
        return Some(Explanation {
            code,
            message: "The sync server couldn't complete the request.",
            next_step: "Try again later. Local work continues.",
        });
    }
    if let Some(code) = first(&["sync-device-removal-unfinished", "management-unfinished"]) {
        return Some(Explanation {
            code,
            message: "An earlier device removal from this device is unfinished.",
            next_step: match surface {
                ErrorSurface::Cli => "Run `aven sync` to finish it, then retry.",
                ErrorSurface::Tui => "Choose Finish removal before removing another device.",
            },
        });
    }
    if has("sync-device-not-found") {
        return Some(Explanation {
            code: "sync-device-not-found",
            message: "That device is no longer in sync.",
            next_step: match surface {
                ErrorSurface::Cli => "Run `aven sync device list` again.",
                ErrorSurface::Tui => "Refresh the device list.",
            },
        });
    }
    if has("sync-device-current") {
        return Some(Explanation {
            code: "sync-device-current",
            message: "This is the current device.",
            next_step: "Remove it from another device in sync.",
        });
    }
    if has("attachment-quota-exceeded") {
        return Some(Explanation {
            code: "attachment-quota-exceeded",
            message: "The sync server's image storage limit for this workspace is full.",
            next_step: "Remove images you no longer need, or ask the server operator to raise `local.attachment_lifecycle.server_workspace_quota_bytes`, then try again.",
        });
    }
    if has("enrollment-store-unsupported") {
        return Some(Explanation {
            code: "enrollment-store-unsupported",
            message: "This version of Aven can't read the protected sync data on this device.",
            next_step: "Update Aven on this device. Do not replace or delete its protected sync keys.",
        });
    }
    if let Some(code) = first(UNSUPPORTED_CHANGE) {
        return Some(Explanation {
            code,
            message: "Another device sent a change this version of Aven doesn't understand, so sync stopped here. Local tasks are safe and editable.",
            next_step: "Update Aven on this device; edits made meanwhile sync after the update.",
        });
    }
    if let Some(code) = first(&["encrypted-tail-storage"]) {
        return Some(Explanation {
            code,
            message: "Sync couldn't write to this device's database, which may be full or busy. Local tasks are safe.",
            next_step: "Free up disk space or close other Aven processes; sync retries automatically.",
        });
    }
    if let Some(code) = first(&["encrypted-tail-apply", "encrypted-tail-domain"]) {
        return Some(Explanation {
            code,
            message: "Sync stopped because this device couldn't apply a synced change. Local tasks are safe and editable.",
            next_step: "Update Aven on this device and every other device, then sync again. If it still stops on the latest version, report it before rebuilding sync: rebuilding doesn't merge other devices' unsynced edits.",
        });
    }
    if let Some(code) = first(BAD_RECORD) {
        return Some(Explanation {
            code,
            message: "Sync stopped on a change it can't apply, to protect your data. Local tasks are safe and editable.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "To keep syncing, rebuild sync on fresh server storage with `aven sync reset`: https://aven.raine.dev/sync/#rebuilding-sync"
                }
                ErrorSurface::Tui => {
                    "To keep syncing, rebuild sync on fresh server storage; run `aven sync reset` and see https://aven.raine.dev/sync/#rebuilding-sync"
                }
            },
        });
    }
    if has("encrypted-tail-refused") {
        return Some(Explanation {
            code: "encrypted-tail-refused",
            message: "The sync server refused this sync.",
            next_step: "Local tasks and unsynced changes stay on this device. Try again later; if it keeps failing, check with your hosting provider or server operator.",
        });
    }
    if let Some(code) = first(&["enrollment-refused", "bootstrap-refused"]) {
        return Some(match action {
            ErrorAction::Join => Explanation {
                code,
                message: "The server refused the join request.",
                next_step: "Sync hosting may be temporarily unavailable, or the invitation may have expired, been cancelled, or already been used. Retry with the same invitation later; if it's still refused, get a new invitation.",
            },
            ErrorAction::Setup => Explanation {
                code,
                message: "The server refused the setup request.",
                next_step: "Check the server and retry with its current setup invitation.",
            },
            _ => Explanation {
                code,
                message: "The server refused the request.",
                next_step: "Check the server and try again.",
            },
        });
    }
    None
}

/// A different setup or server cannot resume the setup started here, and a
/// mismatch is never a reason to discard the setup's keys.
const SETUP_MISMATCH_NEXT_STEP: &str = "Use the invitation that started this setup, or a newer one for the same server storage. If neither is available, back up this database and restore it to a new path for a local-only copy. This setup's keys are kept.";

/// A synced change uses an operation or value this version doesn't know.
const UNSUPPORTED_CHANGE: &[&str] = &[
    "encrypted-tail-operation-unsupported",
    "unsupported-remote-change",
];

/// A synced change, or this device's record of one, can never be applied or
/// uploaded as it is, so every later sync stops at the same place.
const BAD_RECORD: &[&str] = &[
    "encrypted-tail-invalid",
    "encrypted-tail-payload",
    "encrypted-tail-json",
    "encrypted-tail-projection",
    "encrypted-tail-workspace",
    "encrypted-tail-seed",
    "encrypted-tail-authentication",
    "encrypted-tail-generation",
    "encrypted-tail-generation-rank",
    "encrypted-tail-label-history",
    "encrypted-tail-note-history",
    "encrypted-tail-history-lost",
    "encrypted-tail-mapping",
    "encrypted-tail-same-id-divergence",
    "encrypted-tail-integrity-blocked",
    "encrypted-tail-prefix-identity-collision",
    "encrypted-image-initial-catch-up",
    "encrypted-image-projection",
];

fn protected_key_store_explanation(surface: ErrorSurface, error: &Error) -> Option<Explanation> {
    let kind = error.chain().find_map(|cause| {
        cause
            .downcast_ref::<ProtectedLocalKeyStoreError>()
            .map(ProtectedLocalKeyStoreError::kind)
    })?;
    Some(protected_key_store_kind_explanation(kind, surface))
}

#[cfg(target_os = "macos")]
fn protected_storage_unavailable_next_step(surface: ErrorSurface) -> &'static str {
    match surface {
        ErrorSurface::Cli => {
            "Unlock the login Keychain. Retry the command and choose Always Allow on the Aven Keychain request."
        }
        ErrorSurface::Tui => {
            "Unlock the login Keychain. Choose Sync now, then choose Always Allow on the Aven Keychain request."
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn protected_storage_unavailable_next_step(surface: ErrorSurface) -> &'static str {
    match surface {
        ErrorSurface::Cli => {
            "Check that the protected state directory is readable and writable only by your user, then retry the same command."
        }
        ErrorSurface::Tui => {
            "Check that the protected state directory is readable and writable only by your user, then retry the action."
        }
    }
}

fn protected_key_store_kind_explanation(
    kind: ProtectedLocalKeyStoreErrorKind,
    surface: ErrorSurface,
) -> Explanation {
    match kind {
        ProtectedLocalKeyStoreErrorKind::MissingAuthority => Explanation {
            code: "protected-key-storage-missing",
            message: "Protected sync keys are missing from this device.",
            next_step: "Do not replace them with new keys; recover this device from a known-good backup or another device.",
        },
        ProtectedLocalKeyStoreErrorKind::Unavailable => Explanation {
            code: "protected-key-storage-unavailable",
            message: "Protected sync key storage is unavailable.",
            next_step: protected_storage_unavailable_next_step(surface),
        },
        ProtectedLocalKeyStoreErrorKind::Corrupt => Explanation {
            code: "protected-key-storage-unsafe",
            message: "Protected sync key storage is corrupt or unsafe.",
            next_step: "Do not replace its keys. Check the protected storage and recover from a known-good backup if needed.",
        },
        ProtectedLocalKeyStoreErrorKind::WriteFailed => Explanation {
            code: "protected-key-storage-write-failed",
            message: "Protected sync keys couldn't be saved.",
            next_step: "Check access to the system key store and retry the same command.",
        },
        ProtectedLocalKeyStoreErrorKind::WrongDatabase => Explanation {
            code: "protected-key-storage-wrong-database",
            message: "These protected sync keys belong to a different database installation.",
            next_step: "Use the database that owns these keys or recover from a known-good backup.",
        },
        ProtectedLocalKeyStoreErrorKind::SetupMismatch => Explanation {
            code: "protected-key-storage-setup-mismatch",
            message: "The setup invitation doesn't match the protected setup state.",
            next_step: "Resume with the invitation that started this setup.",
        },
        ProtectedLocalKeyStoreErrorKind::UnsupportedPlatform => Explanation {
            code: "protected-key-storage-unsupported",
            message: "Protected sync key storage isn't supported on this platform.",
            next_step: "Use a supported macOS or Linux installation for encrypted sync.",
        },
    }
}

fn access_refused(code: &'static str) -> Explanation {
    Explanation {
        code,
        message: "Access unconfirmed: the server refused this device.",
        next_step: "It may have been removed from sync; check from another device. Local tasks and images stay here.",
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;

    #[test]
    fn unavailable_protected_storage_uses_surface_specific_recovery() {
        let error = anyhow::Error::new(ProtectedLocalKeyStoreError::new(
            ProtectedLocalKeyStoreErrorKind::Unavailable,
        ));
        let cli = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        let tui = explain(ErrorAction::General, ErrorSurface::Tui, &error).unwrap();

        assert!(!tui.next_step.contains("terminal"), "{}", tui.next_step);
        assert!(!tui.next_step.contains("rebuild"), "{}", tui.next_step);
        assert_ne!(cli.next_step, tui.next_step);
        #[cfg(target_os = "macos")]
        {
            assert!(tui.next_step.contains("Sync now"), "{}", tui.next_step);
            assert!(tui.next_step.contains("Always Allow"), "{}", tui.next_step);
            assert!(!cli.next_step.contains("Sync now"), "{}", cli.next_step);
        }
    }

    #[test]
    fn network_explanation_keeps_stable_code_without_promising_an_outcome() {
        let error = anyhow!("error enrollment-network outcome-unknown")
            .context("error sync-join-incomplete");
        let explanation = explain(ErrorAction::Join, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(explanation.code, "enrollment-network");
        assert!(explanation.combined().contains("Local work continues"));
        assert!(!explanation.combined().contains("nothing changed"));
    }

    #[test]
    fn tls_explanation_names_certificate_checks() {
        let error = anyhow!("error bootstrap-tls outcome-unknown")
            .context("error sync-setup-outcome-unknown");
        let explanation = explain(ErrorAction::Setup, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(explanation.code, "bootstrap-tls");
        assert!(explanation.message.contains("secure connection"));
        assert!(explanation.next_step.contains("certificate"));
        assert!(explanation.next_step.contains("trusted"));
    }

    #[test]
    fn request_limit_explanation_qualifies_proxy_guidance() {
        let error = anyhow!("error sync-request-body-limit");
        let explanation = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(explanation.code, "sync-request-body-limit");
        assert!(explanation.message.contains("request"));
        assert!(
            explanation
                .next_step
                .contains("If a reverse proxy is in use")
        );
        assert!(explanation.next_step.contains("8 MB"));
    }

    #[test]
    fn access_refusal_names_removal_only_as_a_possibility() {
        let error = anyhow!("error enrollment-unauthorized")
            .context("error sync-server-refused hint=\"raw\"");
        let explanation = explain(ErrorAction::General, ErrorSurface::Tui, &error).unwrap();
        assert_eq!(explanation.code, "sync-server-refused");
        assert!(explanation.combined().contains("may have been removed"));
        assert!(!explanation.combined().contains("was removed"));
    }

    #[test]
    fn tail_access_refusal_explains_possible_removal() {
        let error = anyhow::Error::from(aven_core::sync::seed_claim::membership::Unauthorized)
            .context("error sync-server-refused hint=\"raw\"");
        let explanation = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(explanation.code, "sync-server-refused");
        assert!(explanation.combined().contains("may have been removed"));
        assert!(explanation.combined().contains("another device"));
    }

    #[test]
    fn disabled_sync_names_its_cause() {
        let mut config = crate::config::AppConfig::default();
        config.sync.disable_override = true;
        let error = config.ensure_sync_allowed().unwrap_err();
        let environment = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(environment.code, "sync-disabled");
        assert!(environment.next_step.contains("AVEN_SYNC_DISABLED"));

        let error = crate::config::AppConfig::default()
            .ensure_automatic_sync_enabled()
            .unwrap_err();
        let configured = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(configured.code, "sync-disabled");
        assert!(configured.next_step.contains("sync.enabled true"));
        assert!(!configured.combined().contains("AVEN_SYNC_DISABLED"));
    }

    #[test]
    fn confirmation_required_names_the_flag() {
        for (code, command) in [
            ("sync-setup-confirmation-required", "`aven sync setup`"),
            ("sync-join-confirmation-required", "`aven sync join`"),
        ] {
            let error = anyhow!("error {code} hint=\"rerun with --yes to confirm\"");
            let explanation = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
            assert_eq!(explanation.code, code);
            assert!(explanation.next_step.contains(command));
            assert!(explanation.next_step.contains("--yes"));
        }
    }

    #[test]
    fn refused_join_retries_before_a_new_invitation() {
        for (surface, error) in [
            (
                ErrorSurface::Cli,
                anyhow!("error enrollment-refused").context("error sync-join-command"),
            ),
            (ErrorSurface::Tui, anyhow!("error enrollment-refused")),
        ] {
            let explanation = explain(ErrorAction::Join, surface, &error).unwrap();
            assert!(explanation.next_step.contains("been cancelled"));
            assert!(explanation.next_step.contains("temporarily unavailable"));
            assert!(explanation.next_step.contains("same invitation later"));
            assert!(!explanation.combined().contains("removed"));
        }
    }

    #[test]
    fn cli_invitation_guidance_names_commands() {
        let error = anyhow!("error sync-device-invitation-invalid");
        let explanation = explain(ErrorAction::Join, ErrorSurface::Cli, &error).unwrap();
        assert!(explanation.next_step.contains("`aven sync invite`"));
    }

    #[test]
    fn expired_setup_invitation_is_named_and_points_to_hosting_or_server_setup() {
        let error = anyhow!("error bootstrap-setup-invitation-expired")
            .context("error sync-setup-invitation-expired hint=\"expired\"");
        let cli = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(cli.code, "sync-setup-invitation-expired");
        assert_eq!(cli.message, "This setup invitation expired.");
        assert!(cli.next_step.contains("`aven sync setup`"));
        let tui = explain(ErrorAction::General, ErrorSurface::Tui, &error).unwrap();
        assert!(tui.next_step.contains("choose Set up sync"));

        let error = anyhow!("error sync-setup-invitation-rejected");
        let rejected = explain(ErrorAction::General, ErrorSurface::Tui, &error).unwrap();
        assert!(rejected.message.contains("expired, was replaced"));

        let error = anyhow!("error sync-setup-invitation-invalid");
        let invalid = explain(ErrorAction::Setup, ErrorSurface::Cli, &error).unwrap();
        for step in [
            cli.next_step,
            tui.next_step,
            rejected.next_step,
            invalid.next_step,
        ] {
            assert!(step.contains("your hosting provider"), "{step}");
            assert!(step.contains("`aven server setup`"), "{step}");
        }
    }

    #[test]
    fn setup_mismatches_keep_the_setup_without_suggesting_reset() {
        for code in [
            "sync-setup-invitation-mismatch",
            "sync-setup-server-mismatch",
        ] {
            let error = anyhow!("error {code} hint=\"raw\"");
            for surface in [ErrorSurface::Cli, ErrorSurface::Tui] {
                let explanation = explain(ErrorAction::Setup, surface, &error).unwrap();
                assert_eq!(explanation.code, code);
                assert!(!explanation.combined().contains("reset"), "{explanation:?}");
                assert!(explanation.next_step.contains("same server storage"));
                assert!(explanation.next_step.contains("keys are kept"));
            }
        }
    }

    #[test]
    fn refused_tail_keeps_local_work_without_suggesting_reset() {
        let tail = anyhow!("error encrypted-tail-refused outcome-unknown");
        let explanation = explain(ErrorAction::General, ErrorSurface::Cli, &tail).unwrap();
        assert_eq!(explanation.code, "encrypted-tail-refused");
        assert!(explanation.next_step.contains("unsynced changes stay"));
        assert!(!explanation.combined().contains("reset"));
    }

    #[test]
    fn enrollment_protected_storage_codes_use_actionable_storage_explanations() {
        for (raw, code) in [
            (
                "enrollment-protected-missing",
                "protected-key-storage-missing",
            ),
            (
                "enrollment-protected-framing",
                "protected-key-storage-unsafe",
            ),
            (
                "enrollment-protected-corrupt",
                "protected-key-storage-unsafe",
            ),
            ("enrollment-protected-limit", "protected-key-storage-unsafe"),
            (
                "enrollment-protected-conflict",
                "protected-key-storage-unsafe",
            ),
            (
                "enrollment-protected-write",
                "protected-key-storage-write-failed",
            ),
        ] {
            let error = anyhow!("error {raw}");
            let explanation = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
            assert_eq!(explanation.code, code);
        }
    }

    #[test]
    fn unsupported_changes_ask_for_an_update() {
        for code in UNSUPPORTED_CHANGE {
            let error = anyhow!("error {code}").context("error sync-round");
            for surface in [ErrorSurface::Cli, ErrorSurface::Tui] {
                let explanation = explain(ErrorAction::General, surface, &error).unwrap();
                assert_eq!(explanation.code, *code);
                assert!(explanation.message.contains("doesn't understand"));
                assert!(explanation.message.contains("Local tasks are safe"));
                assert!(explanation.next_step.contains("Update Aven"));
                assert!(!explanation.combined().contains("reset"));
            }
        }
    }

    #[test]
    fn rejected_applies_ask_for_an_update_first() {
        for code in ["encrypted-tail-apply", "encrypted-tail-domain"] {
            let error = anyhow!("error task-not-found sequence=7 op_type=set_field")
                .context(format!("error {code}"));
            let explanation = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
            assert_eq!(explanation.code, code);
            assert!(explanation.next_step.contains("Update Aven"));
            assert!(!explanation.combined().contains("reset"));
        }
        let error = anyhow!("error encrypted-tail-storage");
        let explanation = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        assert!(explanation.next_step.contains("retries"));
        assert!(!explanation.combined().contains("reset"));
    }

    #[test]
    fn bad_records_point_to_rebuilding_sync() {
        for code in BAD_RECORD {
            let error = anyhow!("error {code}").context("error sync-round");
            for surface in [ErrorSurface::Cli, ErrorSurface::Tui] {
                let explanation = explain(ErrorAction::General, surface, &error).unwrap();
                assert_eq!(explanation.code, *code);
                assert!(explanation.message.contains("to protect your data"));
                assert!(
                    explanation
                        .message
                        .contains("Local tasks are safe and editable")
                );
                assert!(explanation.next_step.contains("`aven sync reset`"));
                assert!(explanation.next_step.contains("/sync/#rebuilding-sync"));
            }
        }
        let error = anyhow::Error::from(aven_core::sync::encrypted_tail::PrefixIdentityCollision);
        let explanation = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(explanation.code, "encrypted-tail-prefix-identity-collision");
    }

    #[test]
    fn transient_tail_failures_are_not_bad_records() {
        for code in [
            "encrypted-tail-http",
            "encrypted-tail-reply",
            "encrypted-tail-cursor",
        ] {
            let error = anyhow!("error {code}");
            assert_eq!(
                explain(ErrorAction::General, ErrorSurface::Cli, &error),
                None
            );
        }
        let error = anyhow!("error encrypted-tail-network outcome-unknown")
            .context("error encrypted-image-initial-catch-up");
        let explanation = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(explanation.code, "encrypted-tail-network");
        assert!(!explanation.combined().contains("reset"));
    }

    #[test]
    fn image_quota_names_the_server_setting() {
        let error = anyhow!("error attachment-quota-exceeded").context("error sync-round");
        for surface in [ErrorSurface::Cli, ErrorSurface::Tui] {
            let explanation = explain(ErrorAction::General, surface, &error).unwrap();
            assert_eq!(explanation.code, "attachment-quota-exceeded");
            assert!(explanation.message.contains("sync server"));
            assert!(
                explanation
                    .next_step
                    .contains("server_workspace_quota_bytes")
            );
        }
    }

    #[test]
    fn unsupported_protected_sync_data_asks_for_an_update() {
        let error = anyhow!("error enrollment-store-unsupported");
        let explanation = explain(ErrorAction::Join, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(explanation.code, "enrollment-store-unsupported");
        assert!(explanation.next_step.contains("Update Aven"));
        assert!(explanation.next_step.contains("Do not replace"));
        assert!(!explanation.combined().contains("reset"));
    }
}
