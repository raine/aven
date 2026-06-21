use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;
use sqlx::{Connection as _, Row, SqliteConnection};
use tracing::info;

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::config;
use crate::db::{insert_change, set_field_version};
use crate::ids::{new_id, now};
use crate::labels::{normalize_label, resolve_labels_in_workspace};
use crate::mutation::{apply_field_value_in_workspace, set_task_field};
use crate::projects::{
    create_project_in_workspace, resolve_existing_project_in_workspace,
    resolve_project_for_add_in_workspace,
};
use crate::refs::get_task;
use crate::task_fields::TaskField;
use crate::types::{Project, Task};

pub(crate) struct TaskDraft {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) project: Option<String>,
    pub(crate) priority: String,
    pub(crate) labels: Vec<String>,
}

pub(crate) struct TaskOutcome {
    pub(crate) task: Task,
    pub(crate) create_change_id: Option<String>,
}

#[derive(Default)]
pub(crate) struct TaskUpdate {
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) add_labels: Vec<String>,
    pub(crate) remove_labels: Vec<String>,
}

pub(crate) struct TaskUpdateOutcome {
    pub(crate) task: Task,
    pub(crate) changed: bool,
}

pub(crate) struct NoteOutcome {
    #[allow(dead_code)]
    pub(crate) task_id: String,
    pub(crate) note_id: String,
}

pub(crate) struct LabelOutcome {
    pub(crate) name: String,
    pub(crate) created: bool,
    pub(crate) change_id: Option<String>,
}

pub(crate) struct ProjectOutcome {
    pub(crate) project: Project,
    pub(crate) created: bool,
    pub(crate) change_id: Option<String>,
}

pub(crate) struct ProjectPathOutcome {
    pub(crate) project: Project,
    pub(crate) path: String,
    pub(crate) config_path: PathBuf,
}

struct ProjectPathTarget {
    project: Project,
    path: PathBuf,
}

pub(crate) struct ConflictListItem {
    pub(crate) task_id: String,
    pub(crate) title: String,
    #[allow(dead_code)]
    pub(crate) project_key: String,
    pub(crate) project_prefix: String,
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) variant_b: String,
}

pub(crate) struct ConflictDetail {
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) local_value: String,
    pub(crate) variant_b: String,
    pub(crate) remote_value: String,
}

pub(crate) struct ConflictOutcome {
    pub(crate) task: Task,
    pub(crate) field: String,
}

pub(crate) struct ConfigShowOutcome {
    pub(crate) path: PathBuf,
    pub(crate) text: String,
}

pub(crate) struct ConfigInitOutcome {
    pub(crate) path: PathBuf,
}

pub(crate) struct ConfigStatusOutcome {
    pub(crate) lines: Vec<String>,
}

pub(crate) struct ConfigPathsOutcome {
    pub(crate) lines: Vec<String>,
}

pub(crate) async fn create_task(
    conn: &mut SqliteConnection,
    draft: TaskDraft,
) -> Result<TaskOutcome> {
    create_task_in_workspace(
        conn,
        crate::workspaces::active_workspace_id().as_str(),
        draft,
    )
    .await
}

pub(crate) async fn create_task_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    draft: TaskDraft,
) -> Result<TaskOutcome> {
    validate_choice("priority", &draft.priority, PRIORITIES)?;
    let id = new_id();
    let ts = now();
    let mut tx = conn.begin().await?;
    let project =
        resolve_project_for_add_in_workspace(&mut tx, workspace_id, draft.project.as_deref())
            .await?;
    let labels = resolve_labels_in_workspace(&mut tx, workspace_id, &draft.labels).await?;
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 'inbox', ?, ?, ?)",
    )
    .bind(workspace_id)
    .bind(&id)
    .bind(&draft.title)
    .bind(&draft.description)
    .bind(&project.key)
    .bind(&draft.priority)
    .bind(&ts)
    .bind(&ts)
    .execute(&mut *tx)
    .await?;
    for label in &labels {
        sqlx::query(
            "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
        )
        .bind(workspace_id)
        .bind(&id)
        .bind(label)
        .execute(&mut *tx)
        .await?;
    }
    let change_id = insert_change(
        &mut tx,
        "task",
        &id,
        None,
        "create_task",
        json!({
            "workspace_id": workspace_id,
            "workspace_key": crate::workspaces::active_workspace().key,
            "title": draft.title,
            "description": draft.description,
            "project_key": project.key,
            "project_name": project.name,
            "project_prefix": project.prefix,
            "status": "inbox",
            "priority": draft.priority,
            "labels": labels,
            "created_at": ts,
        }),
        None,
    )
    .await?;
    for field in TaskField::VERSIONED {
        set_field_version(&mut tx, &id, field.as_str(), &change_id).await?;
    }
    tx.commit().await?;
    info!(
        task_id = %id,
        project_key = %project.key,
        label_count = labels.len(),
        "task created"
    );
    Ok(TaskOutcome {
        task: get_task(conn, &id).await?,
        create_change_id: Some(change_id),
    })
}

