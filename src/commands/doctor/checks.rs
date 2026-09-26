use std::path::Path;

use crate::config::{self as app_config, AppConfig};
use crate::workspaces::resolve_active_workspace_with_database;

use super::super::data_safety::{
    attachment_integrity_checks, database_integrity_report, ensure_integrity_ok,
};
use super::{DoctorReport, DoctorSection};

fn format_optional_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

fn stable_check_code(prefix: &str, label: &str) -> String {
    let suffix = label
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>()
        .split('_')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_");
    format!("{prefix}.{suffix}")
}

fn add_integrity_check(
    section: &mut DoctorSection,
    check: &aven_core::data_safety::IntegrityCheck,
) {
    let code = stable_check_code("integrity", check.label);
    if check.label == "recurrence projection gaps" && !check.ok {
        section.warning(
            "integrity.recurrence_projection_gaps",
            check.label,
            format!(
                "{}; repair by running `aven recur list`, then rerun `aven doctor --integrity`",
                check.value
            ),
        );
    } else if check.label.starts_with("recurrence ") && !check.ok {
        section.check(
            "integrity.recurrence",
            check.label,
            false,
            format!(
                "{}; restore a known-good backup or export unaffected data before repair",
                check.value
            ),
        );
    } else {
        section.check(code, check.label, check.ok, &check.value);
    }
}

