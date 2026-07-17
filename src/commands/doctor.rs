use std::io::{self, IsTerminal};
use std::path::Path;

use anyhow::Result;
use crossterm::style::{Color, Stylize};
use sqlx::SqliteConnection;

use super::data_safety::{database_integrity_report, ensure_integrity_ok};
use crate::config::{self as app_config, AppConfig};
use crate::db::get_meta;
use crate::query;
use crate::render::print_json_pretty;
use crate::sync::sync_server_url_is_valid;
use crate::workspaces::resolve_active_workspace;

#[derive(serde::Serialize)]
pub(super) struct DoctorReport {
    pub(super) sections: Vec<DoctorSection>,
}

impl DoctorReport {
    pub(super) fn new() -> Self {
        Self {
            sections: Vec::new(),
        }
    }

    pub(super) fn section(&mut self, title: &'static str) -> &mut DoctorSection {
        self.sections.push(DoctorSection {
            title,
            rows: Vec::new(),
        });
        self.sections.last_mut().expect("section was pushed")
    }
}

#[derive(serde::Serialize)]
pub(super) struct DoctorSection {
    pub(super) title: &'static str,
    pub(super) rows: Vec<DoctorRow>,
}

impl DoctorSection {
    pub(super) fn check(&mut self, label: &'static str, ok: bool, value: impl Into<String>) {
        self.rows.push(DoctorRow {
            status: if ok {
                DoctorStatus::Ok
            } else {
                DoctorStatus::Error
            },
            label,
            value: value.into(),
        });
    }

    pub(super) fn info(&mut self, label: &'static str, value: impl Into<String>) {
        self.rows.push(DoctorRow {
            status: DoctorStatus::Info,
            label,
            value: value.into(),
        });
    }
}

