use anyhow::{Result, bail};
use sqlx::SqliteConnection;
use tracing::info;

use crate::change_log::{ChangeEntity, ChangePayload, append_change, op_type};
use crate::choices::{TaskPriority, TaskStatus};
use crate::db::{begin_immediate, set_field_version};
use crate::ids::{new_id, now};
use crate::labels::resolve_labels_in_workspace;
use crate::mutation::{set_task_field, set_task_project};
use crate::projects::resolve_project_for_add_in_workspace;
use crate::refs::get_task;
use crate::task_fields::TaskField;
use crate::types::Task;

pub(crate) struct TaskDraft {
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) project: Option<String>,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) labels: Vec<String>,
    pub(crate) is_epic: bool,
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
    pub(crate) is_epic: Option<bool>,
    pub(crate) add_labels: Vec<String>,
    pub(crate) remove_labels: Vec<String>,
}

pub(crate) struct TaskUpdateOutcome {
    pub(crate) task: Task,
    pub(crate) changed: bool,
}

pub(crate) struct NoteDeleteOutcome {
    #[allow(dead_code)]
    pub(crate) task_id: String,
    #[allow(dead_code)]
    pub(crate) note_id: String,
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
    let status = TaskStatus::parse(&draft.status)?;
    let priority = TaskPriority::parse(&draft.priority)?;
    let id = new_id();
    let ts = now();
    let mut tx = begin_immediate(conn).await?;
    let workspace = crate::workspaces::workspace_for_id(&mut tx, workspace_id).await?;
    let project =
        resolve_project_for_add_in_workspace(&mut tx, &workspace.id, draft.project.as_deref())
            .await?;
    let labels = resolve_labels_in_workspace(&mut tx, &workspace.id, &draft.labels).await?;
    sqlx::query(
        "INSERT INTO tasks(workspace_id, id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at, is_epic)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace.id)
    .bind(&id)
    .bind(&draft.title)
    .bind(&draft.description)
    .bind(&project.id)
    .bind(status.as_str())
    .bind(priority.as_str())
    .bind(&ts)
    .bind(&ts)
    .bind(&ts)
    .bind(i64::from(draft.is_epic))
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
    let change_id = append_change(
        &mut tx,
        ChangeEntity::Task,
        &id,
        None,
        op_type::CREATE_TASK,
        ChangePayload::workspace(&workspace)
            .set("title", draft.title)
            .set("description", draft.description)
            .set("project_id", project.id.clone())
            .set("project_key", project.key.clone())
            .set("project_name", project.name.clone())
            .set("project_prefix", project.prefix.clone())
            .set("status", status.as_str())
            .set("priority", priority.as_str())
            .set("is_epic", if draft.is_epic { "1" } else { "0" })
            .set("labels", &labels)
            .set("created_at", ts),
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
        TaskStatus::parse(status)?;
    }
    if let Some(priority) = update.priority.as_deref() {
        TaskPriority::parse(priority)?;
    }
    let mut changed = false;
    let mut tx = begin_immediate(conn).await?;
    if let Some(title) = update.title {
        changed |= update_task_field(&mut tx, task_id, "title", &title).await?;
    }
    if let Some(description) = update.description {
        changed |= update_task_field(&mut tx, task_id, "description", &description).await?;
    }
    if let Some(project) = update.project {
        let project = resolve_project_for_add_in_workspace(
            &mut tx,
            crate::workspaces::active_workspace_id().as_str(),
            Some(&project),
        )
        .await?;
        changed |= set_task_project(&mut tx, task_id, &project).await?;
    }
    if let Some(status) = update.status {
        changed |= update_task_field(&mut tx, task_id, "status", &status).await?;
    }
    if let Some(priority) = update.priority {
        changed |= update_task_field(&mut tx, task_id, "priority", &priority).await?;
    }
    if let Some(is_epic) = update.is_epic {
        if !is_epic {
            let task = get_task(&mut tx, task_id).await?;
            if super::epics::task_has_epic_children(&mut tx, &task.workspace_id, task_id).await? {
                bail!("error epic-has-children task_id={task_id}");
            }
        }
        changed |=
            update_task_field(&mut tx, task_id, "is_epic", if is_epic { "1" } else { "0" }).await?;
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
) -> Result<bool> {
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
        let rows_affected = sqlx::query(
            "INSERT OR IGNORE INTO task_labels(workspace_id, task_id, label) VALUES (?, ?, ?)",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(&label)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if rows_affected > 0 {
            append_change(
                conn,
                ChangeEntity::Task,
                task_id,
                Some("labels"),
                op_type::LABEL_ADD,
                ChangePayload::workspace(&workspace).set("label", label),
            )
            .await?;
            changed = true;
        }
    }
    for label in resolve_labels_in_workspace(conn, &workspace.id, remove_labels).await? {
        let rows_affected = sqlx::query(
            "DELETE FROM task_labels WHERE workspace_id = ? AND task_id = ? AND label = ?",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(&label)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if rows_affected > 0 {
            append_change(
                conn,
                ChangeEntity::Task,
                task_id,
                Some("labels"),
                op_type::LABEL_REMOVE,
                ChangePayload::workspace(&workspace).set("label", label),
            )
            .await?;
            changed = true;
        }
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
    let workspace = crate::workspaces::active_workspace();
    let ts = now();
    let mut tx = begin_immediate(conn).await?;
    let change_id = append_change(
        &mut tx,
        ChangeEntity::Task,
        task_id,
        Some("notes"),
        op_type::NOTE_ADD,
        ChangePayload::workspace(&workspace)
            .set("note_id", &note_id)
            .set("body", &body)
            .set("created_at", &ts),
    )
    .await?;
    sqlx::query(
        "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&workspace.id)
    .bind(&note_id)
    .bind(task_id)
    .bind(&body)
    .bind(&ts)
    .bind(&change_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
        .bind(&ts)
        .bind(&workspace.id)
        .bind(task_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    info!(task_id = %task_id, note_id = %note_id, "note added");
    Ok(NoteOutcome {
        task_id: task_id.to_string(),
        note_id,
    })
}

pub(crate) async fn delete_note(
    conn: &mut SqliteConnection,
    task_id: &str,
    note_id: &str,
) -> Result<NoteDeleteOutcome> {
    let workspace = crate::workspaces::active_workspace();
    let mut tx = begin_immediate(conn).await?;
    let deleted_at = now();
    let deleted =
        sqlx::query("DELETE FROM notes WHERE workspace_id = ? AND task_id = ? AND id = ?")
            .bind(&workspace.id)
            .bind(task_id)
            .bind(note_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
    if deleted > 0 {
        sqlx::query("UPDATE tasks SET queue_activity_at = ? WHERE workspace_id = ? AND id = ?")
            .bind(&deleted_at)
            .bind(&workspace.id)
            .bind(task_id)
            .execute(&mut *tx)
            .await?;
        append_change(
            &mut tx,
            ChangeEntity::Task,
            task_id,
            Some("notes"),
            op_type::NOTE_DELETE,
            ChangePayload::workspace(&workspace)
                .set("note_id", note_id)
                .set("deleted_at", deleted_at),
        )
        .await?;
    }
    tx.commit().await?;
    if deleted > 0 {
        info!(task_id = %task_id, note_id = %note_id, "note deleted");
    }
    Ok(NoteDeleteOutcome {
        task_id: task_id.to_string(),
        note_id: note_id.to_string(),
        changed: deleted > 0,
    })
}