pub(super) async fn add_runtime_database_sections(
    report: &mut DoctorReport,
    database: Option<&aven_core::db::Database>,
    config: &AppConfig,
    config_valid: bool,
    db_path: Option<&Path>,
    workspace_flag: Option<&str>,
    integrity: bool,
) {
    let reason = "database schema is unavailable, pending migration, or unsupported";
    let mut integrity_blob_dir = None;
    let workspace_section = report.section("workspace", "Workspace");
    let mut resolved_workspace = None;
    if let Some(database) = database {
        match std::env::current_dir() {
            Ok(cwd) => {
                match resolve_active_workspace_with_database(database, workspace_flag, config, &cwd)
                    .await
                {
                    Ok(workspace) => {
                        workspace_section.check(
                            "workspace.active",
                            "active workspace",
                            true,
                            format!("{} ({})", workspace.name, workspace.key),
                        );
                        match database.workspace_task_counts(&workspace.id).await {
                            Ok(counts) => workspace_section.info(
                                "workspace.task_counts",
                                "tasks",
                                format!("{} visible, {} total", counts.visible, counts.total),
                            ),
                            Err(_) => workspace_section.check(
                                "workspace.task_counts",
                                "tasks",
                                false,
                                "could not read task counts",
                            ),
                        }
                        resolved_workspace = Some(workspace);
                    }
                    Err(error) => workspace_section.check(
                        "workspace.resolve",
                        "active workspace",
                        false,
                        format!("{error:#}"),
                    ),
                }
            }
            Err(_) => workspace_section.check(
                "workspace.cwd",
                "current directory",
                false,
                "current directory is unavailable",
            ),
        }
    } else {
        workspace_section.skipped("workspace.resolve_skipped", "active workspace", reason);
        workspace_section.skipped(
            "workspace.task_counts_skipped",
            "tasks",
            "active workspace could not be resolved",
        );
    }
    drop(resolved_workspace);

    let sync_section = report.section("sync", "Sync");
    if config_valid {
        sync_section.info(
            "sync.enabled",
            "enabled",
            if config.sync.enabled { "yes" } else { "no" },
        );
        sync_section.info(
            "sync.runtime_allowed",
            "runtime allowed",
            if config.sync_is_allowed() {
                "yes"
            } else {
                "no"
            },
        );
        if let Some(database) = database {
            match crate::sync::encrypted::status_report(database, config).await {
                Ok(status) => {
                    sync_section.info(
                        "sync.set_up",
                        "set up",
                        if status.state == crate::sync::encrypted::SyncState::NotSetUp {
                            "no; run `aven sync setup` or `aven sync join`"
                        } else {
                            "yes"
                        },
                    );
                    sync_section.info(
                        "sync.server",
                        "server",
                        status.server.as_deref().unwrap_or("none"),
                    );
                    sync_section.info(
                        "sync.state",
                        "state",
                        crate::sync::encrypted::status_state_words(status.state),
                    );
                }
                Err(error) => {
                    sync_section.check("sync.state", "state", false, format!("{error:#}"))
                }
            }
        }
        sync_section.info(
            "sync.interval",
            "interval",
            format!("{} seconds", config.sync_interval_seconds()),
        );
        match config.wake_addr() {
            Ok(addr) => {
                sync_section.check("sync.daemon_wake", "daemon wake", true, addr.to_string())
            }
            Err(_) => sync_section.check(
                "sync.daemon_wake",
                "daemon wake",
                false,
                "invalid daemon wake address; configure a loopback socket address",
            ),
        }
    } else {
        for (code, label) in [
            ("sync.settings_skipped", "settings"),
            ("sync.daemon_wake_skipped", "daemon wake"),
        ] {
            sync_section.skipped(code, label, "strict configuration loading failed");
        }
    }

    if let Some(database) = database {
        let database_section = report
            .sections
            .iter_mut()
            .find(|section| section.code == "database")
            .expect("database section exists");
        match database.meta("client_id").await {
            Ok(value) => database_section.check(
                "database.client_id",
                "client id",
                value.is_some(),
                value.as_deref().unwrap_or("missing"),
            ),
            Err(_) => database_section.check(
                "database.client_id",
                "client id",
                false,
                "could not read metadata",
            ),
        }
        for (code, label, key, absent) in [
            (
                "database.sync_cursor",
                "sync cursor",
                "sync_cursor",
                "missing",
            ),
            (
                "database.local_sequence",
                "local sequence",
                "local_seq",
                "missing",
            ),
        ] {
            match database.meta(key).await {
                Ok(value) => database_section.info(code, label, value.as_deref().unwrap_or(absent)),
                Err(_) => database_section.check(code, label, false, "could not read metadata"),
            }
        }
        match database.sync_history_stats().await {
            Ok(stats) => {
                database_section.info(
                    "database.change_rows",
                    "change rows",
                    stats.total_change_rows.to_string(),
                );
                database_section.info(
                    "database.pending_changes",
                    "pending changes",
                    stats.pending_change_rows.to_string(),
                );
                database_section.info(
                    "database.synced_changes",
                    "synced changes",
                    stats.synced_change_rows.to_string(),
                );
                database_section.info(
                    "database.min_server_seq",
                    "min server_seq",
                    format_optional_i64(stats.min_server_seq),
                );
                database_section.info(
                    "database.max_server_seq",
                    "max server_seq",
                    format_optional_i64(stats.max_server_seq),
                );
                database_section.info(
                    "database.payload_bytes",
                    "payload bytes",
                    stats.payload_bytes.to_string(),
                );
            }
            Err(_) => database_section.check(
                "database.sync_history",
                "change history",
                false,
                "could not read sync history",
            ),
        }
        match database.unresolved_conflict_count().await {
            Ok(count) => {
                database_section.info("database.conflicts", "conflicts", count.to_string())
            }
            Err(_) => database_section.check(
                "database.conflicts",
                "conflicts",
                false,
                "could not read conflicts",
            ),
        }
    }

    report.section("attachment_lifecycle", "Attachment lifecycle");
    let lifecycle_index = report.sections.len() - 1;
    report.section("attachments", "Attachments");
    let attachment_index = report.sections.len() - 1;
    let (before_attachments, attachments_and_after) =
        report.sections.split_at_mut(attachment_index);
    let lifecycle_section = &mut before_attachments[lifecycle_index];
    let attachment_section = &mut attachments_and_after[0];
    match (database, db_path) {
        (Some(database), Some(db_path)) => match app_config::resolve_blob_dir(db_path, config) {
            Ok(blob_dir) => {
                match database
                    .attachment_lifecycle_report(
                        &blob_dir,
                        config.local.attachment_lifecycle.policy(),
                    )
                    .await
                {
                    Ok(lifecycle) => {
                        for (code, label, count, bytes) in [
                            (
                                "attachments.referenced",
                                "referenced",
                                lifecycle.referenced.count,
                                lifecycle.referenced.bytes,
                            ),
                            (
                                "attachments.protected",
                                "protected",
                                lifecycle.protected.count,
                                lifecycle.protected.bytes,
                            ),
                            (
                                "attachments.grace_period",
                                "grace period",
                                lifecycle.grace_period.count,
                                lifecycle.grace_period.bytes,
                            ),
                            (
                                "attachments.eligible",
                                "eligible",
                                lifecycle.eligible.count,
                                lifecycle.eligible.bytes,
                            ),
                            (
                                "attachments.staging",
                                "staging",
                                lifecycle.staging.count,
                                lifecycle.staging.bytes,
                            ),
                            (
                                "attachments.trash",
                                "trash",
                                lifecycle.trash.count,
                                lifecycle.trash.bytes,
                            ),
                            (
                                "attachments.reservations",
                                "reservations",
                                lifecycle.reservations.count,
                                lifecycle.reservations.bytes,
                            ),
                        ] {
                            lifecycle_section.info(
                                code,
                                label,
                                format!("count={count} bytes={bytes}"),
                            );
                        }
                        lifecycle_section.check(
                            "attachments.quota",
                            "quota",
                            lifecycle.quota.bytes
                                <= u64::try_from(config.local.attachment_lifecycle.quota_bytes)
                                    .unwrap_or(0),
                            format!(
                                "count={} bytes={} limit={}",
                                lifecycle.quota.count,
                                lifecycle.quota.bytes,
                                config.local.attachment_lifecycle.quota_bytes
                            ),
                        );
                        lifecycle_section.check(
                            "attachments.inconsistencies",
                            "inconsistencies",
                            lifecycle.inconsistencies.count == 0,
                            format!(
                                "count={} bytes={}",
                                lifecycle.inconsistencies.count, lifecycle.inconsistencies.bytes
                            ),
                        );
                    }
                    Err(_) => lifecycle_section.check(
                        "attachments.lifecycle",
                        "lifecycle",
                        false,
                        "could not inspect attachment lifecycle",
                    ),
                }
                if integrity {
                    integrity_blob_dir = Some(blob_dir);
                    attachment_section.info(
                        "attachments.integrity_deferred",
                        "integrity",
                        "reported in the Integrity section",
                    );
                } else {
                    match attachment_integrity_checks(database, &blob_dir, false).await {
                        Ok(checks) => {
                            for check in checks {
                                attachment_section.check(
                                    stable_check_code("attachments", check.label),
                                    check.label,
                                    check.ok,
                                    check.value,
                                );
                            }
                        }
                        Err(_) => attachment_section.check(
                            "attachments.integrity",
                            "integrity",
                            false,
                            "could not inspect attachments",
                        ),
                    }
                }
            }
            Err(_) => {
                lifecycle_section.check(
                    "attachments.path",
                    "blob directory",
                    false,
                    "attachment path could not be resolved",
                );
                attachment_section.skipped(
                    "attachments.skipped",
                    "inspection",
                    "attachment path could not be resolved",
                );
            }
        },
        _ => {
            lifecycle_section.skipped("attachments.lifecycle_skipped", "inspection", reason);
            attachment_section.skipped("attachments.skipped", "inspection", reason);
        }
    }

    if integrity {
        let integrity_section = report.section("integrity", "Integrity");
        match (database, db_path) {
            (Some(database), Some(_)) => {
                let database_result = database_integrity_report(database).await;
                let attachment_result = if let Some(blob_dir) = integrity_blob_dir.as_deref() {
                    attachment_integrity_checks(database, blob_dir, true).await
                } else {
                    Err(anyhow::anyhow!("attachment path could not be resolved"))
                };
                let mut combined = match database_result {
                    Ok(integrity_report) => {
                        integrity_section.check(
                            "integrity.quick_check",
                            "quick check",
                            integrity_report.quick_check_ok,
                            &integrity_report.quick_check_value,
                        );
                        for check in &integrity_report.checks {
                            add_integrity_check(integrity_section, check);
                        }
                        Some(integrity_report)
                    }
                    Err(_) => {
                        integrity_section.check(
                            "integrity.failed",
                            "result",
                            false,
                            "integrity checks could not complete; preserve the database and restore a known-good backup",
                        );
                        None
                    }
                };
                match attachment_result {
                    Ok(checks) => {
                        for check in &checks {
                            integrity_section.check(
                                stable_check_code("integrity", check.label),
                                check.label,
                                check.ok,
                                &check.value,
                            );
                        }
                        if let Some(combined) = &mut combined {
                            combined.checks.extend(checks);
                        }
                    }
                    Err(_) => integrity_section.check(
                        "integrity.attachments",
                        "attachment integrity",
                        false,
                        "could not inspect attachments",
                    ),
                }
                if let Some(combined) = combined
                    && let Err(error) = ensure_integrity_ok(&combined)
                {
                    integrity_section.check(
                        "integrity.result",
                        "result",
                        false,
                        format!("{error:#}"),
                    );
                }
            }
            _ => integrity_section.skipped("integrity.skipped", "result", reason),
        }
    }
}