pub(crate) async fn update_task(
    conn: &mut SqliteConnection,
    task_id: &str,
    update: TaskUpdate,
) -> Result<TaskUpdateOutcome> {
    if let Some(status) = update.status.as_deref() {
        validate_choice("status", status, STATUSES)?;
    }
    if let Some(priority) = update.priority.as_deref() {
        validate_choice("priority", priority, PRIORITIES)?;
    }
    let mut changed = false;
    let mut tx = conn.begin().await?;
    if let Some(title) = update.title {
        update_task_field(&mut tx, task_id, "title", &title).await?;
        changed = true;
    }
    if let Some(description) = update.description {
        update_task_field(&mut tx, task_id, "description", &description).await?;
        changed = true;
    }
    if let Some(project) = update.project {
        let project = resolve_project_for_add_in_workspace(
            &mut tx,
            crate::workspaces::active_workspace_id().as_str(),
            Some(&project),
        )
        .await?;
        update_task_field(&mut tx, task_id, "project", &project.key).await?;
        changed = true;
    }
    if let Some(status) = update.status {
        update_task_field(&mut tx, task_id, "status", &status).await?;
        changed = true;
    }
    if let Some(priority) = update.priority {
        update_task_field(&mut tx, task_id, "priority", &priority).await?;
        changed = true;
    }
    let workspace_id = crate::workspaces::active_workspace_id();
    if update_task_labels_in_workspace(
        &mut tx,
        &workspace_id,
        task_id,
        &update.add_labels,
        &update.remove_labels,
    )
    .await?
    {
        changed = true;
    }
    tx.commit().await?;
    info!(task_id = %task_id, changed, "task updated");
    Ok(TaskUpdateOutcome {
        task: get_task(conn, task_id).await?,
        changed,
    })
}

pub(crate) async fn update_task_field(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    set_task_field(conn, task_id, field, value).await
}

#[allow(dead_code)]
pub(crate) async fn update_task_labels(
    conn: &mut SqliteConnection,
    task_id: &str,
    add_labels: &[String],
    remove_labels: &[String],
) -> Result<bool> {
    update_task_labels_in_workspace(
        conn,
        crate::workspaces::active_workspace_id().as_str(),
        task_id,
        add_labels,
        remove_labels,
    )
    .await
}

