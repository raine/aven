use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use aven_core::data_safety::{AvenExport, IntegrityReport};
use aven_core::db::Database;

use crate::cli::{BackupCommand, BackupRestoreArgs, ExportArgs, ImportArgs};
use crate::config::{self as app_config, AppConfig};
use crate::db;
use crate::ids::now;
use crate::render::quote;

pub(crate) async fn cmd_backup(
    database: &Database,
    config: &AppConfig,
    db_path: &Path,
    args: BackupCommand,
) -> Result<()> {
    let output = match args.output {
        Some(path) => path,
        None => db::default_backup_path(db_path, "manual")?,
    };
    let blob_dir = app_config::resolve_blob_dir(db_path, config)?;
    database
        .create_backup_archive(db_path, &blob_dir, &output)
        .await?;
    let bytes = fs::metadata(&output)
        .with_context(|| format!("could not stat {}", output.display()))?
        .len();
    println!(
        "backup path={} bytes={bytes}",
        quote(&output.display().to_string())
    );
    Ok(())
}

pub(crate) async fn cmd_backup_restore(
    config: &AppConfig,
    db_path: &Path,
    args: BackupRestoreArgs,
) -> Result<()> {
    if !args.yes {
        bail!(
            "error backup-restore-requires-confirmation hint=\"pass --yes to replace local data\""
        );
    }
    let safety = if aven_core::data_safety::is_backup_archive(&args.path)? {
        let blob_dir = app_config::resolve_blob_dir(db_path, config)?;
        aven_core::data_safety::restore_backup_archive(db_path, &blob_dir, &args.path).await?
    } else {
        db::restore_database_file(db_path, &args.path).await?
    };
    println!(
        "restored-backup path={} safety_backup={}",
        quote(&args.path.display().to_string()),
        quote(&safety.display().to_string())
    );
    Ok(())
}

pub(crate) async fn cmd_export(database: &Database, args: ExportArgs) -> Result<()> {
    let export = database.export_data(now()).await?;
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let text = serde_json::to_string(&export).context("could not serialize export")?;
    fs::write(&args.output, text)
        .with_context(|| format!("could not write export file {}", args.output.display()))?;
    let bytes = fs::metadata(&args.output)
        .with_context(|| format!("could not stat {}", args.output.display()))?
        .len();
    println!(
        "exported path={} workspaces={} tasks={} bytes={bytes}",
        quote(&args.output.display().to_string()),
        export.tables.workspaces.len(),
        export.tables.tasks.len()
    );
    Ok(())
}

pub(crate) async fn cmd_import(
    database: &Database,
    db_path: &Path,
    args: ImportArgs,
) -> Result<()> {
    if !args.yes {
        bail!("error import-requires-confirmation hint=\"pass --yes to replace local data\"");
    }
    let text = fs::read_to_string(&args.path)
        .with_context(|| format!("could not read {}", args.path.display()))?;
    let export: AvenExport = serde_json::from_str(&text)
        .with_context(|| format!("could not parse {}", args.path.display()))?;
    database.validate_import_data(&export).await?;
    if export.blobs_included {
        bail!("error import-blobs-included-unsupported");
    }
    let safety = db::default_sqlite_backup_path(db_path, "before-import")?;
    db::backup_database(db_path, &safety)?;
    database.import_data(&export).await?;
    println!(
        "imported path={} safety_backup={} workspaces={} tasks={}",
        quote(&args.path.display().to_string()),
        quote(&safety.display().to_string()),
        export.tables.workspaces.len(),
        export.tables.tasks.len()
    );
    Ok(())
}

pub(crate) async fn database_integrity_report(database: &Database) -> Result<IntegrityReport> {
    database.database_integrity_report().await
}

pub(crate) async fn attachment_integrity_checks(
    database: &Database,
    blob_dir: &Path,
    deep: bool,
) -> Result<Vec<aven_core::data_safety::IntegrityCheck>> {
    database.attachment_integrity_checks(blob_dir, deep).await
}

pub(crate) fn ensure_integrity_ok(report: &IntegrityReport) -> Result<()> {
    let mut bad = vec![];
    if !report.quick_check_ok {
        bad.push("quick check");
    }
    for check in &report.checks {
        if !check.ok {
            bad.push(check.label);
        }
    }
    if bad.is_empty() {
        return Ok(());
    }
    bail!("error data-integrity-failed checks={}", bad.join(", "))
}
