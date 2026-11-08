use anyhow::Result;

use crate::config::AppConfig;
use crate::render::print_json_pretty;
use crate::status::DaemonStatusReport;

pub(crate) async fn cmd_daemon_status(
    config: &AppConfig,
    db_path: &std::path::Path,
    json: bool,
) -> Result<()> {
    let mut report = crate::status::build_daemon_status(config, crate::daemon::status_snapshot()?);
    if db_path.exists() {
        let database = aven_core::db::Database::open(db_path).await?;
        report = report
            .with_sync(database.daemon_sync_state().await?)
            .with_host(aven_host::host::host_state(&database).await?);
    }
    report.notifications = tokio::task::spawn_blocking(crate::notification::authorization)
        .await?
        .and_then(|read| {
            read.inspect_err(
                |error| tracing::debug!(error = %format!("{error:#}"), "notification status"),
            )
            .ok()
        });
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
    if report.turned_off_in_login_items == Some(true) {
        println!("Login Items: Aven is turned off");
    }
    println!(
        "Configuration: sync {}, reminders {}, executable match {}",
        on_off(report.sync_enabled),
        on_off(report.reminders_enabled),
        optional_yes_no(report.executable_matches),
    );
    if let Some(host) = &report.host {
        print_host(host);
    }
    if let Some(authorization) = report.notifications {
        match crate::notification::describe(authorization) {
            (state, Some(remedy)) => println!(
                "Notifications: {state}; {}",
                crate::render::stdout_inline_code(remedy)
            ),
            (state, None) => println!("Notifications: {state}"),
        }
    }
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
        println!(
            "Next: {}",
            crate::render::stdout_inline_code(&error.message)
        );
    }
    for guidance in &report.guidance {
        println!("Next: {}", crate::render::stdout_inline_code(guidance));
    }
}

fn on_off(value: bool) -> &'static str {
    if value { "on" } else { "off" }
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

fn print_host(host: &aven_host::status::HostReport) {
    let state = host.state.as_str();
    let Some(status) = &host.status else {
        println!("Background host: {state}");
        return;
    };
    let what = format!(
        "{}, pid {}, version {}",
        aven_host::host::describe_role(&status.role).replace('`', ""),
        status.pid,
        status.version
    );
    match host.state {
        aven_host::status::HostReportState::Stopped => {
            println!("Background host: {state} (last: {what})");
        }
        _ => {
            let jobs = if status.capabilities.is_empty() {
                "idle".to_string()
            } else {
                status.capabilities.join(", ")
            };
            println!(
                "Background host: {state} ({what}; {jobs}; last check-in {})",
                crate::status::daemon_time(status.heartbeat_at)
            );
        }
    }
}
