use std::collections::{HashMap, HashSet};

use crate::query::TaskNote;
use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

const SQLITE_BIND_CHUNK_SIZE: usize = 900;

pub(crate) struct TaskEnrichment {
    pub(crate) labels_by_task: HashMap<String, Vec<String>>,
    pub(crate) notes_by_task: HashMap<String, Vec<TaskNote>>,
    pub(crate) conflicted_task_ids: HashSet<String>,
}

pub(crate) async fn load_task_enrichment(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
) -> Result<TaskEnrichment> {
    Ok(TaskEnrichment {
        labels_by_task: labels_for_tasks(conn, workspace_id, task_ids).await?,
        notes_by_task: notes_for_tasks(conn, workspace_id, task_ids).await?,
        conflicted_task_ids: tasks_with_unresolved_conflicts(conn, workspace_id, task_ids).await?,
    })
}

async fn notes_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
) -> Result<HashMap<String, Vec<TaskNote>>> {
    let mut notes_by_task = HashMap::new();
    if task_ids.is_empty() {
        return Ok(notes_by_task);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT task_id, body, created_at FROM notes WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY task_id, created_at DESC, id DESC");

        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: String = row.get("task_id");
            let note = TaskNote {
                body: row.get("body"),
                created_at: row.get("created_at"),
            };
            notes_by_task
                .entry(task_id)
                .or_insert_with(Vec::new)
                .push(note);
        }
    }
    Ok(notes_by_task)
}

async fn labels_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
) -> Result<HashMap<String, Vec<String>>> {
    let mut labels_by_task = HashMap::new();
    if task_ids.is_empty() {
        return Ok(labels_by_task);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT task_id, label FROM task_labels WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY task_id, label");

        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: String = row.get("task_id");
            let label: String = row.get("label");
            labels_by_task
                .entry(task_id)
                .or_insert_with(Vec::new)
                .push(label);
        }
    }
    Ok(labels_by_task)
}

async fn tasks_with_unresolved_conflicts(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
) -> Result<HashSet<String>> {
    let mut conflicted = HashSet::new();
    if task_ids.is_empty() {
        return Ok(conflicted);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT DISTINCT task_id FROM conflicts WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND resolved = 0 AND task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(")");

        for row in query.build().fetch_all(&mut *conn).await? {
            conflicted.insert(row.get("task_id"));
        }
    }
    Ok(conflicted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_enrichment_loads_notes_across_bind_chunks() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace_id = crate::workspaces::active_workspace_id();
        let task_ids = (0..=SQLITE_BIND_CHUNK_SIZE)
            .map(|index| format!("task-{index:04}"))
            .collect::<Vec<_>>();

        sqlx::query(
            "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
             VALUES (?, 'note-first-old', 'task-0000', 'older', '001', 'change-first-old'),
                    (?, 'note-first-new', 'task-0000', 'newer', '002', 'change-first-new'),
                    (?, 'note-last', ?, 'last', '003', 'change-last')",
        )
        .bind(&workspace_id)
        .bind(&workspace_id)
        .bind(&workspace_id)
        .bind(task_ids.last().unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();

        let enrichment = load_task_enrichment(&mut conn, &workspace_id, &task_ids)
            .await
            .unwrap();

        assert_eq!(
            enrichment
                .notes_by_task
                .get("task-0000")
                .unwrap()
                .iter()
                .map(|note| note.body.as_str())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
        assert_eq!(
            enrichment
                .notes_by_task
                .get(task_ids.last().unwrap())
                .unwrap()[0]
                .body,
            "last"
        );
    }
}
