use crate::ids::{TaskId, WorkspaceId};
use std::collections::HashMap;

use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::render::quote;
use crate::types::Task;
use crate::workspaces::Workspace;

const DISPLAY_SUFFIX_FLOOR: usize = 4;

pub(crate) struct DisplayRefContext {
    task_ids_by_workspace: HashMap<WorkspaceId, Vec<TaskId>>,
}

impl DisplayRefContext {
    pub(crate) async fn for_workspace(
        conn: &mut SqliteConnection,
        workspace_id: &WorkspaceId,
    ) -> Result<Self> {
        Self::load(conn, std::slice::from_ref(workspace_id)).await
    }

    async fn load(conn: &mut SqliteConnection, workspace_ids: &[WorkspaceId]) -> Result<Self> {
        let mut task_ids_by_workspace = HashMap::with_capacity(workspace_ids.len());
        for workspace_id in workspace_ids {
            task_ids_by_workspace.insert(workspace_id.clone(), task_ids(conn, workspace_id).await?);
        }
        Ok(Self {
            task_ids_by_workspace,
        })
    }

    pub(crate) fn display_ref(&self, task: &Task) -> String {
        self.display_ref_for_id(&task.workspace_id, &task.project_prefix, &task.id)
    }

    pub(crate) fn display_ref_for_id(
        &self,
        workspace_id: &WorkspaceId,
        project_prefix: &str,
        id: &TaskId,
    ) -> String {
        format!(
            "{}-{}",
            project_prefix,
            self.display_suffix(workspace_id, id)
        )
    }

    pub(crate) fn display_suffix(&self, workspace_id: &WorkspaceId, id: &TaskId) -> String {
        let ids = self
            .task_ids_by_workspace
            .get(workspace_id)
            .map(Vec::as_slice)
            .unwrap_or_default();
        display_suffix_for_id(id, ids)
    }
}

#[allow(dead_code)]
pub(crate) async fn get_task_in_workspace(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    id: &TaskId,
) -> Result<Task> {
    get_task_scoped(conn, &workspace.id, id).await
}

async fn get_task_scoped(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    id: &TaskId,
) -> Result<Task> {
    let row = sqlx::query(
        "SELECT t.id, t.workspace_id, t.title, t.description, t.project_id,
         p.key AS project_key, p.prefix AS project_prefix, t.status, t.priority, t.created_at, t.updated_at,
         t.queue_activity_at, t.available_at, t.due_on, t.deleted, t.is_epic
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.workspace_id = ? AND t.id = ?",
    )
    .bind(workspace_id)
    .bind(id)
    .fetch_one(&mut *conn)
    .await?;
    crate::db::task_from_row(&row)
}

#[allow(dead_code)]
pub(crate) async fn resolve_task_ref_in_workspace(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    input: &str,
) -> Result<Task> {
    resolve_task_ref_scoped(conn, &workspace.id, input).await
}

async fn resolve_task_ref_scoped(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
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
         t.queue_activity_at, t.available_at, t.due_on, t.deleted, t.is_epic
         FROM tasks t JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
         WHERE t.workspace_id = ? AND t.id LIKE ? || '%'
         ORDER BY t.id",
    )
    .bind(workspace_id)
    .bind(suffix)
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
    let display_refs = DisplayRefContext::for_workspace(conn, workspace_id).await?;
    println!("error ambiguous-ref input={}", input);
    for task in matches {
        println!(
            "match {} title={}",
            display_refs.display_ref(&task),
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

async fn task_ids(conn: &mut SqliteConnection, workspace_id: &WorkspaceId) -> Result<Vec<TaskId>> {
    Ok(
        sqlx::query_scalar::<_, TaskId>("SELECT id FROM tasks WHERE workspace_id = ? ORDER BY id")
            .bind(workspace_id)
            .fetch_all(&mut *conn)
            .await?,
    )
}

fn display_suffix_for_id(id: &TaskId, ids: &[TaskId]) -> String {
    let len = display_suffix_len(id, ids);
    id.as_str()[..len].to_string()
}

fn display_suffix_len(id: &TaskId, ids: &[TaskId]) -> usize {
    let id = id.as_str();
    let floor = DISPLAY_SUFFIX_FLOOR.min(id.len());
    let index = ids.partition_point(|candidate| candidate.as_str() < id);
    let exact_index = ids
        .get(index)
        .filter(|candidate| candidate.as_str() == id)
        .map(|_| index);
    let insertion_index = exact_index.unwrap_or(index);
    let previous = insertion_index
        .checked_sub(1)
        .and_then(|previous| ids.get(previous));
    let next = ids.get(insertion_index + usize::from(exact_index.is_some()));
    let shared = previous
        .into_iter()
        .chain(next)
        .map(|candidate| common_prefix_len(id, candidate))
        .max()
        .unwrap_or(0);
    floor.max(shared.saturating_add(1).min(id.len()))
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    left.bytes()
        .zip(right.bytes())
        .take_while(|(left, right)| left == right)
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn context_reuses_loaded_workspace_ids() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace_id = crate::workspaces::default_workspace_id();
        let project_id = "PROJECTREFCACHE1";
        sqlx::query(
            "INSERT INTO projects(
                workspace_id, id, key, name, prefix, created_at, updated_at
             ) VALUES (?, ?, 'refs', 'Refs', 'REF', 't', 't')",
        )
        .bind(&workspace_id)
        .bind(project_id)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tasks(
                workspace_id, id, title, description, project_id, status, priority,
                created_at, updated_at, queue_activity_at
             ) VALUES (?, 'ABCD000000000000', 'original', '', ?, 'todo', 'none', 't', 't', 't')",
        )
        .bind(&workspace_id)
        .bind(project_id)
        .execute(&mut *conn)
        .await
        .unwrap();

        let display_refs = DisplayRefContext::for_workspace(&mut conn, &workspace_id)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tasks(
                workspace_id, id, title, description, project_id, status, priority,
                created_at, updated_at, queue_activity_at
             ) VALUES (?, 'ABCD100000000000', 'collider', '', ?, 'todo', 'none', 't', 't', 't')",
        )
        .bind(&workspace_id)
        .bind(project_id)
        .execute(&mut *conn)
        .await
        .unwrap();

        assert_eq!(
            display_refs.display_suffix(&workspace_id, &"ABCD000000000000".parse().unwrap(),),
            "ABCD"
        );
        let refreshed = DisplayRefContext::for_workspace(&mut conn, &workspace_id)
            .await
            .unwrap();
        assert_eq!(
            refreshed.display_suffix(&workspace_id, &"ABCD000000000000".parse().unwrap(),),
            "ABCD0"
        );
    }

    #[test]
    fn sorted_neighbors_determine_unique_display_suffix() {
        let ids: Vec<TaskId> = ["ABCD000000000000", "ABCD100000000000", "ABCE000000000000"]
            .into_iter()
            .map(|id| id.parse().unwrap())
            .collect();

        assert_eq!(display_suffix_for_id(&ids[0], &ids), "ABCD0");
        assert_eq!(display_suffix_for_id(&ids[1], &ids), "ABCD1");
        assert_eq!(display_suffix_for_id(&ids[2], &ids), "ABCE");
    }
}
