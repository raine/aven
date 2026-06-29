use anyhow::Result;
use serde::Serialize;
use sqlx::{Row, SqliteConnection};

use crate::db::task_has_conflict;
use crate::query::{TaskDependencySummary, TaskListItem, task_dependency_summary};
use crate::refs::display_ref;
use crate::render::{KvLine, print_multiline_block, quote};
use crate::types::Task;

#[allow(dead_code)]
pub(crate) async fn labels_for_task(
    conn: &mut SqliteConnection,
    task_id: &str,
) -> Result<Vec<String>> {
    let workspace_id = crate::workspaces::active_workspace_id();
    labels_for_task_in_workspace(conn, &workspace_id, task_id).await
}

pub(crate) async fn labels_for_task_in_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT label FROM task_labels WHERE workspace_id = ? AND task_id = ? ORDER BY label",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?)
}

async fn print_task_line(conn: &mut SqliteConnection, task: &Task) -> Result<()> {
    let labels = labels_for_task_in_workspace(conn, &task.workspace_id, &task.id)
        .await?
        .join(",");
    let conflict = if task_has_conflict(conn, &task.workspace_id, &task.id).await? {
        " conflicts=yes"
    } else {
        ""
    };
    let deleted = if task.deleted { " deleted=yes" } else { "" };
    println!(
        "{} status={} priority={} labels={}{}{} title={}",
        display_ref(conn, task).await?,
        task.status,
        task.priority,
        labels,
        conflict,
        deleted,
        quote(&task.title)
    );
    Ok(())
}

pub(crate) async fn print_task_line_item(item: &TaskListItem) -> Result<()> {
    let labels = item.labels.join(",");
    let line = KvLine::new(item.display_ref.clone())
        .field("status", item.task.status)
        .field("priority", item.task.priority)
        .field("labels", &labels)
        .optional("conflicts", item.has_conflict.then(|| "yes".to_string()))
        .optional("deleted", item.task.deleted.then(|| "yes".to_string()))
        .optional(
            "blocked_by",
            (item.unresolved_blocker_count > 0).then(|| item.unresolved_blocker_count.to_string()),
        )
        .optional(
            "blocks",
            (item.dependent_count > 0).then(|| item.dependent_count.to_string()),
        )
        .quoted("title", &item.task.title)
        .finish();
    println!("{line}");
    Ok(())
}

pub(crate) async fn print_task(conn: &mut SqliteConnection, task: &Task, full: bool) -> Result<()> {
    print_task_line(conn, task).await?;
    if full {
        println!("id={}", task.id);
        println!(
            "project={} prefix={}",
            task.project_key, task.project_prefix
        );
        println!("created={} updated={}", task.created_at, task.updated_at);
        if !task.description.is_empty() {
            println!("description<<EOF");
            print!("{}", task.description);
            if !task.description.ends_with('\n') {
                println!();
            }
            println!("EOF");
        }
        print_task_dependencies(conn, task).await?;
        let notes = sqlx::query(
            "SELECT body, created_at FROM notes
             WHERE workspace_id = ? AND task_id = ? ORDER BY created_at, id",
        )
        .bind(&task.workspace_id)
        .bind(&task.id)
        .fetch_all(&mut *conn)
        .await?;
        for note in notes {
            let created_at: String = note.get("created_at");
            let body: String = note.get("body");
            println!("note created={created_at}");
            print_multiline_block("body", &body);
        }
        print_conflicts(conn, task, None).await?;
    }
    Ok(())
}

pub(crate) fn print_task_dependency_summary(summary: &TaskDependencySummary) {
    print_dependency_section("depends_on", &summary.depends_on);
    print_dependency_section("blocks", &summary.blocks);
}

fn print_dependency_section(label: &str, items: &[crate::query::TaskDependencyItem]) {
    let open = items.iter().filter(|item| item.unresolved).count();
    println!("{label} open={open} total={}", items.len());
    for item in items {
        println!(
            "- {} status={} title={}",
            item.display_ref,
            item.task.status,
            quote(&item.task.title)
        );
    }
}

async fn print_task_dependencies(conn: &mut SqliteConnection, task: &Task) -> Result<()> {
    let summary = task_dependency_summary(conn, &task.workspace_id, &task.id).await?;
    print_task_dependency_summary(&summary);
    Ok(())
}

