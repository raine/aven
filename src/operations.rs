use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::json;
use sqlx::{Connection as _, SqliteConnection};

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::config;
use crate::db::{insert_change, set_field_version};
use crate::ids::{new_id, now};
use crate::labels::{normalize_label, resolve_labels};
use crate::mutation::{apply_field_value, set_task_field};
use crate::projects::{
    add_project_path as add_project_path_mapping, create_project, resolve_existing_project,
    resolve_project_for_add,
};
use crate::refs::get_task;
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
}

pub(crate) struct TaskUpdate {
    pub(crate) title: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) project: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) add_labels: Vec<String>,
    pub(crate) remove_labels: Vec<String>,
}

impl Default for TaskUpdate {
    fn default() -> Self {
        Self {
            title: None,
            description: None,
            project: None,
            status: None,
            priority: None,
            add_labels: Vec::new(),
            remove_labels: Vec::new(),
        }
    }
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
}

pub(crate) struct ProjectOutcome {
    pub(crate) project: Project,
}

pub(crate) struct ProjectPathOutcome {
    pub(crate) project: Project,
    pub(crate) path: String,
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
    validate_choice("priority", &draft.priority, PRIORITIES)?;
    let id = new_id();
    let ts = now();
    let mut tx = conn.begin().await?;
    let project = resolve_project_for_add(&mut tx, draft.project.as_deref()).await?;
    let labels = resolve_labels(&mut tx, &draft.labels).await?;
    sqlx::query!(
        "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'inbox', ?, ?, ?)",
        id,
        draft.title,
        draft.description,
        project.key,
        draft.priority,
        ts,
        ts,
    )
    .execute(&mut *tx)
    .await?;
    for label in &labels {
        sqlx::query!(
            "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
            id,
            label,
        )
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
    for field in [
        "title",
        "description",
        "project",
        "status",
        "priority",
        "deleted",
    ] {
        set_field_version(&mut tx, &id, field, &change_id).await?;
    }
    tx.commit().await?;
    Ok(TaskOutcome {
        task: get_task(conn, &id).await?,
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
        let project = resolve_project_for_add(&mut tx, Some(&project)).await?;
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
    if update_task_labels(&mut tx, task_id, &update.add_labels, &update.remove_labels).await? {
        changed = true;
    }
    tx.commit().await?;
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

pub(crate) async fn update_task_labels(
    conn: &mut SqliteConnection,
    task_id: &str,
    add_labels: &[String],
    remove_labels: &[String],
) -> Result<bool> {
    let mut changed = false;
    for label in resolve_labels(conn, add_labels).await? {
        sqlx::query!(
            "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
            task_id,
            label,
        )
        .execute(&mut *conn)
        .await?;
        insert_change(
            conn,
            "task",
            task_id,
            Some("labels"),
            "label_add",
            json!({ "label": label }),
            None,
        )
        .await?;
        changed = true;
    }
    for label in resolve_labels(conn, remove_labels).await? {
        sqlx::query!(
            "DELETE FROM task_labels WHERE task_id = ? AND label = ?",
            task_id,
            label,
        )
        .execute(&mut *conn)
        .await?;
        insert_change(
            conn,
            "task",
            task_id,
            Some("labels"),
            "label_remove",
            json!({ "label": label }),
            None,
        )
        .await?;
        changed = true;
    }
    Ok(changed)
}

pub(crate) async fn set_task_deleted(
    conn: &mut SqliteConnection,
    task_id: &str,
    deleted: bool,
) -> Result<TaskOutcome> {
    set_task_field(conn, task_id, "deleted", if deleted { "1" } else { "0" }).await?;
    Ok(TaskOutcome {
        task: get_task(conn, task_id).await?,
    })
}

pub(crate) async fn add_note(
    conn: &mut SqliteConnection,
    task_id: &str,
    body: String,
) -> Result<NoteOutcome> {
    let note_id = new_id();
    let ts = now();
    let mut tx = conn.begin().await?;
    let change_id = insert_change(
        &mut tx,
        "task",
        task_id,
        Some("notes"),
        "note_add",
        json!({ "note_id": note_id, "body": body, "created_at": ts }),
        None,
    )
    .await?;
    sqlx::query!(
        "INSERT INTO notes(id, task_id, body, created_at, change_id) VALUES (?, ?, ?, ?, ?)",
        note_id,
        task_id,
        body,
        ts,
        change_id,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(NoteOutcome {
        task_id: task_id.to_string(),
        note_id,
    })
}

pub(crate) async fn create_label_operation(
    conn: &mut SqliteConnection,
    name: &str,
) -> Result<LabelOutcome> {
    let name = normalize_label(name);
    if name.is_empty() {
        bail!("error invalid-label");
    }
    let created_at = now();
    sqlx::query!(
        "INSERT OR IGNORE INTO labels(name, created_at) VALUES (?, ?)",
        name,
        created_at,
    )
    .execute(&mut *conn)
    .await?;
    insert_change(
        conn,
        "label",
        &name,
        None,
        "create_label",
        json!({ "name": name, "created_at": created_at }),
        None,
    )
    .await?;
    Ok(LabelOutcome { name })
}

pub(crate) async fn create_project_operation(
    conn: &mut SqliteConnection,
    name: &str,
    path: Option<&Path>,
) -> Result<ProjectOutcome> {
    let project = create_project(conn, name).await?;
    if let Some(path) = path {
        add_project_path_mapping(conn, &project.key, path).await?;
    }
    Ok(ProjectOutcome { project })
}

fn canonicalize_project_path(path: &Path) -> Result<String> {
    let path =
        fs::canonicalize(path).with_context(|| format!("could not resolve {}", path.display()))?;
    Ok(path.display().to_string())
}

pub(crate) async fn add_project_path_operation(
    conn: &mut SqliteConnection,
    project: &str,
    path: &Path,
) -> Result<ProjectPathOutcome> {
    let project = resolve_existing_project(conn, project).await?;
    let path = canonicalize_project_path(path)?;
    add_project_path_mapping(conn, &project.key, Path::new(&path)).await?;
    Ok(ProjectPathOutcome { project, path })
}

pub(crate) async fn remove_project_path_operation(
    conn: &mut SqliteConnection,
    project: &str,
    path: &Path,
) -> Result<ProjectPathOutcome> {
    let project = resolve_existing_project(conn, project).await?;
    let path = canonicalize_project_path(path)?;
    sqlx::query!(
        "DELETE FROM project_paths WHERE project_key = ? AND path = ?",
        project.key,
        path,
    )
    .execute(&mut *conn)
    .await?;
    Ok(ProjectPathOutcome { project, path })
}

pub(crate) async fn list_conflicts(
    conn: &mut SqliteConnection,
    project_key: Option<&str>,
    field: Option<&str>,
) -> Result<Vec<ConflictListItem>> {
    let rows = sqlx::query!(
        r#"SELECT c.task_id AS "task_id!: String", c.field AS "field!: String",
                 c.variant_a AS "variant_a!: String", c.variant_b AS "variant_b!: String",
                 t.title AS "title!: String", p.prefix AS "prefix!: String",
                 t.project_key AS "project_key!: String"
                 FROM conflicts c
                 JOIN tasks t ON t.id = c.task_id
                 JOIN projects p ON p.key = t.project_key
                 WHERE c.resolved = 0
                 AND (?1 IS NULL OR t.project_key = ?1)
                 AND (?2 IS NULL OR c.field = ?2)
                 ORDER BY c.created_at"#,
        project_key,
        field,
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ConflictListItem {
            task_id: row.task_id,
            title: row.title,
            project_key: row.project_key,
            project_prefix: row.prefix,
            field: row.field,
            variant_a: row.variant_a,
            variant_b: row.variant_b,
        })
        .collect())
}

pub(crate) async fn task_conflicts(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: Option<&str>,
) -> Result<Vec<ConflictDetail>> {
    let rows = sqlx::query!(
        r#"SELECT field AS "field!: String", variant_a AS "variant_a!: String",
         local_value AS "local_value!: String", variant_b AS "variant_b!: String",
         remote_value AS "remote_value!: String"
         FROM conflicts
         WHERE task_id = ? AND resolved = 0 AND (? IS NULL OR field = ?)
         ORDER BY field, id"#,
        task_id,
        field,
        field,
    )
    .fetch_all(&mut *conn)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| ConflictDetail {
            field: row.field,
            variant_a: row.variant_a,
            local_value: row.local_value,
            variant_b: row.variant_b,
            remote_value: row.remote_value,
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
    let exists = sqlx::query_scalar::<_, i64>(
        "SELECT id FROM conflicts WHERE task_id = ? AND field = ? AND resolved = 0 LIMIT 1",
    )
    .bind(task_id)
    .bind(field)
    .fetch_optional(&mut *conn)
    .await?;
    if exists.is_none() {
        bail!("error conflict-not-found task_id={task_id} field={field}");
    }

    let mut tx = conn.begin().await?;
    apply_field_value(&mut tx, task_id, field, value).await?;
    sqlx::query!(
        "UPDATE conflicts SET resolved = 1 WHERE task_id = ? AND field = ? AND resolved = 0",
        task_id,
        field,
    )
    .execute(&mut *tx)
    .await?;
    let change_id = insert_change(
        &mut tx,
        "task",
        task_id,
        Some(field),
        "resolve_field",
        json!({ "value": value }),
        None,
    )
    .await?;
    set_field_version(&mut tx, task_id, field, &change_id).await?;
    tx.commit().await?;
    Ok(ConflictOutcome {
        task: get_task(conn, task_id).await?,
        field: field.to_string(),
    })
}

pub(crate) fn show_config() -> Result<ConfigShowOutcome> {
    let path = config::config_file_path()?;
    let config = config::AppConfig::load()?;
    let text = toml::to_string_pretty(&config)?;
    Ok(ConfigShowOutcome { path, text })
}

pub(crate) fn show_config_status() -> Result<ConfigStatusOutcome> {
    let config = config::AppConfig::load()?;
    let sync_server = config::resolve_sync_server(None, &config)
        .map_or_else(|error| format!("unavailable ({error:#})"), |server| server);
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
    let db_source = if std::env::var_os("ATM_DB").is_some() {
        "ATM_DB"
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
