use std::collections::{HashMap, HashSet};

use crate::query::fragments;
use crate::query::{TaskDependencyLink, TaskNote};
use crate::refs::display_ref_for_id;
use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

const SQLITE_BIND_CHUNK_SIZE: usize = 900;

pub(crate) struct TaskEnrichment {
    pub(crate) labels_by_task: HashMap<String, Vec<String>>,
    pub(crate) notes_by_task: HashMap<String, Vec<TaskNote>>,
    pub(crate) conflicted_task_ids: HashSet<String>,
    pub(crate) unresolved_blocker_counts_by_task: HashMap<String, i64>,
    pub(crate) dependent_counts_by_task: HashMap<String, i64>,
    pub(crate) depends_on_by_task: HashMap<String, Vec<TaskDependencyLink>>,
    pub(crate) blocks_by_task: HashMap<String, Vec<TaskDependencyLink>>,
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
        unresolved_blocker_counts_by_task: unresolved_blocker_counts_for_tasks(
            conn,
            workspace_id,
            task_ids,
        )
        .await?,
        dependent_counts_by_task: dependent_counts_for_tasks(conn, workspace_id, task_ids).await?,
        depends_on_by_task: dependency_links_for_tasks(conn, workspace_id, task_ids, false).await?,
        blocks_by_task: dependency_links_for_tasks(conn, workspace_id, task_ids, true).await?,
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

async fn unresolved_blocker_counts_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
) -> Result<HashMap<String, i64>> {
    let mut counts = HashMap::new();
    if task_ids.is_empty() {
        return Ok(counts);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT d.task_id, COUNT(*) AS blockers
             FROM task_dependencies d
             JOIN tasks blocker
              ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
             WHERE d.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND d.task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(format!(
            ") AND {} GROUP BY d.task_id",
            fragments::open_task_clause("blocker"),
        ));

        for row in query.build().fetch_all(&mut *conn).await? {
            counts.insert(row.get("task_id"), row.get::<i64, _>("blockers"));
        }
    }
    Ok(counts)
}

async fn dependent_counts_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
) -> Result<HashMap<String, i64>> {
    let mut counts = HashMap::new();
    if task_ids.is_empty() {
        return Ok(counts);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT d.depends_on_task_id, COUNT(*) AS dependents
             FROM task_dependencies d
             JOIN tasks blocker
              ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
             JOIN tasks dependent
              ON dependent.workspace_id = d.workspace_id AND dependent.id = d.task_id
             WHERE d.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND d.depends_on_task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(format!(
            ") AND {} AND {} GROUP BY d.depends_on_task_id",
            fragments::open_task_clause("blocker"),
            fragments::open_task_clause("dependent"),
        ));

        for row in query.build().fetch_all(&mut *conn).await? {
            counts.insert(
                row.get("depends_on_task_id"),
                row.get::<i64, _>("dependents"),
            );
        }
    }
    Ok(counts)
}

async fn dependency_links_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_ids: &[String],
    blocks_only: bool,
) -> Result<HashMap<String, Vec<TaskDependencyLink>>> {
    let mut links = HashMap::new();
    if task_ids.is_empty() {
        return Ok(links);
    }
    let workspace_task_ids = workspace_task_ids(conn, workspace_id).await?;
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let initial = if blocks_only {
            format!(
                "SELECT d.depends_on_task_id AS source_task_id,
                        t.id, t.title, t.status, t.priority, p.prefix AS project_prefix,
                        d.created_at AS dependency_created_at,
                        CASE
                            WHEN {}
                             AND {}
                            THEN 1 ELSE 0
                        END AS unresolved
                 FROM task_dependencies d
                 JOIN tasks blocker
                  ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
                 JOIN tasks t
                  ON t.workspace_id = d.workspace_id AND t.id = d.task_id
                 JOIN projects p
                  ON p.workspace_id = t.workspace_id AND p.id = t.project_id
                 WHERE d.workspace_id =",
                fragments::open_task_clause("blocker"),
                fragments::open_task_clause("t"),
            )
        } else {
            format!(
                "SELECT d.task_id AS source_task_id,
                        t.id, t.title, t.status, t.priority, p.prefix AS project_prefix,
                        d.created_at AS dependency_created_at,
                        CASE
                            WHEN {}
                            THEN 1 ELSE 0
                        END AS unresolved
                 FROM task_dependencies d
                 JOIN tasks t
                  ON t.workspace_id = d.workspace_id AND t.id = d.depends_on_task_id
                 JOIN projects p
                  ON p.workspace_id = t.workspace_id AND p.id = t.project_id
                 WHERE d.workspace_id =",
                fragments::open_task_clause("t"),
            )
        };
        let mut query = QueryBuilder::<Sqlite>::new(&initial);
        query.push_bind(workspace_id);
        let source_column = if blocks_only {
            "d.depends_on_task_id"
        } else {
            "d.task_id"
        };
        query.push(" AND ");
        query.push(source_column);
        query.push(" IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY unresolved DESC, t.status, t.title, d.created_at, t.id");

        for row in query.build().fetch_all(&mut *conn).await? {
            let source_task_id: String = row.get("source_task_id");
            let task_id: String = row.get("id");
            let project_prefix: String = row.get("project_prefix");
            links
                .entry(source_task_id)
                .or_insert_with(Vec::new)
                .push(TaskDependencyLink {
                    task_id: task_id.clone(),
                    display_ref: display_ref_for_id(&project_prefix, &task_id, &workspace_task_ids),
                    title: row.get("title"),
                    status: row.get("status"),
                    priority: row.get("priority"),
                    unresolved: row.get::<i64, _>("unresolved") != 0,
                });
        }
    }
    Ok(links)
}

async fn workspace_task_ids(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT id FROM tasks WHERE workspace_id = ? ORDER BY id")
            .bind(workspace_id)
            .fetch_all(&mut *conn)
            .await?,
    )
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
