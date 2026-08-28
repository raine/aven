use std::fs;
use std::io::{self, IsTerminal};
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use crossterm::style::{Color, Stylize};
use serde_yaml::Value;

use super::data_safety::{
    attachment_integrity_checks, database_integrity_report, ensure_integrity_ok,
};
use crate::cli::DoctorArgs;
use crate::config::{self as app_config, AppConfig};
use crate::render::print_json_pretty;
use crate::sync::sync_server_url_is_valid;
use crate::workspaces::resolve_active_workspace_with_database;

#[derive(serde::Serialize)]
pub(super) struct DoctorReport {
    pub(super) overall_status: DoctorStatus,
    pub(super) sections: Vec<DoctorSection>,
}

impl DoctorReport {
    fn new() -> Self {
        Self {
            overall_status: DoctorStatus::Ok,
            sections: Vec::new(),
        }
    }

    fn section(&mut self, code: &'static str, title: &'static str) -> &mut DoctorSection {
        self.sections.push(DoctorSection {
            code,
            title,
            status: DoctorStatus::Ok,
            rows: Vec::new(),
        });
        self.sections.last_mut().expect("section was pushed")
    }

    fn finish(&mut self) {
        for section in &mut self.sections {
            section.status = if section
                .rows
                .iter()
                .any(|row| row.status == DoctorStatus::Error)
            {
                DoctorStatus::Error
            } else if section
                .rows
                .iter()
                .any(|row| row.status == DoctorStatus::Warning)
            {
                DoctorStatus::Warning
            } else if section
                .rows
                .iter()
                .all(|row| row.status == DoctorStatus::Skipped)
            {
                DoctorStatus::Skipped
            } else {
                DoctorStatus::Ok
            };
        }
        self.overall_status = if self
            .sections
            .iter()
            .any(|section| section.status == DoctorStatus::Error)
        {
            DoctorStatus::Error
        } else if self
            .sections
            .iter()
            .any(|section| section.status == DoctorStatus::Warning)
        {
            DoctorStatus::Warning
        } else {
            DoctorStatus::Ok
        };
    }

    fn has_errors(&self) -> bool {
        self.sections
            .iter()
            .flat_map(|section| &section.rows)
            .any(|row| row.status == DoctorStatus::Error)
    }
}

#[derive(serde::Serialize)]
pub(super) struct DoctorSection {
    pub(super) code: &'static str,
    pub(super) title: &'static str,
    pub(super) status: DoctorStatus,
    pub(super) rows: Vec<DoctorRow>,
}

impl DoctorSection {
    fn row(
        &mut self,
        code: impl Into<String>,
        label: &'static str,
        status: DoctorStatus,
        value: impl Into<String>,
        skipped_reason: Option<String>,
    ) {
        self.rows.push(DoctorRow {
            code: code.into(),
            status,
            label,
            value: value.into(),
            skipped_reason,
        });
    }

    fn check(
        &mut self,
        code: impl Into<String>,
        label: &'static str,
        ok: bool,
        value: impl Into<String>,
    ) {
        self.row(
            code,
            label,
            if ok {
                DoctorStatus::Ok
            } else {
                DoctorStatus::Error
            },
            value,
            None,
        );
    }

    fn info(&mut self, code: impl Into<String>, label: &'static str, value: impl Into<String>) {
        self.row(code, label, DoctorStatus::Info, value, None);
    }

    fn warning(&mut self, code: impl Into<String>, label: &'static str, value: impl Into<String>) {
        self.row(code, label, DoctorStatus::Warning, value, None);
    }

    fn skipped(&mut self, code: impl Into<String>, label: &'static str, reason: impl Into<String>) {
        let reason = reason.into();
        self.row(code, label, DoctorStatus::Skipped, "skipped", Some(reason));
    }
}

#[derive(serde::Serialize)]
pub(super) struct DoctorRow {
    pub(super) code: String,
    pub(super) status: DoctorStatus,
    pub(super) label: &'static str,
    pub(super) value: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) skipped_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum DoctorStatus {
    Ok,
    Info,
    Skipped,
    Warning,
    Error,
}

pub(super) struct DoctorRenderer {
    styled: bool,
}

impl DoctorRenderer {
    fn auto() -> Self {
        Self {
            styled: io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    fn print(&self, report: &DoctorReport) {
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
        println!();
        println!("overall: {}", report.overall_status.as_str());
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
        let value = row
            .skipped_reason
            .as_ref()
            .map(|reason| format!("skipped: {reason}"))
            .unwrap_or_else(|| row.value.clone());
        if self.styled {
            let label = format!("{:<label_width$}", row.label);
            println!(
                "  {} {}  {}",
                row.status.icon().with(row.status.color()).bold(),
                label.with(row.status.label_color()),
                value.with(Color::Rgb {
                    r: 150,
                    g: 150,
                    b: 150,
                })
            );
        } else {
            println!("  {} {:<18} {value}", row.status.marker(), row.label);
        }
    }
}

impl DoctorStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Info => "info",
            Self::Skipped => "skipped",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }

