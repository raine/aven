mod bootstrap;
mod checks;
mod render;
mod report;

use std::path::PathBuf;

use anyhow::{Result, bail};

use crate::cli::DoctorArgs;
use crate::config::AppConfig;
use crate::render::print_json_pretty;

use bootstrap::{inspect_config, resolve_doctor_db_path};
use checks::{add_ai_section, add_daemon_section, add_runtime_database_sections};
pub(super) use render::DoctorRenderer;
pub(super) use report::{DoctorReport, DoctorRow, DoctorSection, DoctorStatus};

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
    add_ai_section(&mut report, config, bootstrap.config.is_some(), database).await;
    add_daemon_section(&mut report, config, database).await;

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