pub(super) fn add_daemon_section(report: &mut DoctorReport, config: &AppConfig) {
    let daemon_section = report.section("daemon", "Daemon");
    match crate::daemon::status_snapshot() {
        Ok(snapshot) => {
            let status = crate::status::build_daemon_status(config, snapshot);
            daemon_section.info("daemon.state", "state", status.state.as_str());
            daemon_section.info(
                "daemon.installed",
                "installed",
                if status.installed { "yes" } else { "no" },
            );
            match status.loaded {
                Some(loaded) => daemon_section.check(
                    "daemon.loaded",
                    "loaded",
                    loaded,
                    if loaded { "yes" } else { "no" },
                ),
                None => daemon_section.info("daemon.loaded", "loaded", "unavailable"),
            }
            match status.running {
                Some(running) => daemon_section.check(
                    "daemon.running",
                    "running",
                    running,
                    if running { "yes" } else { "no" },
                ),
                None => daemon_section.info("daemon.running", "running", "unavailable"),
            }
            if let Some(path) = &status.paths.service {
                daemon_section.info("daemon.plist", "plist", path.display().to_string());
            }
            daemon_section.info(
                "daemon.program",
                "program",
                status
                    .paths
                    .program
                    .as_ref()
                    .map(|path| path.display().to_string())
                    .unwrap_or_else(|| "missing".to_string()),
            );
            if let Some(path) = &status.paths.current_executable {
                daemon_section.info(
                    "daemon.current_executable",
                    "current exe",
                    path.display().to_string(),
                );
            }
            match status.executable_matches {
                Some(matches) => daemon_section.check(
                    "daemon.program_match",
                    "program match",
                    matches,
                    if matches { "yes" } else { "no" },
                ),
                None => daemon_section.info("daemon.program_match", "program match", "unavailable"),
            }
        }
        Err(_) => daemon_section.check(
            "daemon.status",
            "status",
            false,
            "daemon status is unavailable; inspect the service manager directly",
        ),
    }
}
