use std::collections::{HashMap, HashSet};

use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

const SQLITE_BIND_CHUNK_SIZE: usize = 900;

pub(crate) struct TaskEnrichment {
    pub(crate) labels_by_task: HashMap<String, Vec<String>>,
    pub(crate) conflicted_task_ids: HashSet<String>,
}

pub(crate) async fn load_task_enrichment(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
) -> Result<TaskEnrichment> {
    Ok(TaskEnrichment {
        labels_by_task: labels_for_tasks(conn, workspace_id, task_ids).await?,
        conflicted_task_ids: tasks_with_unresolved_conflicts(conn, workspace_id, task_ids).await?,
    })
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
        let mut separated = query.separated(", ");
        for task_id in chunk {
            separated.push_bind(task_id);
        }
        drop(separated);
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
        let mut separated = query.separated(", ");
        for task_id in chunk {
            separated.push_bind(task_id);
        }
        drop(separated);
        query.push(")");

        for row in query.build().fetch_all(&mut *conn).await? {
            conflicted.insert(row.get("task_id"));
        }
    }
    Ok(conflicted)
}