    fn marker(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warning => "!!",
            Self::Error => "!!",
            Self::Info => "..",
            Self::Skipped => "--",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Warning => "!",
            Self::Error => "✗",
            Self::Info => "·",
            Self::Skipped => "-",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Ok => Color::Green,
            Self::Warning => Color::Yellow,
            Self::Error => Color::Red,
            Self::Info | Self::Skipped => Color::DarkGrey,
        }
    }

    fn label_color(self) -> Color {
        match self {
            Self::Ok | Self::Warning | Self::Error => Color::White,
            Self::Info | Self::Skipped => Color::Grey,
        }
    }
}

struct ConfigBootstrap {
    path: Option<PathBuf>,
    config: Option<AppConfig>,
    loose_db_path: Option<PathBuf>,
    failure: Option<String>,
}

fn inspect_config() -> ConfigBootstrap {
    let path = match app_config::config_file_path() {
        Ok(path) => path,
        Err(_) => {
            return ConfigBootstrap {
                path: None,
                config: None,
                loose_db_path: None,
                failure: Some(
                    "configuration path is unavailable; set AVEN_CONFIG_DIR or HOME".to_string(),
                ),
            };
        }
    };
    if !path.exists() {
        return ConfigBootstrap {
            config: AppConfig::load_from_path(&path).ok(),
            path: Some(path),
            loose_db_path: None,
            failure: None,
        };
    }
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(_) => {
            return ConfigBootstrap {
                path: Some(path),
                config: None,
                loose_db_path: None,
                failure: Some(
                    "configuration file is unreadable; check file ownership and permissions"
                        .to_string(),
                ),
            };
        }
    };
    let loose = serde_yaml::from_str::<Value>(&text).ok();
    let loose_db_path = loose.as_ref().and_then(configured_database_path);
    match AppConfig::load_from_path(&path) {
        Ok(config) => ConfigBootstrap {
            path: Some(path),
            config: Some(config),
            loose_db_path,
            failure: None,
        },
        Err(_) => {
            let location = serde_yaml::from_str::<AppConfig>(&text)
                .err()
                .and_then(|error| error.location())
                .map(|location| {
                    format!(" at line {}, column {}", location.line(), location.column())
                })
                .unwrap_or_default();
            ConfigBootstrap {
                path: Some(path),
                config: None,
                loose_db_path,
                failure: Some(format!(
                    "configuration is invalid{location}; fix the YAML or invalid value, then rerun `aven doctor`"
                )),
            }
        }
    }
}

fn configured_database_path(value: &Value) -> Option<PathBuf> {
    value
        .get("local")?
        .get("db_path")?
        .as_str()
        .map(PathBuf::from)
}

fn resolve_doctor_db_path(
    flag: Option<PathBuf>,
    bootstrap: &ConfigBootstrap,
) -> (Option<PathBuf>, &'static str, Option<String>) {
    if let Some(path) = flag {
        return (Some(path), "--db", None);
    }
    if let Some(path) = app_config::debug_db_path_from_env() {
        return (Some(path), "AVEN_DEV_DB", None);
    }
    if let Some(path) = std::env::var_os("AVEN_DB") {
        return (Some(PathBuf::from(path)), "AVEN_DB", None);
    }
    let configured = bootstrap
        .config
        .as_ref()
        .and_then(|config| config.local.db_path.clone())
        .or_else(|| bootstrap.loose_db_path.clone());
    if let Some(path) = configured {
        return match app_config::expand_tilde(&path) {
            Ok(path) => (Some(path), "config local.db_path", None),
            Err(_) => (
                None,
                "config local.db_path",
                Some("database path could not expand `~`; set HOME or pass --db".to_string()),
            ),
        };
    }
    if cfg!(debug_assertions) {
        return (
            None,
            "unresolved",
            Some("debug builds require --db, AVEN_DEV_DB, AVEN_DB, or local.db_path".to_string()),
        );
    }
    match app_config::default_db_path() {
        Ok(path) => (Some(path), "default", None),
        Err(_) => (
            None,
            "default",
            Some("platform database path is unavailable; set XDG_STATE_HOME or HOME".to_string()),
        ),
    }
}

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

