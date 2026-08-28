use anyhow::Result;
use aven_core::db::Database;

use crate::config::AppConfig;
use crate::render::print_json_pretty;
use crate::status::{DaemonStatusReport, SyncStatusReport};

pub(crate) async fn cmd_sync_status(
    database: &Database,
    config: &AppConfig,
    db_path: &std::path::Path,
    json: bool,
) -> Result<()> {
    let report = crate::status::build_sync_status(database, config, db_path).await?;
    if json {
        print_json_pretty(&report)
    } else {
        print_sync_status(&report);
        Ok(())
    }
}

pub(crate) fn cmd_daemon_status(config: &AppConfig, json: bool) -> Result<()> {
    let report = crate::status::build_daemon_status(config, crate::daemon::status_snapshot()?);
    if json {
        print_json_pretty(&report)
    } else {
        print_daemon_status(&report);
        Ok(())
    }
}

fn print_sync_status(report: &SyncStatusReport) {
    println!("Sync: {}", report.state.as_str());
    println!(
        "Configuration: {}",
        if report.configured {
            if report.enabled && report.runtime_allowed {
                "enabled"
            } else {
                "disabled"
            }
        } else {
            "unconfigured"
        }
    );
    if let Some(server) = &report.effective_server {
        println!("Server: {server}");
    }
    if let Some(pinned) = &report.pinned_server {
        let suffix = match report.server_matches_pin {
            Some(true) => " (matches configuration)",
            Some(false) => " (does not match configuration)",
            None => "",
        };
        println!("Pinned server: {pinned}{suffix}");
    }
    println!(
        "Work: {} pending changes, {} attachment uploads ({} bytes), {} attachment downloads ({} bytes), {} conflicts",
        report.pending.changes,
        report.pending.attachment_uploads,
        report.pending.attachment_upload_bytes,
        report.pending.attachment_downloads,
        report.pending.attachment_download_bytes,
        report.unresolved_conflicts,
    );
    println!(
        "Progress: cursor {}, local sequence {}",
        display_number(report.progress.cursor),
        display_number(report.progress.local_sequence),
    );
    println!(
        "Last: attempt {}, success {}",
        display(report.last.attempt_at.as_deref()),
        display(report.last.success_at.as_deref()),
    );
    if let Some(error) = &report.last.safe_error {
        println!("Last error: {error}");
    }
    for guidance in &report.guidance {
        println!("Next: {guidance}");
    }
}

fn print_daemon_status(report: &DaemonStatusReport) {
    println!("Daemon: {}", report.state.as_str());
    println!(
        "Service: platform {}, installed {}, loaded {}, running {}",
        if report.platform_supported {
            "supported"
        } else {
            "unsupported"
        },
        yes_no(report.installed),
        optional_yes_no(report.loaded),
        optional_yes_no(report.running),
    );
    println!(
        "Configuration: {}, executable match {}",
        if report.configuration_valid {
            "valid"
        } else {
            "invalid"
        },
        optional_yes_no(report.executable_matches),
    );
    if let Some(path) = &report.paths.service {
        println!("Service file: {}", path.display());
    }
    if let Some(path) = &report.paths.program {
        println!("Program: {}", path.display());
    }
    if let Some(path) = &report.paths.stdout_log {
        println!("Logs: {}", path.display());
    }
    if let Some(path) = &report.paths.stderr_log {
        println!("Errors: {}", path.display());
    }
    for guidance in &report.guidance {
        println!("Next: {guidance}");
    }
}

fn display(value: Option<&str>) -> &str {
    value.unwrap_or("unavailable")
}

fn display_number(value: Option<i64>) -> String {
    value.map_or_else(|| "unavailable".to_string(), |value| value.to_string())
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

fn optional_yes_no(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unavailable",
    }
}
