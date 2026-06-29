use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sqlx::{SqliteConnection, query_scalar};

use crate::cli::{BackupCommand, BackupRestoreArgs, ExportArgs, ImportArgs};

mod tables;
use crate::db;
use crate::ids::now;
use crate::render::quote;

#[derive(Debug, Clone)]
pub(crate) struct IntegrityReport {
    pub(crate) quick_check_ok: bool,
    pub(crate) quick_check_value: String,
    pub(crate) checks: Vec<IntegrityCheck>,
}

#[derive(Debug, Clone)]
pub(crate) struct IntegrityCheck {
    pub(crate) label: &'static str,
    pub(crate) ok: bool,
    pub(crate) value: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AvenExport {
    format: String,
    version: i64,
    exported_at: String,
    schema_version: i64,
    tables: ExportTables,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExportTables {
    workspaces: Vec<WorkspaceRow>,
    projects: Vec<ProjectRow>,
    project_paths: Vec<ProjectPathRow>,
    project_id_aliases: Vec<ProjectIdAliasRow>,
    labels: Vec<LabelRow>,
    tasks: Vec<TaskRow>,
    task_labels: Vec<TaskLabelRow>,
    notes: Vec<NoteRow>,
    task_dependencies: Vec<TaskDependencyRow>,
    changes: Vec<ChangeRow>,
    field_versions: Vec<FieldVersionRow>,
    conflicts: Vec<ConflictRow>,
    meta: Vec<MetaRow>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct WorkspaceRow {
    id: String,
    name: String,
    key: String,
    created_at: String,
    updated_at: String,
    archived: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct ProjectRow {
    id: String,
    workspace_id: String,
    key: String,
    name: String,
    prefix: String,
    created_at: String,
    updated_at: String,
    deleted: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct ProjectPathRow {
    workspace_id: String,
    project_id: String,
    path: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct ProjectIdAliasRow {
    workspace_id: String,
    remote_project_id: String,
    local_project_id: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct LabelRow {
    workspace_id: String,
    name: String,
    created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct TaskRow {
    workspace_id: String,
    id: String,
    title: String,
    description: String,
    project_id: String,
    status: String,
    priority: String,
    created_at: String,
    updated_at: String,
    queue_activity_at: String,
    deleted: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct TaskLabelRow {
    workspace_id: String,
    task_id: String,
    label: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct NoteRow {
    workspace_id: String,
    id: String,
    task_id: String,
    body: String,
    created_at: String,
    change_id: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct TaskDependencyRow {
    workspace_id: String,
    task_id: String,
    depends_on_task_id: String,
    created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct ChangeRow {
    change_id: String,
    client_id: String,
    local_seq: i64,
    entity_type: String,
    entity_id: String,
    field: Option<String>,
    op_type: String,
    payload: String,
    base_version: Option<String>,
    created_at: String,
    server_seq: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct FieldVersionRow {
    entity_id: String,
    field: String,
    version: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct ConflictRow {
    id: i64,
    workspace_id: String,
    task_id: String,
    field: String,
    base_version: Option<String>,
    local_value: String,
    remote_value: String,
    local_change_id: Option<String>,
    remote_change_id: String,
    variant_a: String,
    variant_b: String,
    created_at: String,
    resolved: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
struct MetaRow {
    key: String,
    value: String,
}

pub(crate) async fn cmd_backup(
    _conn: &mut SqliteConnection,
    db_path: &Path,
    args: BackupCommand,
) -> Result<()> {
    let output = match args.output {
        Some(path) => path,
        None => db::default_backup_path(db_path, "manual")?,
    };
    db::backup_database(db_path, &output)?;
    let bytes = fs::metadata(&output)
        .with_context(|| format!("could not stat {}", output.display()))?
        .len();
    println!(
        "backup path={} bytes={bytes}",
        quote(&output.display().to_string())
    );
    Ok(())
}

pub(crate) async fn cmd_backup_restore(db_path: &Path, args: BackupRestoreArgs) -> Result<()> {
    if !args.yes {
        bail!(
            "error backup-restore-requires-confirmation hint=\"pass --yes to replace local data\""
        );
    }
    let safety = db::restore_database_file(db_path, &args.path).await?;
    println!(
        "restored-backup path={} safety_backup={}",
        quote(&args.path.display().to_string()),
        quote(&safety.display().to_string())
    );
    Ok(())
}

pub(crate) async fn cmd_export(conn: &mut SqliteConnection, args: ExportArgs) -> Result<()> {
    let schema_version = db::current_schema_version(conn).await?;
    let tables = ExportTables {
        workspaces: scan_workspaces(conn).await?,
        projects: scan_projects(conn).await?,
        project_paths: scan_project_paths(conn).await?,
        project_id_aliases: scan_project_id_aliases(conn).await?,
        labels: scan_labels(conn).await?,
        tasks: scan_tasks(conn).await?,
        task_labels: scan_task_labels(conn).await?,
        notes: scan_notes(conn).await?,
        task_dependencies: scan_task_dependencies(conn).await?,
        changes: scan_changes(conn).await?,
        field_versions: scan_field_versions(conn).await?,
        conflicts: scan_conflicts(conn).await?,
        meta: scan_meta(conn).await?,
    };
    if let Some(parent) = args.output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let export = AvenExport {
        format: "aven-export".to_string(),
        version: 1,
        exported_at: now(),
        schema_version,
        tables,
    };
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
    conn: &mut SqliteConnection,
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
    ensure_supported_export(conn, &export).await?;
    validate_export_snapshot(&export)?;
    let target_client_id = db::get_meta(conn, "client_id")
        .await?
        .context("missing target client_id")?;
    let safety = db::default_backup_path(db_path, "before-import")?;
    db::backup_database(db_path, &safety)?;
    let mut tx = db::begin_immediate(conn).await?;
    replace_from_export(&mut tx, &export, &target_client_id).await?;
    let report = database_integrity_report(&mut tx).await?;
    ensure_integrity_ok(&report)?;
    tx.commit().await?;
    println!(
        "imported path={} safety_backup={} workspaces={} tasks={}",
        quote(&args.path.display().to_string()),
        quote(&safety.display().to_string()),
        export.tables.workspaces.len(),
        export.tables.tasks.len()
    );
    Ok(())
}

async fn scan_workspaces(conn: &mut SqliteConnection) -> Result<Vec<WorkspaceRow>> {
    tables::scan_rows(
        conn,
        "SELECT id, name, key, created_at, updated_at, archived FROM workspaces",
    )
    .await
}

async fn scan_projects(conn: &mut SqliteConnection) -> Result<Vec<ProjectRow>> {
    tables::scan_rows(
        conn,
        "SELECT id, workspace_id, key, name, prefix, created_at, updated_at, deleted FROM projects",
    )
    .await
}

async fn scan_project_paths(conn: &mut SqliteConnection) -> Result<Vec<ProjectPathRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, project_id, path FROM project_paths",
    )
    .await
}

async fn scan_project_id_aliases(conn: &mut SqliteConnection) -> Result<Vec<ProjectIdAliasRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, remote_project_id, local_project_id FROM project_id_aliases",
    )
    .await
}

async fn scan_labels(conn: &mut SqliteConnection) -> Result<Vec<LabelRow>> {
    tables::scan_rows(conn, "SELECT workspace_id, name, created_at FROM labels").await
}

async fn scan_tasks(conn: &mut SqliteConnection) -> Result<Vec<TaskRow>> {
    tables::scan_rows(conn, "SELECT workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at, deleted FROM tasks").await
}

async fn scan_task_labels(conn: &mut SqliteConnection) -> Result<Vec<TaskLabelRow>> {
    tables::scan_rows(conn, "SELECT workspace_id, task_id, label FROM task_labels").await
}

async fn scan_notes(conn: &mut SqliteConnection) -> Result<Vec<NoteRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, id, task_id, body, created_at, change_id FROM notes",
    )
    .await
}

async fn scan_task_dependencies(conn: &mut SqliteConnection) -> Result<Vec<TaskDependencyRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, task_id, depends_on_task_id, created_at FROM task_dependencies",
    )
    .await
}

async fn scan_changes(conn: &mut SqliteConnection) -> Result<Vec<ChangeRow>> {
    tables::scan_rows(conn, "SELECT change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq FROM changes").await
}

async fn scan_field_versions(conn: &mut SqliteConnection) -> Result<Vec<FieldVersionRow>> {
    tables::scan_rows(conn, "SELECT entity_id, field, version FROM field_versions").await
}

async fn scan_conflicts(conn: &mut SqliteConnection) -> Result<Vec<ConflictRow>> {
    tables::scan_rows(conn, "SELECT id, workspace_id, task_id, field, base_version, local_value, remote_value, local_change_id, remote_change_id, variant_a, variant_b, created_at, resolved FROM conflicts").await
}

async fn scan_meta(conn: &mut SqliteConnection) -> Result<Vec<MetaRow>> {
    tables::scan_rows(conn, "SELECT key, value FROM meta").await
}

async fn ensure_supported_export(conn: &mut SqliteConnection, export: &AvenExport) -> Result<()> {
    if export.format != "aven-export" {
        bail!("error export-format-unsupported format={}", export.format);
    }
    if export.version != 1 {
        bail!(
            "error export-version-unsupported version={}",
            export.version
        );
    }
    let current = db::current_schema_version(conn).await?;
    if export.schema_version != current {
        bail!(
            "error export-schema-unsupported expected={} actual={}",
            current,
            export.schema_version
        );
    }
    Ok(())
}

fn validate_export_snapshot(export: &AvenExport) -> Result<()> {
    let mut workspace_ids = HashSet::new();
    for workspace in &export.tables.workspaces {
        if workspace.id.is_empty() {
            bail!("error invalid-export-snapshot workspace id is empty");
        }
        if workspace_ids.contains(&workspace.id) {
            continue;
        }
        workspace_ids.insert(workspace.id.clone());
    }

    let mut project_ids: HashMap<String, HashSet<String>> = HashMap::new();
    for project in &export.tables.projects {
        if !workspace_ids.contains(&project.workspace_id) {
            bail!(
                "error invalid-export-snapshot project.workspace_id={} is missing",
                project.workspace_id
            );
        }
        project_ids
            .entry(project.workspace_id.clone())
            .or_default()
            .insert(project.id.clone());
    }

    for path in &export.tables.project_paths {
        let projects = project_ids.get(&path.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot project_path.workspace_id={} is missing",
                path.workspace_id
            ))
        })?;
        if !projects.contains(&path.project_id) {
            bail!(
                "error invalid-export-snapshot project_path.project_id={} is missing in workspace {}",
                path.project_id,
                path.workspace_id
            );
        }
    }

    let mut label_keys: HashSet<(String, String)> = HashSet::new();
    for label in &export.tables.labels {
        if !workspace_ids.contains(&label.workspace_id) {
            bail!(
                "error invalid-export-snapshot label.workspace_id={} is missing",
                label.workspace_id
            );
        }
        label_keys.insert((label.workspace_id.clone(), label.name.clone()));
    }

    let mut task_ids: HashMap<String, HashSet<String>> = HashMap::new();
    for task in &export.tables.tasks {
        let workspace_projects = project_ids.get(&task.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot task.workspace_id={} is missing",
                task.workspace_id
            ))
        })?;
        if !workspace_projects.contains(&task.project_id) {
            bail!(
                "error invalid-export-snapshot task.project_id={} is missing in workspace {}",
                task.project_id,
                task.workspace_id
            );
        }
        task_ids
            .entry(task.workspace_id.clone())
            .or_default()
            .insert(task.id.clone());
    }

    for task_label in &export.tables.task_labels {
        let task_workspace = task_ids.get(&task_label.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot task_label.workspace_id={} is missing",
                task_label.workspace_id
            ))
        })?;
        if !task_workspace.contains(&task_label.task_id) {
            bail!(
                "error invalid-export-snapshot task_label.task_id={} is missing in workspace {}",
                task_label.task_id,
                task_label.workspace_id
            );
        }
        if !label_keys.contains(&(task_label.workspace_id.clone(), task_label.label.clone())) {
            bail!(
                "error invalid-export-snapshot task_label.label={} is missing in workspace {}",
                task_label.label,
                task_label.workspace_id
            );
        }
    }

    for note in &export.tables.notes {
        let task_workspace = task_ids.get(&note.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot note.workspace_id={} is missing",
                note.workspace_id
            ))
        })?;
        if !task_workspace.contains(&note.task_id) {
            bail!(
                "error invalid-export-snapshot note.task_id={} is missing in workspace {}",
                note.task_id,
                note.workspace_id
            );
        }
    }

    for dep in &export.tables.task_dependencies {
        let tasks = task_ids.get(&dep.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot dependency.workspace_id={} is missing",
                dep.workspace_id
            ))
        })?;
        if !tasks.contains(&dep.task_id) || !tasks.contains(&dep.depends_on_task_id) {
            bail!(
                "error invalid-export-snapshot task_dependencies are missing tasks in workspace {}",
                dep.workspace_id
            );
        }
    }

    for alias in &export.tables.project_id_aliases {
        let workspace_projects = project_ids.get(&alias.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot project_alias.workspace_id={} is missing",
                alias.workspace_id
            ))
        })?;
        if !workspace_projects.contains(&alias.local_project_id) {
            bail!(
                "error invalid-export-snapshot local_project_id={} is missing in workspace {}",
                alias.local_project_id,
                alias.workspace_id
            );
        }
        if alias.remote_project_id.is_empty() {
            bail!(
                "error invalid-export-snapshot remote_project_id empty in workspace {}",
                alias.workspace_id
            );
        }
    }

    Ok(())
}