pub(crate) async fn cmd_doctor(
    db_flag: Option<PathBuf>,
    workspace_flag: Option<&str>,
    args: DoctorArgs,
) -> Result<()> {
    let bootstrap = inspect_config();
    let (db_path, db_source, db_resolution_error) = resolve_doctor_db_path(db_flag, &bootstrap);
    let fallback_config = AppConfig::default();
    let config = bootstrap.config.as_ref().unwrap_or(&fallback_config);
    let mut report = DoctorReport::new();

    let config_section = report.section("configuration", "Configuration");
    match (&bootstrap.path, &bootstrap.failure) {
        (Some(path), None) if path.exists() => config_section.check(
            "config.valid",
            "config file",
            true,
            path.display().to_string(),
        ),
        (Some(path), None) => config_section.info(
            "config.defaults",
            "config file",
            format!("{} (using defaults)", path.display()),
        ),
        (Some(path), Some(failure)) => config_section.check(
            "config.invalid",
            "config file",
            false,
            format!("{}: {failure}", path.display()),
        ),
        (None, Some(failure)) => {
            config_section.check("config.path_unavailable", "config file", false, failure)
        }
        (None, None) => config_section.skipped(
            "config.path_unavailable",
            "config file",
            "configuration path is unavailable",
        ),
    }
    config_section.info("database.source", "database source", db_source);
    match (&db_path, db_resolution_error) {
        (Some(path), _) => {
            config_section.info("database.path", "database path", path.display().to_string())
        }
        (None, Some(error)) => {
            config_section.check("database.path_unresolved", "database path", false, error)
        }
        (None, None) => config_section.skipped(
            "database.path_unresolved",
            "database path",
            "no database path was resolved",
        ),
    }

    let inspected = match db_path.as_deref() {
        Some(path) => Some(aven_core::db::Database::inspect(path).await),
        None => None,
    };
    let database_section = report.section("database", "Database");
    if let Some(inspected) = &inspected {
        let inspection = &inspected.inspection;
        database_section.check(
            "database.exists",
            "exists",
            inspection.exists,
            if inspection.exists {
                "database file exists"
            } else {
                "database file is missing; restore a backup or verify the selected path"
            },
        );
        if inspection.exists {
            database_section.check(
                "database.file_type",
                "file type",
                inspection.is_file,
                if inspection.is_file {
                    "regular file"
                } else {
                    "path is not a regular file"
                },
            );
            if inspection.is_file {
                database_section.check(
                    "database.sqlite_header",
                    "SQLite header",
                    inspection.header_is_sqlite,
                    if inspection.header_is_sqlite {
                        "SQLite format 3"
                    } else {
                        "invalid or unreadable; restore a known-good SQLite backup"
                    },
                );
            } else {
                database_section.skipped(
                    "database.sqlite_header_skipped",
                    "SQLite header",
                    "database path is not a regular file",
                );
            }
            database_section.info(
                "database.permissions",
                "permissions",
                if inspection.read_only_permissions {
                    "filesystem marks the database read-only"
                } else {
                    "filesystem mode permits writes"
                },
            );
            match &inspection.open_error {
                Some(error) => database_section.check(
                    "database.open",
                    "sqlite",
                    false,
                    format!("{error}; check path permissions or restore a backup"),
                ),
                None => database_section.check(
                    "database.open",
                    "sqlite",
                    true,
                    "opened read-only without migrations",
                ),
            }
        } else {
            database_section.skipped(
                "database.file_type_skipped",
                "file type",
                "database file does not exist",
            );
            database_section.skipped(
                "database.sqlite_header_skipped",
                "SQLite header",
                "database file does not exist",
            );
            database_section.skipped(
                "database.open_skipped",
                "sqlite",
                "database file does not exist",
            );
        }
        database_section.info(
            "database.sidecars",
            "sidecars",
            format!(
                "wal={} shm={}",
                inspection.wal_exists, inspection.shm_exists
            ),
        );
        if let Some(version) = inspection.schema_version {
            database_section.info(
                "database.schema_version",
                "schema version",
                match inspection.latest_schema_version {
                    Some(latest) => format!("current={version} supported={latest}"),
                    None => format!("current={version} supported=unknown"),
                },
            );
            if inspection.unsupported_future_schema {
                database_section.check(
                    "database.schema_future",
                    "schema support",
                    false,
                    format!(
                        "version {version} is newer than this Aven supports; upgrade Aven or use a compatible backup"
                    ),
                );
            } else if !inspection.pending_migrations.is_empty() {
                database_section.warning(
                    "database.migrations_pending",
                    "migrations",
                    format!(
                        "{} pending; back up the database, then run a normal Aven command to migrate",
                        inspection.pending_migrations.len()
                    ),
                );
            } else {
                database_section.check(
                    "database.schema_supported",
                    "schema support",
                    true,
                    "schema is current",
                );
            }
        } else {
            database_section.skipped(
                "database.schema_skipped",
                "schema version",
                "SQLite could not be opened safely",
            );
        }
        if let Some(version) = inspection.failed_migration {
            database_section.check(
                "database.migration_failed",
                "migration state",
                false,
                format!(
                    "migration {version} is marked failed; preserve the database and restore a pre-migration backup"
                ),
            );
        }
    } else {
        database_section.skipped(
            "database.inspection_skipped",
            "inspection",
            "database path resolution failed",
        );
    }

    let database = inspected.as_ref().and_then(|value| value.database.as_ref());
    add_runtime_database_sections(
        &mut report,
        database,
        config,
        bootstrap.config.is_some(),
        db_path.as_deref(),
        workspace_flag,
        args.integrity,
    )
    .await;
    add_daemon_section(&mut report, config);

    report.finish();
    let has_errors = report.has_errors();
    if args.json {
        print_json_pretty(&report)?;
    } else {
        DoctorRenderer::auto().print(&report);
    }
    if args.fail_on_error && has_errors {
        bail!("doctor found error-level findings");
    }
    Ok(())
}

