//! Shared plain-language explanations for sync-related errors.
//!
//! Stable engine codes remain available to scripts and diagnostics, while the
//! user-facing message and next step avoid exposing internal error chains.
use anyhow::Error;

use crate::protected_local_keys::{ProtectedLocalKeyStoreError, ProtectedLocalKeyStoreErrorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ErrorAction {
    General,
    Sync,
    Setup,
    Join,
    ListDevices,
    RemoveDevice,
    FinishRemoval,
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

    if all.iter().any(|code| code.ends_with("-network")) {
        let code = if has("enrollment-network") {
            "enrollment-network"
        } else if has("bootstrap-network") {
            "bootstrap-network"
        } else if has("encrypted-tail-network") {
            "encrypted-tail-network"
        } else {
            "sync-network"
        };
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
                    "Run `aven server setup`, then paste the complete invitation into `aven sync setup`."
                }
                ErrorSurface::Tui => {
                    "Run server setup, then paste its complete invitation into Set up sync."
                }
            },
        });
    }
    if has("e2ee-server-already-claimed") {
        return Some(Explanation {
            code: "e2ee-server-already-claimed",
            message: "This server storage already belongs to an encrypted sync.",
            next_step: "Start the existing server normally, or choose empty storage for a new sync.",
        });
    }
    if has("e2ee-data-only-import-unavailable") {
        return Some(Explanation {
            code: "e2ee-data-only-import-unavailable",
            message: "Import is unavailable for a database that takes part in encrypted sync.",
            next_step: "Import into a new local database instead.",
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
    if has("sync-setup-invitation-mismatch") {
        return Some(Explanation {
            code: "sync-setup-invitation-mismatch",
            message: "This invitation belongs to a different setup.",
            next_step: match surface {
                ErrorSurface::Cli => {
                    "Rerun `aven sync setup` with the invitation that started this setup."
                }
                ErrorSurface::Tui => "Resume with the invitation that started this setup.",
            },
        });
    }
    if let Some(explanation) = protected_key_store_explanation(error) {
        return Some(explanation);
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
                    "Run `aven server setup` on the server again for a new invitation, then rerun `aven sync setup` with it. Nothing here was changed."
                }
                ErrorSurface::Tui => {
                    "Run `aven server setup` on the server again for a new invitation, then choose Set up sync and paste it. Nothing here was changed."
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
                    "Run `aven server setup` on the server for a current invitation, then rerun `aven sync setup` with it. Nothing here was changed."
                }
                ErrorSurface::Tui => {
                    "Run `aven server setup` on the server for a current invitation, then choose Set up sync and paste it. Nothing here was changed."
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
    if has("shared-state-install")
        || has("snapshot-target-not-fresh")
        || has("sync-join-target-not-empty")
    {
        return Some(Explanation {
            code: if has("sync-join-target-not-empty") {
                "sync-join-target-not-empty"
            } else if has("shared-state-install") {
                "shared-state-install"
            } else {
                "snapshot-target-not-fresh"
            },
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
    if has("sync-join-invitation-conflict") || has("enrollment-invitation-conflict") {
        return Some(Explanation {
            code: if has("sync-join-invitation-conflict") {
                "sync-join-invitation-conflict"
            } else {
                "enrollment-invitation-conflict"
            },
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
    if has("sync-key-change-required") || has("withdrawal-rotation-required") {
        return Some(Explanation {
            code: if has("sync-key-change-required") {
                "sync-key-change-required"
            } else {
                "withdrawal-rotation-required"
            },
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
    if has("sync-invitation-unresolved") || has("withdrawal-required-unsupported") {
        return Some(Explanation {
            code: if has("sync-invitation-unresolved") {
                "sync-invitation-unresolved"
            } else {
                "withdrawal-required-unsupported"
            },
            message: "The last invitation may have sent keys to a device that hasn't joined.",
            next_step: "Add a device again after it joins, or after the invitation expires and the next sync changes keys.",
        });
    }
    if has("sync-device-change-limit") || has("membership-change-limit") {
        return Some(Explanation {
            code: if has("sync-device-change-limit") {
                "sync-device-change-limit"
            } else {
                "membership-change-limit"
            },
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
    if has("enrollment-revoked") || has("sync-device-removed") {
        return Some(access_refused(if has("sync-device-removed") {
            "sync-device-removed"
        } else {
            "enrollment-revoked"
        }));
    }
    if has("sync-join-command") && has("enrollment-refused") {
        return Some(Explanation {
            code: "enrollment-refused",
            message: "The server refused the join request.",
            next_step: "The invitation may have expired, been cancelled, or already been used; run `aven sync invite` on the other device and try again.",
        });
    }
    if has("sync-server-refused")
        || (has("enrollment-unauthorized")
            && matches!(
                action,
                ErrorAction::Sync
                    | ErrorAction::ListDevices
                    | ErrorAction::RemoveDevice
                    | ErrorAction::FinishRemoval
                    | ErrorAction::General
            ))
    {
        return Some(access_refused(if has("sync-server-refused") {
            "sync-server-refused"
        } else {
            "enrollment-unauthorized"
        }));
    }
    if has("enrollment-timeout") || has("enrollment-server") {
        return Some(Explanation {
            code: if has("enrollment-timeout") {
                "enrollment-timeout"
            } else {
                "enrollment-server"
            },
            message: "The sync server couldn't complete the request.",
            next_step: "Try again later. Local work continues.",
        });
    }
    if has("sync-device-removal-unfinished") || has("management-unfinished") {
        return Some(Explanation {
            code: if has("sync-device-removal-unfinished") {
                "sync-device-removal-unfinished"
            } else {
                "management-unfinished"
            },
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
    if has("enrollment-refused") || has("bootstrap-refused") {
        let code = if has("enrollment-refused") {
            "enrollment-refused"
        } else {
            "bootstrap-refused"
        };
        return Some(match action {
            ErrorAction::Join => Explanation {
                code,
                message: "The server refused the join request.",
                next_step: "The invitation may have expired, been cancelled, or already been used; get a new invitation and try again.",
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

fn protected_key_store_explanation(error: &Error) -> Option<Explanation> {
    let kind = error.chain().find_map(|cause| {
        cause
            .downcast_ref::<ProtectedLocalKeyStoreError>()
            .map(ProtectedLocalKeyStoreError::kind)
    })?;
    Some(match kind {
        ProtectedLocalKeyStoreErrorKind::MissingAuthority => Explanation {
            code: "protected-key-storage-missing",
            message: "Protected sync keys are missing from this device.",
            next_step: "Do not replace them with new keys; recover this device from a known-good backup or another device.",
        },
        ProtectedLocalKeyStoreErrorKind::Unavailable => Explanation {
            code: "protected-key-storage-unavailable",
            message: "Protected sync key storage is unavailable.",
            next_step: "Make the system key store available, then retry the same command.",
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
    })
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
    fn network_explanation_keeps_stable_code_without_promising_an_outcome() {
        let error = anyhow!("error enrollment-network outcome-unknown")
            .context("error sync-join-incomplete");
        let explanation = explain(ErrorAction::Join, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(explanation.code, "enrollment-network");
        assert!(explanation.combined().contains("Local work continues"));
        assert!(!explanation.combined().contains("nothing changed"));
    }

    #[test]
    fn access_refusal_names_removal_only_as_a_possibility() {
        let error = anyhow!("error enrollment-unauthorized")
            .context("error sync-server-refused hint=\"raw\"");
        let explanation = explain(ErrorAction::Sync, ErrorSurface::Tui, &error).unwrap();
        assert_eq!(explanation.code, "sync-server-refused");
        assert!(explanation.combined().contains("may have been removed"));
        assert!(!explanation.combined().contains("was removed"));
    }

    #[test]
    fn disabled_sync_names_its_cause() {
        let mut config = crate::config::AppConfig::default();
        config.sync.disable_override = true;
        let error = config.ensure_sync_allowed().unwrap_err();
        let environment = explain(ErrorAction::Sync, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(environment.code, "sync-disabled");
        assert!(environment.next_step.contains("AVEN_SYNC_DISABLED"));

        let error = crate::config::AppConfig::default()
            .ensure_automatic_sync_enabled()
            .unwrap_err();
        let configured = explain(ErrorAction::Sync, ErrorSurface::Cli, &error).unwrap();
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
    fn refused_join_mentions_cancellation() {
        let error = anyhow!("error enrollment-refused").context("error sync-join-command");
        let explanation = explain(ErrorAction::Join, ErrorSurface::Cli, &error).unwrap();
        assert!(explanation.next_step.contains("been cancelled"));
        assert!(!explanation.combined().contains("removed"));
    }

    #[test]
    fn cli_invitation_guidance_names_commands() {
        let error = anyhow!("error sync-device-invitation-invalid");
        let explanation = explain(ErrorAction::Join, ErrorSurface::Cli, &error).unwrap();
        assert!(explanation.next_step.contains("`aven sync invite`"));
    }

    #[test]
    fn expired_setup_invitation_is_named_and_points_to_server_setup() {
        let error = anyhow!("error bootstrap-setup-invitation-expired")
            .context("error sync-setup-invitation-expired hint=\"expired\"");
        let cli = explain(ErrorAction::General, ErrorSurface::Cli, &error).unwrap();
        assert_eq!(cli.code, "sync-setup-invitation-expired");
        assert_eq!(cli.message, "This setup invitation expired.");
        assert!(cli.next_step.contains("`aven server setup`"));
        assert!(cli.next_step.contains("`aven sync setup`"));
        let tui = explain(ErrorAction::General, ErrorSurface::Tui, &error).unwrap();
        assert!(tui.next_step.contains("`aven server setup`"));
        assert!(tui.next_step.contains("choose Set up sync"));

        let error = anyhow!("error sync-setup-invitation-rejected");
        let rejected = explain(ErrorAction::General, ErrorSurface::Tui, &error).unwrap();
        assert!(rejected.message.contains("expired, was replaced"));
        assert!(rejected.next_step.contains("`aven server setup`"));
    }
}