#[derive(serde::Serialize)]
pub(super) struct DoctorRow {
    pub(super) status: DoctorStatus,
    pub(super) label: &'static str,
    pub(super) value: String,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DoctorStatus {
    Ok,
    Error,
    Info,
}

pub(super) struct DoctorRenderer {
    styled: bool,
}

impl DoctorRenderer {
    pub(super) fn auto() -> Self {
        Self {
            styled: io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    pub(super) fn print(&self, report: &DoctorReport) {
        if self.styled {
            println!(
                "{}",
                "aven doctor"
                    .with(Color::Rgb {
                        r: 45,
                        g: 174,
                        b: 135
                    })
                    .bold()
            );
        } else {
            println!("aven doctor");
        }
        for section in &report.sections {
            println!();
            self.print_section(section.title);
            let label_width = section
                .rows
                .iter()
                .map(|row| row.label.chars().count())
                .max()
                .unwrap_or(0);
            for row in &section.rows {
                self.print_row(row, label_width);
            }
        }
    }

    fn print_section(&self, title: &str) {
        if self.styled {
            println!("{}", title.with(Color::Cyan).bold());
        } else {
            println!("{title}");
            println!("{}", "-".repeat(title.len()));
        }
    }

    fn print_row(&self, row: &DoctorRow, label_width: usize) {
        if self.styled {
            self.print_styled_row(row, label_width);
        } else {
            let marker = row.status.marker();
            println!("  {marker} {:<18} {}", row.label, row.value);
        }
    }

    fn print_styled_row(&self, row: &DoctorRow, label_width: usize) {
        let label = format!("{:<label_width$}", row.label);
        println!(
            "  {} {}  {}",
            row.status.icon().with(row.status.color()).bold(),
            label.with(row.status.label_color()),
            row.value.as_str().with(Color::Rgb {
                r: 150,
                g: 150,
                b: 150,
            })
        );
    }
}

impl DoctorStatus {
    fn marker(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "!!",
            Self::Info => "..",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Error => "✗",
            Self::Info => "·",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Ok => Color::Green,
            Self::Error => Color::Red,
            Self::Info => Color::DarkGrey,
        }
    }

    fn label_color(self) -> Color {
        match self {
            Self::Ok | Self::Error => Color::White,
            Self::Info => Color::Grey,
        }
    }
}
fn format_optional_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

pub(crate) async fn cmd_doctor(
    conn: &mut SqliteConnection,
    config: &AppConfig,
    db_path: &Path,
    db_flag_set: bool,
    workspace_flag: Option<&str>,
    integrity: bool,
    json: bool,
) -> Result<()> {
    let config_file = app_config::config_file_path();
    let db_source = if db_flag_set {
        "--db"
    } else if std::env::var_os("AVEN_DB").is_some() {
        "AVEN_DB"
    } else if config.local.db_path.is_some() {
        "config local.db_path"
    } else {
        "default"
    };
    let client_id = get_meta(conn, "client_id").await?;
    let sync_cursor = get_meta(conn, "sync_cursor").await?;
    let local_seq = get_meta(conn, "local_seq").await?;
    let pinned_server = get_meta(conn, "sync_server_url").await?;
    let cwd = std::env::current_dir()?;
    let workspace = resolve_active_workspace(conn, workspace_flag, config, &cwd).await;
    let counts = match &workspace {
        Ok(workspace) => Some(query::workspace_task_counts(conn, &workspace.id).await?),
        Err(_) => None,
    };
    let sync_history = query::sync_history_stats(conn).await?;
    let unresolved_conflicts = query::unresolved_conflict_count(conn).await?;
    let sync_server = app_config::resolve_sync_server(None, config);
    let wake_addr = config.wake_addr();

    let mut report = DoctorReport::new();
    let config_section = report.section("Configuration");
    match config_file {
        Ok(path) if path.exists() => {
            config_section.check("config file", true, path.display().to_string());
        }
        Ok(path) => {
            config_section.info(
                "config file",
                format!("{} (using defaults)", path.display()),
            );
        }
        Err(error) => {
            config_section.check("config file", false, format!("{error:#}"));
        }
    }
    config_section.info("database source", db_source);
    config_section.info("database path", db_path.display().to_string());

    let database_section = report.section("Database");
    database_section.check("sqlite", true, "opened successfully");
    database_section.check(
        "client id",
        client_id.is_some(),
        client_id.as_deref().unwrap_or("missing"),
    );
    database_section.info("sync cursor", sync_cursor.as_deref().unwrap_or("missing"));
    database_section.info("local sequence", local_seq.as_deref().unwrap_or("missing"));
    database_section.info("pinned server", pinned_server.as_deref().unwrap_or("none"));
    database_section.info("change rows", sync_history.total_change_rows.to_string());
    database_section.info(
        "pending changes",
        sync_history.pending_change_rows.to_string(),
    );
    database_section.info(
        "synced changes",
        sync_history.synced_change_rows.to_string(),
    );
    database_section.info(
        "min server_seq",
        format_optional_i64(sync_history.min_server_seq),
    );
    database_section.info(
        "max server_seq",
        format_optional_i64(sync_history.max_server_seq),
    );
    database_section.info("payload bytes", sync_history.payload_bytes.to_string());
    database_section.info("conflicts", unresolved_conflicts.to_string());

    let workspace_section = report.section("Workspace");
    match workspace {
        Ok(workspace) => {
            workspace_section.check(
                "active workspace",
                true,
                format!("{} ({})", workspace.name, workspace.key),
            );
            if let Some(counts) = counts {
                workspace_section.info(
                    "tasks",
                    format!("{} visible, {} total", counts.visible, counts.total),
                );
            }
        }
        Err(error) => {
            workspace_section.check("active workspace", false, format!("{error:#}"));
        }
    }

    let sync_section = report.section("Sync");
    sync_section.info("enabled", if config.sync.enabled { "yes" } else { "no" });
    match sync_server {
        Ok(server) => {
            sync_section.check("server", sync_server_url_is_valid(&server), &server);
            if let Some(pinned) = pinned_server.as_deref() {
                let normalized = server.trim_end_matches('/');
                sync_section.check(
                    "server match",
                    pinned == normalized,
                    format!("pinned={pinned} configured={normalized}"),
                );
            }
        }
        Err(error) => {
            if config.sync.enabled {
                sync_section.check("server", false, format!("{error:#}"));
            } else {
                sync_section.info("server", "not configured");
            }
        }
    }
    match config.sync.server_url.as_deref() {
        Some(server) => {
            sync_section.check("daemon server", sync_server_url_is_valid(server), server)
        }
        None if config.sync.enabled => sync_section.check("daemon server", false, "not configured"),
        None => sync_section.info("daemon server", "not configured"),
    }
    sync_section.info(
        "auth token",
        if config.sync_auth_token().is_some() {
            "configured"
        } else {
            "not configured"
        },
    );
    sync_section.info(
        "interval",
        format!("{} seconds", config.sync_interval_seconds()),
    );
    match wake_addr {
        Ok(addr) => sync_section.check("daemon wake", true, addr.to_string()),
        Err(error) => sync_section.check("daemon wake", false, format!("{error:#}")),
    }

    let daemon_status = crate::daemon::status_snapshot()?;
    let daemon_section = report.section("Daemon");
    daemon_section.info(
        "installed",
        if daemon_status.installed { "yes" } else { "no" },
    );
    match daemon_status.loaded {
        Some(loaded) => daemon_section.check("loaded", loaded, if loaded { "yes" } else { "no" }),
        None => daemon_section.info("loaded", "unknown"),
    }
    daemon_section.info("plist", daemon_status.plist_path.display().to_string());
    daemon_section.info(
        "program",
        daemon_status
            .program
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "missing".to_string()),
    );
    daemon_section.info(
        "current exe",
        daemon_status.current_executable.display().to_string(),
    );
    match daemon_status.program_matches_current {
        Some(matches) => {
            daemon_section.check("program match", matches, if matches { "yes" } else { "no" })
        }
        None => daemon_section.info("program match", "unknown"),
    }

    if integrity {
        let integrity_report = database_integrity_report(conn).await?;
        let integrity_section = report.section("Integrity");
        integrity_section.check(
            "quick check",
            integrity_report.quick_check_ok,
            &integrity_report.quick_check_value,
        );
        for check in &integrity_report.checks {
            integrity_section.check(check.label, check.ok, &check.value);
        }
        if let Err(error) = ensure_integrity_ok(&integrity_report) {
            integrity_section.check("result", false, format!("{error:#}"));
        }
    }

    if json {
        print_json_pretty(&report)?;
    } else {
        DoctorRenderer::auto().print(&report);
    }
    Ok(())
}