async fn add_runtime_database_sections(
    report: &mut DoctorReport,
    database: Option<&aven_core::db::Database>,
    config: &AppConfig,
    config_valid: bool,
    db_path: Option<&Path>,
    workspace_flag: Option<&str>,
    integrity: bool,
) {
    let reason = "database schema is unavailable, pending migration, or unsupported";
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
        match app_config::resolve_sync_server(None, config) {
            Ok(server) => {
                sync_section.check(
                    "sync.server",
                    "server",
                    sync_server_url_is_valid(&server),
                    &server,
                );
                if let Some(database) = database
                    && let Ok(Some(pinned)) = database.meta("sync_server_url").await
                {
                    let normalized = server.trim_end_matches('/');
                    sync_section.check(
                        "sync.server_match",
                        "server match",
                        pinned == normalized,
                        format!("pinned={pinned} configured={normalized}"),
                    );
                }
            }
            Err(_) if config.sync.enabled => sync_section.check(
                "sync.server",
                "server",
                false,
                "sync-server-required; configure sync.server_url",
            ),
            Err(_) => sync_section.info("sync.server", "server", "not configured"),
        }
        match config.sync.server_url.as_deref() {
            Some(server) => sync_section.check(
                "sync.daemon_server",
                "daemon server",
                sync_server_url_is_valid(server),
                server,
            ),
            None if config.sync.enabled => sync_section.check(
                "sync.daemon_server",
                "daemon server",
                false,
                "not configured",
            ),
            None => sync_section.info("sync.daemon_server", "daemon server", "not configured"),
        }
        sync_section.info(
            "sync.auth_token",
            "auth token",
            if config.sync_auth_token().is_some() {
                "configured"
            } else {
                "not configured"
            },
        );
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
            ("sync.server_skipped", "server"),
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
            (
                "database.pinned_server",
                "pinned server",
                "sync_server_url",
                "none",
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
                match attachment_integrity_checks(database, &blob_dir, integrity).await {
                    Ok(checks) if !integrity => {
                        for check in checks {
                            attachment_section.check(
                                stable_check_code("attachments", check.label),
                                check.label,
                                check.ok,
                                check.value,
                            );
                        }
                    }
                    Ok(_) => attachment_section.info(
                        "attachments.integrity_deferred",
                        "integrity",
                        "reported in the Integrity section",
                    ),
                    Err(_) => attachment_section.check(
                        "attachments.integrity",
                        "integrity",
                        false,
                        "could not inspect attachments",
                    ),
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
            (Some(database), Some(db_path)) => match database_integrity_report(database).await {
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
                    let mut combined = integrity_report.clone();
                    if let Ok(blob_dir) = app_config::resolve_blob_dir(db_path, config)
                        && let Ok(checks) = attachment_integrity_checks(database, &blob_dir, true).await
                    {
                        for check in &checks {
                            integrity_section.check(
                                stable_check_code("integrity", check.label),
                                check.label,
                                check.ok,
                                &check.value,
                            );
                        }
                        combined.checks.extend(checks);
                    }
                    if let Err(error) = ensure_integrity_ok(&combined) {
                        integrity_section.check("integrity.result", "result", false, format!("{error:#}"));
                    }
                }
                Err(_) => integrity_section.check(
                    "integrity.failed",
                    "result",
                    false,
                    "integrity checks could not complete; preserve the database and restore a known-good backup",
                ),
            },
            _ => integrity_section.skipped("integrity.skipped", "result", reason),
        }
    }
}

fn add_daemon_section(report: &mut DoctorReport, config: &AppConfig) {
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
