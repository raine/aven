use anyhow::Result;
use sqlx::{Row, SqliteConnection};

use crate::types::Task;

use super::{
    SortDirection, TaskDependencySummary, TaskFilters, TaskListItem, TaskQueryMode, TaskSort,
    list_task_items_in_workspace, task_dependency_summary,
};

#[derive(Debug)]
pub(crate) struct TaskDetail {
    pub(crate) item: TaskListItem,
    pub(crate) project_name: String,
    pub(crate) dependencies: TaskDependencySummary,
    pub(crate) notes: Vec<TaskDetailNote>,
    pub(crate) conflicts: Vec<TaskDetailConflict>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskDetailNote {
    pub(crate) id: String,
    pub(crate) body: String,
    pub(crate) created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskDetailConflict {
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) local_value: String,
    pub(crate) variant_b: String,
    pub(crate) remote_value: String,
}

pub(crate) async fn task_detail(conn: &mut SqliteConnection, task: &Task) -> Result<TaskDetail> {
    let item = list_task_items_in_workspace(
        conn,
        &task.workspace_id,
        TaskFilters {
            task_ids: vec![task.id.clone()],
            ..TaskFilters::default().include_deleted(task.deleted)
        },
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
    )
    .await?
    .into_iter()
    .next()
    .expect("task must exist after resolve");
    let project_name = sqlx::query_scalar::<_, String>(
        "SELECT name FROM projects WHERE workspace_id = ? AND id = ?",
    )
    .bind(&task.workspace_id)
    .bind(&task.project_id)
    .fetch_one(&mut *conn)
    .await?;
    let dependencies = task_dependency_summary(conn, &task.workspace_id, &task.id).await?;
    let notes = task_detail_notes(conn, &task.workspace_id, &task.id).await?;
    let conflicts = task_detail_conflicts(conn, &task.workspace_id, &task.id).await?;

    Ok(TaskDetail {
        item,
        project_name,
        dependencies,
        notes,
        conflicts,
    })
}

async fn task_detail_notes(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<Vec<TaskDetailNote>> {
    let rows = sqlx::query(
        "SELECT id, body, created_at FROM notes
         WHERE workspace_id = ? AND task_id = ? ORDER BY created_at, id",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| TaskDetailNote {
            id: row.get("id"),
            body: row.get("body"),
            created_at: row.get("created_at"),
        })
        .collect())
}

async fn task_detail_conflicts(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<Vec<TaskDetailConflict>> {
    let rows = sqlx::query(
        "SELECT field, variant_a, local_value, variant_b, remote_value
         FROM conflicts
         WHERE workspace_id = ? AND task_id = ? AND resolved = 0
         ORDER BY field, id",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| TaskDetailConflict {
            field: row.get("field"),
            variant_a: row.get("variant_a"),
            local_value: row.get("local_value"),
            variant_b: row.get("variant_b"),
            remote_value: row.get("remote_value"),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_detail_loads_enriched_fields_in_stable_order() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace = crate::workspaces::Workspace::default();
        let workspace_id = workspace.id.clone();
        sqlx::query(
            "INSERT INTO projects(id, key, name, prefix, created_at, updated_at)
             VALUES ('PROJECTDETAIL001', 'detail', 'Detail Project', 'DTL', 't', 't')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        for (id, title) in [
            ("D3TA100000000001", "detailed task"),
            ("D3TA100000000002", "blocker task"),
        ] {
            sqlx::query(
                "INSERT INTO tasks(id, title, description, project_id, status, priority, created_at, updated_at, queue_activity_at)
                 VALUES (?, ?, 'description', 'PROJECTDETAIL001', 'todo', 'high', 't', 't', 't')",
            )
            .bind(id)
            .bind(title)
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        sqlx::query(
            "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
             VALUES (?, 'note-b', 'D3TA100000000001', 'second', '002', 'change-b'),
                    (?, 'note-a', 'D3TA100000000001', 'first', '001', 'change-a')",
        )
        .bind(&workspace_id)
        .bind(&workspace_id)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at)
             VALUES (?, 'D3TA100000000001', 'D3TA100000000002', '003')",
        )
        .bind(&workspace_id)
        .execute(&mut *conn)
        .await
        .unwrap();
        for (field, resolved) in [("title", 0), ("priority", 1)] {
            sqlx::query(
                "INSERT INTO conflicts(workspace_id, task_id, field, local_value, remote_value,
                 remote_change_id, variant_a, variant_b, created_at, resolved)
                 VALUES (?, 'D3TA100000000001', ?, 'local', 'remote', ?, 'a', 'b', '004', ?)",
            )
            .bind(&workspace_id)
            .bind(field)
            .bind(format!("remote-{field}"))
            .bind(resolved)
            .execute(&mut *conn)
            .await
            .unwrap();
        }

        let task = crate::refs::resolve_task_ref_in_workspace(
            &mut conn,
            &workspace,
            "D3TA100000000001",
        )
        .await
        .unwrap();
        let detail = task_detail(&mut conn, &task).await.unwrap();

        assert_eq!(detail.item.task.title, "detailed task");
        assert_eq!(detail.project_name, "Detail Project");
        assert_eq!(
            detail
                .notes
                .iter()
                .map(|note| (note.id.as_str(), note.body.as_str()))
                .collect::<Vec<_>>(),
            [("note-a", "first"), ("note-b", "second")]
        );
        assert_eq!(detail.dependencies.depends_on.len(), 1);
        assert_eq!(detail.dependencies.depends_on[0].task.title, "blocker task");
        assert_eq!(detail.conflicts.len(), 1);
        assert_eq!(detail.conflicts[0].field, "title");
    }
}
