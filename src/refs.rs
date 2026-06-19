use std::collections::HashMap;

use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::render::quote;
use crate::types::Task;

const DISPLAY_SUFFIX_FLOOR: usize = 4;

pub(crate) async fn get_task(conn: &mut SqliteConnection, id: &str) -> Result<Task> {
    let row = sqlx::query!(
        r#"SELECT t.id AS "id!: String", t.title AS "title!: String",
         t.description AS "description!: String", t.project_key AS "project_key!: String",
         p.prefix AS "project_prefix!: String", t.status AS "status!: String",
         t.priority AS "priority!: String", t.created_at AS "created_at!: String",
         t.updated_at AS "updated_at!: String", t.deleted AS "deleted!: i64"
         FROM tasks t JOIN projects p ON p.key = t.project_key
         WHERE t.id = ?"#,
        id,
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Task {
        id: row.id,
        title: row.title,
        description: row.description,
        project_key: row.project_key,
        project_prefix: row.project_prefix,
        status: row.status,
        priority: row.priority,
        created_at: row.created_at,
        updated_at: row.updated_at,
        deleted: row.deleted != 0,
    })
}

pub(crate) async fn resolve_task_ref(conn: &mut SqliteConnection, input: &str) -> Result<Task> {
    let (hint, suffix) = split_ref(input);
    if suffix.len() < 3 {
        bail!("error ref-too-short input={} minimum=3", input);
    }
    let suffix = suffix.to_ascii_uppercase();
    let rows = sqlx::query!(
        r#"SELECT t.id AS "id!: String", t.title AS "title!: String",
         t.description AS "description!: String", t.project_key AS "project_key!: String",
         p.prefix AS "project_prefix!: String", t.status AS "status!: String",
         t.priority AS "priority!: String", t.created_at AS "created_at!: String",
         t.updated_at AS "updated_at!: String", t.deleted AS "deleted!: i64"
         FROM tasks t JOIN projects p ON p.key = t.project_key
         WHERE t.id LIKE ? || '%'
         ORDER BY t.id"#,
        suffix,
    )
    .fetch_all(&mut *conn)
    .await?;
    let matches = rows
        .into_iter()
        .map(|row| Task {
            id: row.id,
            title: row.title,
            description: row.description,
            project_key: row.project_key,
            project_prefix: row.project_prefix,
            status: row.status,
            priority: row.priority,
            created_at: row.created_at,
            updated_at: row.updated_at,
            deleted: row.deleted != 0,
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("error unknown-ref input={}", input);
    }
    if let Some(hint) = hint {
        let hinted: Vec<Task> = matches
            .iter()
            .filter(|task| task.project_prefix.eq_ignore_ascii_case(&hint))
            .cloned()
            .collect();
        if hinted.len() == 1 {
            return Ok(hinted[0].clone());
        }
    }
    if matches.len() == 1 {
        return Ok(matches[0].clone());
    }
    println!("error ambiguous-ref input={}", input);
    for task in matches {
        println!(
            "match {} title={}",
            display_ref(conn, &task).await?,
            quote(&task.title)
        );
    }
    println!("hint \"retry with longer ref\"");
    bail!("ambiguous ref");
}

fn split_ref(input: &str) -> (Option<String>, String) {
    if let Some((prefix, suffix)) = input.split_once('-') {
        (Some(prefix.to_string()), normalize_ref(suffix))
    } else {
        (None, normalize_ref(input))
    }
}

fn normalize_ref(input: &str) -> String {
    input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| match ch.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            ch => ch,
        })
        .collect()
}

pub(crate) async fn display_ref(conn: &mut SqliteConnection, task: &Task) -> Result<String> {
    Ok(format!(
        "{}-{}",
        task.project_prefix,
        display_suffix(conn, &task.id).await?
    ))
}

pub(crate) async fn display_refs_for_tasks(
    conn: &mut SqliteConnection,
    tasks: &[Task],
) -> Result<HashMap<String, String>> {
    let ids = task_ids(conn).await?;
    Ok(tasks
        .iter()
        .map(|task| {
            let suffix = display_suffix_for_id(&task.id, &ids);
            (task.id.clone(), format!("{}-{suffix}", task.project_prefix))
        })
        .collect())
}

pub(crate) async fn display_suffix(conn: &mut SqliteConnection, id: &str) -> Result<String> {
    let ids = task_ids(conn).await?;
    Ok(display_suffix_for_id(id, &ids))
}

async fn task_ids(conn: &mut SqliteConnection) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT id FROM tasks ORDER BY id")
            .fetch_all(&mut *conn)
            .await?,
    )
}

fn display_suffix_for_id(id: &str, ids: &[String]) -> String {
    let len = display_suffix_len(id, ids);
    id[..len].to_string()
}

fn display_suffix_len(id: &str, ids: &[String]) -> usize {
    for len in DISPLAY_SUFFIX_FLOOR.min(id.len())..=id.len() {
        let prefix = &id[..len];
        let count = ids.iter().filter(|other| other.starts_with(prefix)).count();
        if count <= 1 {
            return len;
        }
    }
    id.len()
}
