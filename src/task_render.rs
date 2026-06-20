use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::db::task_has_conflict;
use crate::query::TaskListItem;
use crate::refs::display_ref;
use crate::render::quote;
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
    let conflict = if item.has_conflict {
        " conflicts=yes"
    } else {
        ""
    };
    let deleted = if item.task.deleted {
        " deleted=yes"
    } else {
        ""
    };
    println!(
        "{} status={} priority={} labels={}{}{} title={}",
        item.display_ref,
        item.task.status,
        item.task.priority,
        labels,
        conflict,
        deleted,
        quote(&item.task.title)
    );
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
            println!("note created={} body={}", created_at, quote(&body));
        }
        print_conflicts(conn, task, None).await?;
    }
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
        println!("variant {} value={}", variant_a, quote(&local_value));
        println!("variant {} value={}", variant_b, quote(&remote_value));
    }
    Ok(())
}
