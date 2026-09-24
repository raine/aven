use anyhow::Result;

use crate::config::AppConfig;
use crate::render::print_json_pretty;
use crate::status::DaemonStatusReport;

pub(crate) fn cmd_daemon_status(config: &AppConfig, json: bool) -> Result<()> {
    let report = crate::status::build_daemon_status(config, crate::daemon::status_snapshot()?);
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