pub(crate) async fn print_conflicts(
    conn: &mut SqliteConnection,
    task: &Task,
    field: Option<&str>,
) -> Result<()> {
    let rows = sqlx::query(
        "SELECT field, variant_a, local_value, variant_b, remote_value
         FROM conflicts
         WHERE workspace_id = ? AND task_id = ? AND resolved = 0 AND (? IS NULL OR field = ?)
         ORDER BY field, id",
    )
    .bind(&task.workspace_id)
    .bind(&task.id)
    .bind(field)
    .bind(field)
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        let field: String = row.get("field");
        let variant_a: String = row.get("variant_a");
        let local_value: String = row.get("local_value");
        let variant_b: String = row.get("variant_b");
        let remote_value: String = row.get("remote_value");
        println!(
            "conflict {} field={}",
            display_ref(conn, task).await?,
            field
        );
        println!("variant {variant_a}");
        print_multiline_block("value", &local_value);
        println!("variant {variant_b}");
        print_multiline_block("value", &remote_value);
    }
    Ok(())
}

// --- JSON DTOs ---

#[derive(Serialize)]
pub(crate) struct TaskLineJson {
    pub(crate) r#ref: String,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) project: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) labels: Vec<String>,
    pub(crate) deleted: bool,
    pub(crate) has_conflict: bool,
    pub(crate) blocked_by: i64,
    pub(crate) blocks: i64,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

pub(crate) fn task_line_json_item(item: &TaskListItem) -> TaskLineJson {
    TaskLineJson {
        r#ref: item.display_ref.clone(),
        id: item.task.id.clone(),
        title: item.task.title.clone(),
        project: item.task.project_key.clone(),
        status: item.task.status.to_string(),
        priority: item.task.priority.to_string(),
        labels: item.labels.clone(),
        deleted: item.task.deleted,
        has_conflict: item.has_conflict,
        blocked_by: item.unresolved_blocker_count,
        blocks: item.dependent_count,
        created_at: item.task.created_at.clone(),
        updated_at: item.task.updated_at.clone(),
    }
}

#[derive(Serialize)]
pub(crate) struct TaskFullJson {
    pub(crate) task: TaskLineJson,
    pub(crate) project_prefix: String,
    pub(crate) description: String,
    pub(crate) dependencies: TaskDependencySummaryJson,
    pub(crate) notes: Vec<TaskNoteJson>,
    pub(crate) conflicts: Vec<TaskConflictJson>,
}

#[derive(Serialize)]
pub(crate) struct TaskNoteJson {
    pub(crate) body: String,
    pub(crate) created_at: String,
}

#[derive(Serialize)]
pub(crate) struct TaskConflictJson {
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) local_value: String,
    pub(crate) variant_b: String,
    pub(crate) remote_value: String,
}

#[derive(Serialize)]
pub(crate) struct TaskDependencySummaryJson {
    pub(crate) depends_on_open: i64,
    pub(crate) depends_on_total: i64,
    pub(crate) blocks_open: i64,
    pub(crate) blocks_total: i64,
    pub(crate) depends_on: Vec<TaskDependencyItemJson>,
    pub(crate) blocks: Vec<TaskDependencyItemJson>,
}

#[derive(Serialize)]
pub(crate) struct TaskDependencyItemJson {
    pub(crate) r#ref: String,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) deleted: bool,
    pub(crate) unresolved: bool,
    pub(crate) created_at: String,
}

pub(crate) fn task_dependency_summary_json(
    summary: &TaskDependencySummary,
) -> TaskDependencySummaryJson {
    TaskDependencySummaryJson {
        depends_on_open: summary.depends_on.iter().filter(|d| d.unresolved).count() as i64,
        depends_on_total: summary.depends_on.len() as i64,
        blocks_open: summary.blocks.iter().filter(|d| d.unresolved).count() as i64,
        blocks_total: summary.blocks.len() as i64,
        depends_on: summary
            .depends_on
            .iter()
            .map(|d| TaskDependencyItemJson {
                r#ref: d.display_ref.clone(),
                id: d.task.id.clone(),
                title: d.task.title.clone(),
                status: d.task.status.to_string(),
                priority: d.task.priority.to_string(),
                deleted: d.task.deleted,
                unresolved: d.unresolved,
                created_at: d.task.created_at.clone(),
            })
            .collect(),
        blocks: summary
            .blocks
            .iter()
            .map(|d| TaskDependencyItemJson {
                r#ref: d.display_ref.clone(),
                id: d.task.id.clone(),
                title: d.task.title.clone(),
                status: d.task.status.to_string(),
                priority: d.task.priority.to_string(),
                deleted: d.task.deleted,
                unresolved: d.unresolved,
                created_at: d.task.created_at.clone(),
            })
            .collect(),
    }
}