pub(crate) async fn update_task_labels_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
    add_labels: &[String],
    remove_labels: &[String],
) -> Result<bool> {
    let mut changed = false;
    for label in resolve_labels_in_workspace(conn, workspace_id, add_labels).await? {
        sqlx::query(
            "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(&label)
        .execute(&mut *conn)
        .await?;
        insert_change(
            conn,
            "task",
            task_id,
            Some("labels"),
            "label_add",
            json!({
                "workspace_id": workspace_id,
                "workspace_key": crate::workspaces::active_workspace().key,
                "label": label,
            }),
            None,
        )
        .await?;
        changed = true;
    }
    for label in resolve_labels_in_workspace(conn, workspace_id, remove_labels).await? {
        sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ? AND label = ?")
            .bind(workspace_id)
            .bind(task_id)
            .bind(&label)
            .execute(&mut *conn)
            .await?;
        insert_change(
            conn,
            "task",
            task_id,
            Some("labels"),
            "label_remove",
            json!({
                "workspace_id": workspace_id,
                "workspace_key": crate::workspaces::active_workspace().key,
                "label": label,
            }),
            None,
        )
        .await?;
        changed = true;
    }
    if changed {
        info!(
            task_id = %task_id,
            added = add_labels.len(),
            removed = remove_labels.len(),
            "task labels changed"
        );
    }
    Ok(changed)
}

pub(crate) async fn set_task_deleted(
    conn: &mut SqliteConnection,
    task_id: &str,
    deleted: bool,
) -> Result<TaskOutcome> {
    set_task_field(conn, task_id, "deleted", if deleted { "1" } else { "0" }).await?;
    info!(task_id = %task_id, deleted, "task deleted flag changed");
    Ok(TaskOutcome {
        task: get_task(conn, task_id).await?,
        create_change_id: None,
    })
}

pub(crate) async fn add_note(
    conn: &mut SqliteConnection,
    task_id: &str,
    body: String,
) -> Result<NoteOutcome> {
    let note_id = new_id();
    let workspace_id = crate::workspaces::active_workspace_id();
    let ts = now();
    let mut tx = conn.begin().await?;
    let change_id = insert_change(
        &mut tx,
        "task",
        task_id,
        Some("notes"),
        "note_add",
        json!({
            "workspace_id": workspace_id,
            "workspace_key": crate::workspaces::active_workspace().key,
            "note_id": note_id,
            "body": body,
            "created_at": ts,
        }),
        None,
    )
    .await?;
    sqlx::query(
        "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace_id)
    .bind(&note_id)
    .bind(task_id)
    .bind(&body)
    .bind(&ts)
    .bind(&change_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    info!(task_id = %task_id, note_id = %note_id, "note added");
    Ok(NoteOutcome {
        task_id: task_id.to_string(),
        note_id,
    })
}

pub(crate) async fn create_label_operation(
    conn: &mut SqliteConnection,
    name: &str,
) -> Result<LabelOutcome> {
    create_label_operation_in_workspace(
        conn,
        crate::workspaces::active_workspace_id().as_str(),
        name,
    )
    .await
}

pub(crate) async fn create_label_operation_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    name: &str,
) -> Result<LabelOutcome> {
    let name = normalize_label(name);
    if name.is_empty() {
        bail!("error invalid-label");
    }
    let existed = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM labels WHERE workspace_id = ? AND name = ?",
    )
    .bind(workspace_id)
    .bind(&name)
    .fetch_one(&mut *conn)
    .await?
        > 0;
    let created_at = now();
    sqlx::query("INSERT OR IGNORE INTO labels(workspace_id, name, created_at) VALUES (?, ?, ?)")
        .bind(workspace_id)
        .bind(&name)
        .bind(&created_at)
        .execute(&mut *conn)
        .await?;
    let created = !existed;
    let change_id = if created {
        Some(
            insert_change(
                conn,
                "label",
                &name,
                None,
                "create_label",
                json!({
                    "workspace_id": workspace_id,
                    "workspace_key": crate::workspaces::active_workspace().key,
                    "name": name,
                    "created_at": created_at,
                }),
                None,
            )
            .await?,
        )
    } else {
        None
    };
    if created {
        info!("label created");
    }
    Ok(LabelOutcome {
        name,
        created,
        change_id,
    })
}

pub(crate) async fn create_project_operation(
    conn: &mut SqliteConnection,
    name: &str,
    path: Option<&Path>,
) -> Result<ProjectOutcome> {
    let path = path.map(canonicalize_project_path).transpose()?;
    let outcome = create_project_in_workspace(
        conn,
        crate::workspaces::active_workspace_id().as_str(),
        name,
    )
    .await?;
    if let Some(path) = path {
        save_project_path_mapping(&outcome.project, path)?;
    }
    if outcome.created {
        info!(project_key = %outcome.project.key, "project created");
    }
    Ok(ProjectOutcome {
        project: outcome.project,
        created: outcome.created,
        change_id: outcome.change_id,
    })
}

fn canonicalize_project_path(path: &Path) -> Result<PathBuf> {
    fs::canonicalize(path).with_context(|| format!("could not resolve {}", path.display()))
}

fn project_path_remove_candidates(path: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(path) = fs::canonicalize(path) {
        paths.push(path);
    }
    let supplied = if path.is_absolute() {
        path.to_path_buf()
    } else if let Ok(cwd) = env::current_dir() {
        cwd.join(path)
    } else {
        path.to_path_buf()
    };
    if !paths.iter().any(|path| path == &supplied) {
        paths.push(supplied);
    }
    paths
}

async fn resolve_project_path_target(
    conn: &mut SqliteConnection,
    project: &str,
    path: &Path,
) -> Result<ProjectPathTarget> {
    let project = resolve_existing_project_in_workspace(
        conn,
        crate::workspaces::active_workspace_id().as_str(),
        project,
    )
    .await?;
    let path = canonicalize_project_path(path)?;
    Ok(ProjectPathTarget { project, path })
}

fn save_project_path_mapping(project: &Project, path: PathBuf) -> Result<ProjectPathOutcome> {
    let config_path = config::config_file_path()?;
    let mut app_config = config::AppConfig::load_from_path(&config_path)?;
    let workspace = crate::workspaces::active_workspace();
    app_config.add_project_override_path(
        Some(&workspace.id),
        Some(&workspace.key),
        &project.key,
        path.clone(),
    );
    config::write_config(&config_path, &app_config)?;
    Ok(ProjectPathOutcome {
        project: project.clone(),
        path: path.display().to_string(),
        config_path,
    })
}

pub(crate) async fn add_project_path_operation(
    conn: &mut SqliteConnection,
    project: &str,
    path: &Path,
) -> Result<ProjectPathOutcome> {
    let target = resolve_project_path_target(conn, project, path).await?;
    save_project_path_mapping(&target.project, target.path)
}

pub(crate) async fn remove_project_path_operation(
    conn: &mut SqliteConnection,
    project: &str,
    path: &Path,
) -> Result<ProjectPathOutcome> {
    let project = resolve_existing_project_in_workspace(
        conn,
        crate::workspaces::active_workspace_id().as_str(),
        project,
    )
    .await?;
    let config_path = config::config_file_path()?;
    let mut app_config = config::AppConfig::load_from_path(&config_path)?;
    let workspace = crate::workspaces::active_workspace();
    let remove_paths = project_path_remove_candidates(path);
    app_config.remove_project_override_path(
        Some(&workspace.id),
        Some(&workspace.key),
        &project.key,
        &remove_paths,
    );
    for path in &remove_paths {
        sqlx::query(
            "DELETE FROM project_paths WHERE workspace_id = ? AND project_key = ? AND path = ?",
        )
        .bind(&project.workspace_id)
        .bind(&project.key)
        .bind(path.display().to_string())
        .execute(&mut *conn)
        .await?;
    }
    config::write_config(&config_path, &app_config)?;
    let path = remove_paths
        .first()
        .unwrap_or(&path.to_path_buf())
        .display()
        .to_string();
    Ok(ProjectPathOutcome {
        project,
        path,
        config_path,
    })
}

pub(crate) async fn list_conflicts(
    conn: &mut SqliteConnection,
    project_key: Option<&str>,
    field: Option<&str>,
) -> Result<Vec<ConflictListItem>> {
    let workspace_id = crate::workspaces::active_workspace_id();
    let rows = sqlx::query(
        r#"SELECT c.task_id, c.field, c.variant_a, c.variant_b,
                 t.title, p.prefix, t.project_key
                 FROM conflicts c
                 JOIN tasks t ON t.workspace_id = c.workspace_id AND t.id = c.task_id
                 JOIN projects p ON p.workspace_id = t.workspace_id AND p.key = t.project_key
                 WHERE c.workspace_id = ? AND c.resolved = 0
                 AND (? IS NULL OR t.project_key = ?)
                 AND (? IS NULL OR c.field = ?)
                 ORDER BY c.created_at"#,
    )
    .bind(&workspace_id)
    .bind(project_key)
    .bind(project_key)
    .bind(field)
    .bind(field)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ConflictListItem {
            task_id: row.get("task_id"),
            title: row.get("title"),
            project_key: row.get("project_key"),
            project_prefix: row.get("prefix"),
            field: row.get("field"),
            variant_a: row.get("variant_a"),
            variant_b: row.get("variant_b"),
        })
        .collect())
}

