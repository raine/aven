use crate::ids::{ProjectId, TaskId, WorkspaceId};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sqlx::{SqliteConnection, query_scalar};
use std::collections::{HashMap, HashSet};

mod tables;
use crate::db::{self, Database};

#[derive(Debug, Clone)]
pub struct IntegrityReport {
    pub quick_check_ok: bool,
    pub quick_check_value: String,
    pub checks: Vec<IntegrityCheck>,
}

#[derive(Debug, Clone)]
pub struct IntegrityCheck {
    pub label: &'static str,
    pub ok: bool,
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AvenExport {
    pub format: String,
    pub version: i64,
    pub exported_at: String,
    pub schema_version: i64,
    pub tables: ExportTables,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ExportTables {
    pub workspaces: Vec<WorkspaceRow>,
    pub projects: Vec<ProjectRow>,
    pub project_paths: Vec<ProjectPathRow>,
    pub project_id_aliases: Vec<ProjectIdAliasRow>,
    pub labels: Vec<LabelRow>,
    pub tasks: Vec<TaskRow>,
    pub task_labels: Vec<TaskLabelRow>,
    pub notes: Vec<NoteRow>,
    pub task_dependencies: Vec<TaskDependencyRow>,
    pub task_epic_links: Vec<TaskEpicLinkRow>,
    pub changes: Vec<ChangeRow>,
    pub field_versions: Vec<FieldVersionRow>,
    pub conflicts: Vec<ConflictRow>,
    pub meta: Vec<MetaRow>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct WorkspaceRow {
    pub id: WorkspaceId,
    pub name: String,
    pub key: String,
    pub created_at: String,
    pub updated_at: String,
    pub archived: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProjectRow {
    pub id: ProjectId,
    pub workspace_id: WorkspaceId,
    pub key: String,
    pub name: String,
    pub prefix: String,
    pub created_at: String,
    pub updated_at: String,
    pub deleted: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProjectPathRow {
    pub workspace_id: WorkspaceId,
    pub project_id: ProjectId,
    pub path: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ProjectIdAliasRow {
    pub workspace_id: WorkspaceId,
    pub remote_project_id: ProjectId,
    pub local_project_id: ProjectId,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct LabelRow {
    pub workspace_id: WorkspaceId,
    pub name: String,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskRow {
    pub workspace_id: WorkspaceId,
    pub id: TaskId,
    pub title: String,
    pub description: String,
    pub project_id: ProjectId,
    pub status: String,
    pub priority: String,
    pub created_at: String,
    pub updated_at: String,
    pub queue_activity_at: String,
    #[serde(default)]
    pub available_at: String,
    #[serde(default)]
    pub due_on: String,
    pub deleted: i64,
    pub is_epic: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskEpicLinkRow {
    pub workspace_id: WorkspaceId,
    pub child_task_id: TaskId,
    pub epic_task_id: TaskId,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskLabelRow {
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub label: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct NoteRow {
    pub workspace_id: WorkspaceId,
    pub id: String,
    pub task_id: TaskId,
    pub body: String,
    pub created_at: String,
    pub change_id: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskDependencyRow {
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub depends_on_task_id: TaskId,
    pub created_at: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ChangeRow {
    pub change_id: String,
    pub client_id: String,
    pub local_seq: i64,
    pub entity_type: String,
    pub entity_id: String,
    pub field: Option<String>,
    pub op_type: String,
    pub payload: String,
    pub base_version: Option<String>,
    pub created_at: String,
    pub server_seq: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct FieldVersionRow {
    pub entity_id: String,
    pub field: String,
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct ConflictRow {
    pub id: i64,
    pub workspace_id: WorkspaceId,
    pub task_id: TaskId,
    pub field: String,
    pub base_version: Option<String>,
    pub local_value: String,
    pub remote_value: String,
    pub local_change_id: Option<String>,
    pub remote_change_id: String,
    pub variant_a: String,
    pub variant_b: String,
    pub created_at: String,
    pub resolved: i64,
}

#[derive(Debug, Serialize, Deserialize, sqlx::FromRow)]
pub struct MetaRow {
    pub key: String,
    pub value: String,
}

impl Database {
    pub async fn export_data(&self, exported_at: String) -> Result<AvenExport> {
        let mut conn = self.acquire().await?;
        let schema_version = db::current_schema_version(&mut conn).await?;
        Ok(AvenExport {
            format: "aven-export".to_string(),
            version: 1,
            exported_at,
            schema_version,
            tables: ExportTables {
                workspaces: scan_workspaces(&mut conn).await?,
                projects: scan_projects(&mut conn).await?,
                project_paths: scan_project_paths(&mut conn).await?,
                project_id_aliases: scan_project_id_aliases(&mut conn).await?,
                labels: scan_labels(&mut conn).await?,
                tasks: scan_tasks(&mut conn).await?,
                task_labels: scan_task_labels(&mut conn).await?,
                notes: scan_notes(&mut conn).await?,
                task_dependencies: scan_task_dependencies(&mut conn).await?,
                task_epic_links: scan_task_epic_links(&mut conn).await?,
                changes: scan_changes(&mut conn).await?,
                field_versions: scan_field_versions(&mut conn).await?,
                conflicts: scan_conflicts(&mut conn).await?,
                meta: scan_meta(&mut conn).await?,
            },
        })
    }

    pub async fn validate_import_data(&self, export: &AvenExport) -> Result<()> {
        let mut conn = self.acquire().await?;
        ensure_supported_export(&mut conn, export).await?;
        validate_export_snapshot(export)
    }

    pub async fn import_data(&self, export: &AvenExport) -> Result<IntegrityReport> {
        let mut conn = self.acquire().await?;
        ensure_supported_export(&mut conn, export).await?;
        validate_export_snapshot(export)?;
        let target_client_id = db::get_meta(&mut conn, "client_id")
            .await?
            .context("missing target client_id")?;
        let mut tx = db::begin_immediate(&mut conn).await?;
        replace_from_export(&mut tx, export, &target_client_id).await?;
        let report = database_integrity_report_with_connection(&mut tx).await?;
        ensure_integrity_ok(&report)?;
        tx.commit().await?;
        Ok(report)
    }

    pub async fn database_integrity_report(&self) -> Result<IntegrityReport> {
        let mut conn = self.acquire().await?;
        database_integrity_report_with_connection(&mut conn).await
    }
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
    tables::scan_rows(conn, "SELECT workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at, available_at, due_on, deleted, is_epic FROM tasks").await
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

async fn scan_task_epic_links(conn: &mut SqliteConnection) -> Result<Vec<TaskEpicLinkRow>> {
    tables::scan_rows(
        conn,
        "SELECT workspace_id, child_task_id, epic_task_id, created_at FROM task_epic_links",
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
        if workspace_ids.contains(&workspace.id) {
            continue;
        }
        workspace_ids.insert(workspace.id.clone());
    }

    let mut project_ids: HashMap<WorkspaceId, HashSet<ProjectId>> = HashMap::new();
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

    let mut label_keys: HashSet<(WorkspaceId, String)> = HashSet::new();
    for label in &export.tables.labels {
        if !workspace_ids.contains(&label.workspace_id) {
            bail!(
                "error invalid-export-snapshot label.workspace_id={} is missing",
                label.workspace_id
            );
        }
        label_keys.insert((label.workspace_id.clone(), label.name.clone()));
    }

    let mut task_ids: HashMap<WorkspaceId, HashSet<TaskId>> = HashMap::new();
    for task in &export.tables.tasks {
        if let Err(error) = crate::time_validation::validate_due_on_value(&task.due_on) {
            bail!(
                "error invalid-export-snapshot task.due_on={} is invalid: {error}",
                task.due_on
            );
        }
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

    for epic_link in &export.tables.task_epic_links {
        let tasks = task_ids.get(&epic_link.workspace_id).ok_or_else(|| {
            anyhow::Error::msg(format!(
                "error invalid-export-snapshot epic_link.workspace_id={} is missing",
                epic_link.workspace_id
            ))
        })?;
        if !tasks.contains(&epic_link.child_task_id) || !tasks.contains(&epic_link.epic_task_id) {
            bail!(
                "error invalid-export-snapshot task_epic_links are missing tasks in workspace {}",
                epic_link.workspace_id
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
    }

    Ok(())
}

async fn replace_from_export(
    tx: &mut SqliteConnection,
    export: &AvenExport,
    target_client_id: &str,
) -> Result<()> {
    let delete_order = [
        "DELETE FROM task_epic_links",
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
    tables::import_task_epic_links(tx, &export.tables.task_epic_links).await?;
    tables::import_changes(tx, &export.tables.changes).await?;
    tables::import_field_versions(tx, &export.tables.field_versions).await?;
    tables::import_conflicts(tx, &export.tables.conflicts).await?;

    Ok(())
}

async fn database_integrity_report_with_connection(
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
        "epic link children",
        "SELECT count(*) FROM task_epic_links l LEFT JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.child_task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "epic link parents",
        "SELECT count(*) FROM task_epic_links l LEFT JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.epic_task_id WHERE t.id IS NULL",
    )
    .await?);
    checks.push(count_check(
        conn,
        "epic link parent flags",
        "SELECT count(*) FROM task_epic_links l JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.epic_task_id WHERE t.is_epic = 0",
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
        "task due dates",
        "SELECT count(*) FROM tasks WHERE due_on != '' AND (length(due_on) != 10 OR substr(due_on, 5, 1) != '-' OR substr(due_on, 8, 1) != '-' OR date(due_on) IS NULL OR strftime('%Y-%m-%d', due_on) != due_on)",
    )
    .await?);
    checks.push(count_check(
        conn,
        "field version tasks",
        "SELECT count(*) FROM field_versions fv LEFT JOIN tasks t ON t.id = fv.entity_id WHERE t.id IS NULL AND fv.field IN ('title','description','status','priority','project','labels','available_at','due_on','deleted','is_epic')",
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
