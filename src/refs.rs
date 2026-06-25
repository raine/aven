use std::collections::HashMap;

use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::render::quote;
use crate::types::Task;
use crate::workspaces::Workspace;

const DISPLAY_SUFFIX_FLOOR: usize = 4;

pub(crate) async fn get_task(conn: &mut SqliteConnection, id: &str) -> Result<Task> {
    get_task_scoped(conn, None, id).await
}

#[allow(dead_code)]
pub(crate) async fn get_task_in_workspace(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    id: &str,
) -> Result<Task> {
    get_task_scoped(conn, Some(&workspace.id), id).await
}

async fn get_task_scoped(
    conn: &mut SqliteConnection,
    workspace_id: Option<&str>,
    id: &str,
) -> Result<Task> {
    let row = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.created_at, t.updated_at,
         t.queue_activity_at, t.deleted
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.id = ? AND (? IS NULL OR t.workspace_id = ?)",
    )
    .bind(id)
    .bind(workspace_id)
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await?;
    crate::db::task_from_row(&row)
}

pub(crate) async fn resolve_task_ref(conn: &mut SqliteConnection, input: &str) -> Result<Task> {
    let workspace_id = crate::workspaces::active_workspace_id();
    resolve_task_ref_scoped(conn, Some(&workspace_id), input).await
}

#[allow(dead_code)]
pub(crate) async fn resolve_task_ref_in_workspace(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    input: &str,
) -> Result<Task> {
    resolve_task_ref_scoped(conn, Some(&workspace.id), input).await
}

async fn resolve_task_ref_scoped(
    conn: &mut SqliteConnection,
    workspace_id: Option<&str>,
    input: &str,
) -> Result<Task> {
    let (hint, suffix) = split_ref(input);
    if suffix.len() < 3 {
        bail!("error ref-too-short input={} minimum=3", input);
    }
    let suffix = suffix.to_ascii_uppercase();
    let rows = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.created_at, t.updated_at,
         t.queue_activity_at, t.deleted
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.id LIKE ? || '%' AND (? IS NULL OR t.workspace_id = ?)
         ORDER BY t.id",
    )
    .bind(suffix)
    .bind(workspace_id)
    .bind(workspace_id)
    .fetch_all(&mut *conn)
    .await?;
    let matches = rows
        .into_iter()
        .map(|row| crate::db::task_from_row(&row))
        .collect::<Result<Vec<_>>>()?;
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
        display_suffix_for_workspace(conn, &task.workspace_id, &task.id).await?
    ))
}

pub(crate) async fn display_refs_for_tasks(
    conn: &mut SqliteConnection,
    tasks: &[Task],
) -> Result<HashMap<String, String>> {
    let mut by_workspace = HashMap::<String, Vec<String>>::new();
    for task in tasks {
        by_workspace
            .entry(task.workspace_id.clone())
            .or_default()
            .push(task.id.clone());
    }
    for (workspace_id, ids) in &mut by_workspace {
        *ids = task_ids(conn, workspace_id).await?;
    }
    Ok(tasks
        .iter()
        .map(|task| {
            let suffix = display_suffix_for_id(&task.id, &by_workspace[&task.workspace_id]);
            (task.id.clone(), format!("{}-{suffix}", task.project_prefix))
        })
        .collect())
}

pub(crate) async fn display_suffix(conn: &mut SqliteConnection, id: &str) -> Result<String> {
    let workspace_id = crate::workspaces::active_workspace_id();
    display_suffix_for_workspace(conn, &workspace_id, id).await
}

async fn display_suffix_for_workspace(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    id: &str,
) -> Result<String> {
    let ids = task_ids(conn, workspace_id).await?;
    Ok(display_suffix_for_id(id, &ids))
}

async fn task_ids(conn: &mut SqliteConnection, workspace_id: &str) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT id FROM tasks WHERE workspace_id = ? ORDER BY id")
            .bind(workspace_id)
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