pub(crate) async fn task_conflicts(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: Option<&str>,
) -> Result<Vec<ConflictDetail>> {
    let workspace_id = crate::workspaces::active_workspace_id();
    let rows = sqlx::query(
        r#"SELECT field, variant_a, local_value, variant_b, remote_value
         FROM conflicts
         WHERE workspace_id = ? AND task_id = ? AND resolved = 0 AND (? IS NULL OR field = ?)
         ORDER BY field, id"#,
    )
    .bind(&workspace_id)
    .bind(task_id)
    .bind(field)
    .bind(field)
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ConflictDetail {
            field: row.get("field"),
            variant_a: row.get("variant_a"),
            local_value: row.get("local_value"),
            variant_b: row.get("variant_b"),
            remote_value: row.get("remote_value"),
        })
        .collect())
}

pub(crate) async fn conflict_variant_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    token: &str,
) -> Result<String> {
    for detail in task_conflicts(conn, task_id, Some(field)).await? {
        if token == detail.variant_a {
            return Ok(detail.local_value);
        }
        if token == detail.variant_b {
            return Ok(detail.remote_value);
        }
    }
    bail!("error unknown-variant token={token}")
}

pub(crate) async fn resolve_conflict(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<ConflictOutcome> {
    let workspace_id = crate::workspaces::active_workspace_id();
    let mut tx = conn.begin().await?;
    let result = sqlx::query(
        "UPDATE conflicts SET resolved = 1 WHERE workspace_id = ? AND task_id = ? AND field = ? AND resolved = 0",
    )
    .bind(&workspace_id)
    .bind(task_id)
    .bind(field)
    .execute(&mut *tx)
    .await?;
    if result.rows_affected() != 1 {
        bail!("error conflict-not-found task_id={task_id} field={field}");
    }
    apply_field_value_in_workspace(&mut tx, &workspace_id, task_id, field, value).await?;
    let change_id = insert_change(
        &mut tx,
        "task",
        task_id,
        Some(field),
        "resolve_field",
        json!({
            "workspace_id": workspace_id,
            "workspace_key": crate::workspaces::active_workspace().key,
            "value": value,
        }),
        None,
    )
    .await?;
    set_field_version(&mut tx, task_id, field, &change_id).await?;
    tx.commit().await?;
    info!(task_id = %task_id, field = %field, "conflict resolved");
    Ok(ConflictOutcome {
        task: get_task(conn, task_id).await?,
        field: field.to_string(),
    })
}

pub(crate) fn show_config() -> Result<ConfigShowOutcome> {
    let path = config::config_file_path()?;
    let config = config::AppConfig::load()?;
    let text = serde_yaml::to_string(&config)?;
    Ok(ConfigShowOutcome { path, text })
}

pub(crate) fn show_config_status() -> Result<ConfigStatusOutcome> {
    let config = config::AppConfig::load()?;
    let sync_server = config::resolve_sync_server(None, &config)
        .unwrap_or_else(|error| format!("unavailable ({error:#})"));
    let wake_addr = config.wake_addr().map_or_else(
        |error| format!("invalid ({error:#})"),
        |addr| addr.to_string(),
    );
    Ok(ConfigStatusOutcome {
        lines: vec![
            format!("sync enabled: {}", config.sync.enabled),
            format!("sync server: {sync_server}"),
            format!("sync interval seconds: {}", config.sync_interval_seconds()),
            format!("daemon wake address: {wake_addr}"),
            "daemon state: not checked from TUI".to_string(),
            "sync state: not checked from TUI".to_string(),
        ],
    })
}

pub(crate) fn show_config_paths() -> Result<ConfigPathsOutcome> {
    let config = config::AppConfig::load()?;
    let config_dir = config::config_dir_path()?;
    let config_file = config::config_file_path()?;
    let default_db = config::default_db_path()?;
    let effective_db = config::resolve_db_path(None, &config)?;
    let db_source = if std::env::var_os("AVEN_DB").is_some() {
        "AVEN_DB"
    } else if config.local.db_path.is_some() {
        "config local.db_path"
    } else {
        "default"
    };
    Ok(ConfigPathsOutcome {
        lines: vec![
            format!("config directory: {}", config_dir.display()),
            format!("config file: {}", config_file.display()),
            format!("default database: {}", default_db.display()),
            format!("effective database: {}", effective_db.display()),
            format!("database source: {db_source}"),
        ],
    })
}

pub(crate) fn init_config() -> Result<ConfigInitOutcome> {
    let path = config::config_file_path()?;
    config::write_default_config(&path)?;
    Ok(ConfigInitOutcome { path })
}