async fn replace_from_export(
    tx: &mut SqliteConnection,
    export: &AvenExport,
    target_client_id: &str,
) -> Result<()> {
    let delete_order = [
        "DELETE FROM task_dependencies",
        "DELETE FROM task_labels",
        "DELETE FROM notes",
        "DELETE FROM conflicts",
        "DELETE FROM field_versions",
        "DELETE FROM changes",
        "DELETE FROM project_paths",
        "DELETE FROM project_id_aliases",
        "DELETE FROM tasks",
        "DELETE FROM labels",
        "DELETE FROM projects",
        "DELETE FROM workspaces",
        "DELETE FROM meta",
    ];
    for sql in delete_order {
        sqlx::query(sql).execute(&mut *tx).await?;
    }

    db::set_meta(tx, "client_id", target_client_id).await?;
    db::set_meta(tx, "sync_cursor", "0").await?;
    let local_seq = export
        .tables
        .changes
        .iter()
        .map(|row| row.local_seq)
        .max()
        .unwrap_or(0);
    db::set_meta(tx, "local_seq", &local_seq.to_string()).await?;

    for meta in &export.tables.meta {
        if matches!(
            meta.key.as_str(),
            "client_id" | "sync_server_url" | "sync_cursor" | "local_seq"
        ) {
            continue;
        }
        db::set_meta(tx, &meta.key, &meta.value).await?;
    }

    tables::import_workspaces(tx, &export.tables.workspaces).await?;
    tables::import_projects(tx, &export.tables.projects).await?;
    tables::import_project_id_aliases(tx, &export.tables.project_id_aliases).await?;
    tables::import_project_paths(tx, &export.tables.project_paths).await?;
    tables::import_labels(tx, &export.tables.labels).await?;
    tables::import_tasks(tx, &export.tables.tasks).await?;
    tables::import_task_labels(tx, &export.tables.task_labels).await?;
    tables::import_notes(tx, &export.tables.notes).await?;
    tables::import_task_dependencies(tx, &export.tables.task_dependencies).await?;
    tables::import_changes(tx, &export.tables.changes).await?;
    tables::import_field_versions(tx, &export.tables.field_versions).await?;
    tables::import_conflicts(tx, &export.tables.conflicts).await?;

    Ok(())
}

