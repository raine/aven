use anyhow::Result;

use crate::config::AppConfig;
use crate::render::print_json_pretty;
use crate::status::DaemonStatusReport;

pub(crate) async fn cmd_daemon_status(
    config: &AppConfig,
    db_path: &std::path::Path,
    json: bool,
) -> Result<()> {
    let sync = if db_path.exists() {
        aven_core::db::Database::open(db_path)
            .await?
            .daemon_sync_state()
            .await?
    } else {
        Default::default()
    };
    let report = crate::status::build_daemon_status(config, crate::daemon::status_snapshot()?)
        .with_sync(sync);
    if json {
        print_json_pretty(&report)
    } else {
        print_daemon_status(&report);
        Ok(())
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
    println!(
        "Last daemon sync: {}",
        report
            .sync
            .last_success_at
            .and_then(|at| chrono::DateTime::from_timestamp(at, 0))
            .map_or_else(|| "never recorded".to_string(), |at| at.to_rfc3339())
    );
    if let Some(error) = &report.sync.error {
        println!(
            "Daemon error: {} since {}",
            error.code,
            chrono::DateTime::from_timestamp(error.since, 0)
                .map_or_else(|| error.since.to_string(), |at| at.to_rfc3339())
        );
        println!("Next: {}", error.message);
    }
    for guidance in &report.guidance {
        println!("Next: {guidance}");
    }
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
