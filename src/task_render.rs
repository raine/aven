use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::db::task_has_conflict;
use crate::query::TaskListItem;
use crate::refs::display_ref;
use crate::render::quote;
use crate::types::Task;

pub(crate) async fn labels_for_task(
    conn: &mut SqliteConnection,
    task_id: &str,
) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar!(
        r#"SELECT label AS "label!: String" FROM task_labels WHERE task_id = ? ORDER BY label"#,
        task_id
    )
    .fetch_all(&mut *conn)
    .await?)
}

async fn print_task_line(conn: &mut SqliteConnection, task: &Task) -> Result<()> {
    let labels = labels_for_task(conn, &task.id).await?.join(",");
    let conflict = if task_has_conflict(conn, &task.id).await? {
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
        let notes = sqlx::query!(
            r#"SELECT body AS "body!: String", created_at AS "created_at!: String"
             FROM notes WHERE task_id = ? ORDER BY created_at, id"#,
            task.id,
        )
        .fetch_all(&mut *conn)
        .await?;
        for note in notes {
            println!(
                "note created={} body={}",
                note.created_at,
                quote(&note.body)
            );
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
    let rows = sqlx::query!(
        r#"SELECT field AS "field!: String", variant_a AS "variant_a!: String",
         local_value AS "local_value!: String", variant_b AS "variant_b!: String",
         remote_value AS "remote_value!: String"
         FROM conflicts
         WHERE task_id = ? AND resolved = 0 AND (? IS NULL OR field = ?)
         ORDER BY field, id"#,
        task.id,
        field,
        field,
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        println!(
            "conflict {} field={}",
            display_ref(conn, task).await?,
            row.field
        );
        println!(
            "variant {} value={}",
            row.variant_a,
            quote(&row.local_value)
        );
        println!(
            "variant {} value={}",
            row.variant_b,
            quote(&row.remote_value)
        );
    }
    Ok(())
}

pub(crate) async fn conflict_variant_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    token: &str,
) -> Result<String> {
    let rows = sqlx::query!(
        r#"SELECT variant_a AS "variant_a!: String", local_value AS "local_value!: String",
         variant_b AS "variant_b!: String", remote_value AS "remote_value!: String"
         FROM conflicts WHERE task_id = ? AND field = ? AND resolved = 0"#,
        task_id,
        field,
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        if token == row.variant_a {
            return Ok(row.local_value);
        }
        if token == row.variant_b {
            return Ok(row.remote_value);
        }
    }
    bail!("error unknown-variant token={}", token);
}
