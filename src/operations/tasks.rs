use anyhow::Result;
use serde_json::json;
use sqlx::{Connection as _, SqliteConnection};
use tracing::info;

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::db::{insert_change, set_field_version};
use crate::ids::{new_id, now};
use crate::labels::resolve_labels_in_workspace;
use crate::mutation::set_task_field;
use crate::projects::resolve_project_for_add_in_workspace;
use crate::refs::get_task;
use crate::task_fields::TaskField;
use crate::types::Task;

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
    let workspace = crate::workspaces::workspace_for_id(&mut tx, workspace_id).await?;
    let project =
        resolve_project_for_add_in_workspace(&mut tx, &workspace.id, draft.project.as_deref())
            .await?;
    let labels = resolve_labels_in_workspace(&mut tx, &workspace.id, &draft.labels).await?;
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, 'inbox', ?, ?, ?)",
    )
    .bind(&workspace.id)
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
        .bind(&workspace.id)
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
            "workspace_id": &workspace.id,
            "workspace_key": &workspace.key,
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
    let workspace = crate::workspaces::workspace_for_id(conn, workspace_id).await?;
    let mut changed = false;
    for label in resolve_labels_in_workspace(conn, &workspace.id, add_labels).await? {
        sqlx::query(
            "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
        )
        .bind(&workspace.id)
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
                "workspace_id": &workspace.id,
                "workspace_key": &workspace.key,
                "label": label,
            }),
            None,
        )
        .await?;
        changed = true;
    }
    for label in resolve_labels_in_workspace(conn, &workspace.id, remove_labels).await? {
        sqlx::query("DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ? AND label = ?")
            .bind(&workspace.id)
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
                "workspace_id": &workspace.id,
                "workspace_key": &workspace.key,
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