pub(crate) async fn database_integrity_report(
    conn: &mut SqliteConnection,
) -> Result<IntegrityReport> {
    let quick_check_value: String = query_scalar("PRAGMA quick_check")
        .fetch_one(&mut *conn)
        .await?;
    let mut checks = Vec::new();
    checks.push(count_check(
        conn,
        "task projects",
        "SELECT count(*) FROM tasks t LEFT JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "project paths",
        "SELECT count(*) FROM project_paths pp LEFT JOIN projects p ON p.workspace_id = pp.workspace_id AND p.id = pp.project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "project aliases",
        "SELECT count(*) FROM project_id_aliases a LEFT JOIN projects p ON p.workspace_id = a.workspace_id AND p.id = a.local_project_id WHERE p.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task label tasks",
        "SELECT count(*) FROM task_labels tl LEFT JOIN tasks t ON t.workspace_id = tl.workspace_id AND t.id = tl.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "task label labels",
        "SELECT count(*) FROM task_labels tl LEFT JOIN labels l ON l.workspace_id = tl.workspace_id AND l.name = tl.label WHERE l.name IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "notes",
        "SELECT count(*) FROM notes n LEFT JOIN tasks t ON t.workspace_id = n.workspace_id AND t.id = n.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "note changes",
        "SELECT count(*) FROM notes n LEFT JOIN changes c ON c.change_id = n.change_id WHERE c.change_id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "dependency tasks",
        "SELECT count(*) FROM task_dependencies d LEFT JOIN tasks t ON t.workspace_id = d.workspace_id AND t.id = d.task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "dependency targets",
        "SELECT count(*) FROM task_dependencies d LEFT JOIN tasks t ON t.workspace_id = d.workspace_id AND t.id = d.depends_on_task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "conflict tasks",
        "SELECT count(*) FROM conflicts c LEFT JOIN tasks t ON t.workspace_id = c.workspace_id AND t.id = c.task_id WHERE c.resolved = 0 AND t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "field version tasks",
        "SELECT count(*) FROM field_versions fv LEFT JOIN tasks t ON t.id = fv.entity_id WHERE t.id IS NULL AND fv.field IN ('title','description','status','priority','project','labels','deleted')",
    )
    .await?);
    checks.push(count_check(
        conn,
        "field version changes",
        "SELECT count(*) FROM field_versions fv LEFT JOIN changes c ON c.change_id = fv.version WHERE c.change_id IS NULL",
    )
    .await?);
    push_meta_checks(conn, &mut checks).await?;

    Ok(IntegrityReport {
        quick_check_ok: quick_check_value == "ok",
        quick_check_value,
        checks,
    })
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

async fn count_check(
    conn: &mut SqliteConnection,
    label: &'static str,
    query: &'static str,
) -> Result<IntegrityCheck> {
    let count: i64 = query_scalar(query).fetch_one(&mut *conn).await?;
    Ok(IntegrityCheck {
        label,
        ok: count == 0,
        value: format!("{count} orphaned"),
    })
}

async fn push_meta_checks(
    conn: &mut SqliteConnection,
    checks: &mut Vec<IntegrityCheck>,
) -> Result<()> {
    let local_seq = db::get_meta(conn, "local_seq").await?;
    let local_seq_check = match local_seq {
        Some(raw) => match raw.parse::<i64>() {
            Ok(value) => {
                let max_seq: i64 = query_scalar("SELECT COALESCE(MAX(local_seq), 0) FROM changes")
                    .fetch_one(&mut *conn)
                    .await?;
                let ok = value >= max_seq;
                IntegrityCheck {
                    label: "meta local_seq",
                    ok,
                    value: value.to_string(),
                }
            }
            Err(error) => IntegrityCheck {
                label: "meta local_seq",
                ok: false,
                value: error.to_string(),
            },
        },
        None => IntegrityCheck {
            label: "meta local_seq",
            ok: false,
            value: "missing".to_string(),
        },
    };
    checks.push(local_seq_check);

    let sync_cursor = db::get_meta(conn, "sync_cursor").await?;
    let sync_cursor_ok = match sync_cursor {
        Some(raw) => match raw.parse::<i64>() {
            Ok(_) => IntegrityCheck {
                label: "sync cursor",
                ok: true,
                value: raw,
            },
            Err(error) => IntegrityCheck {
                label: "sync cursor",
                ok: false,
                value: error.to_string(),
            },
        },
        None => IntegrityCheck {
            label: "sync cursor",
            ok: false,
            value: "missing".to_string(),
        },
    };
    checks.push(sync_cursor_ok);

    Ok(())
}
