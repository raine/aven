use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};
use serde_json::to_value;
use sqlx::{Row, SqliteConnection};

use crate::change_log::op_type;
use crate::db::{Database, IdentifiedChange, begin_immediate, insert_change_with_identity};
use crate::ids::{TaskId, new_id, now};
use crate::metadata::TaskMetadataInput;
use crate::sync::wire::{
    ChangeWire, MAX_MOVE_TASKS, MoveDependencySnapshot, MoveEpicSnapshot, MoveRelatedSnapshot,
    MoveTaskMetadataSnapshot, MoveTaskSnapshot, MoveTasksPayload,
};
use crate::workspaces::Workspace;

#[derive(Debug, Clone)]
pub struct MoveTasksInput {
    pub task_ids: Vec<TaskId>,
    pub target_workspace: Workspace,
    pub target_project: String,
}

#[derive(Debug, Clone)]
pub struct MoveTasksOutcome {
    pub task_ids: Vec<TaskId>,
    pub target_project_key: String,
}

impl Database {
    pub async fn move_tasks(
        &self,
        source_workspace: &Workspace,
        input: MoveTasksInput,
    ) -> Result<MoveTasksOutcome> {
        let mut conn = self.acquire_writer().await?;
        let mut tx = begin_immediate(&mut conn).await?;
        let outcome = move_tasks_in_transaction(&mut tx, source_workspace, input).await;
        match outcome {
            Ok(outcome) => {
                tx.commit().await?;
                Ok(outcome)
            }
            Err(error) => {
                tx.rollback().await?;
                Err(error)
            }
        }
    }
}

async fn move_tasks_in_transaction(
    conn: &mut SqliteConnection,
    source_workspace: &Workspace,
    input: MoveTasksInput,
) -> Result<MoveTasksOutcome> {
    ensure!(
        source_workspace.id != input.target_workspace.id,
        "error move-same-workspace"
    );
    let target_project = crate::projects::resolve_existing_project_in_workspace(
        conn,
        &input.target_workspace.id,
        &input.target_project,
    )
    .await?;
    let task_ids = expand_and_validate_members(conn, source_workspace, input.task_ids).await?;
    let (dependencies, related, epics) =
        relationship_snapshots(conn, source_workspace, &task_ids).await?;
    let task_snapshots =
        task_snapshots(conn, source_workspace, &input.target_workspace, &task_ids).await?;
    let payload = MoveTasksPayload {
        source_workspace_id: source_workspace.id.clone(),
        target_workspace_id: input.target_workspace.id.clone(),
        project_id: target_project.id.clone(),
        project_key: target_project.key.clone(),
        project_name: target_project.name.clone(),
        project_prefix: target_project.prefix.clone(),
        tasks: task_snapshots,
        dependencies,
        related,
        epics,
    };
    let payload_value = to_value(&payload)?;
    let group_id = new_id();
    let change_id = new_id();
    let created_at = now();
    let validation_change = ChangeWire {
        change_id: change_id.clone(),
        client_id: "local".to_string(),
        local_seq: 1,
        entity_type: "task_move".to_string(),
        entity_id: group_id.clone(),
        field: None,
        op_type: op_type::MOVE_TASKS.to_string(),
        payload: payload_value.clone(),
        base_version: None,
        created_at: created_at.clone(),
        server_seq: None,
    };
    crate::sync::wire::validate_local_change_shape(&validation_change)?;

    insert_change_with_identity(
        conn,
        IdentifiedChange {
            change_id: &change_id,
            entity_type: "task_move",
            entity_id: &group_id,
            field: None,
            op_type: op_type::MOVE_TASKS,
            payload: payload_value,
            base_version: None,
            created_at: &created_at,
        },
    )
    .await?;
    crate::sync::apply::apply_remote_change(conn, &validation_change).await?;

    Ok(MoveTasksOutcome {
        task_ids,
        target_project_key: target_project.key,
    })
}

async fn expand_and_validate_members(
    conn: &mut SqliteConnection,
    source_workspace: &Workspace,
    requested: Vec<TaskId>,
) -> Result<Vec<TaskId>> {
    if requested.is_empty() {
        bail!("error move-task-required");
    }
    let mut members = requested.into_iter().collect::<BTreeSet<_>>();
    loop {
        let mut added = false;
        for task_id in members.iter().cloned().collect::<Vec<_>>() {
            let is_epic: Option<i64> =
                sqlx::query_scalar("SELECT is_epic FROM tasks WHERE workspace_id = ? AND id = ?")
                    .bind(&source_workspace.id)
                    .bind(&task_id)
                    .fetch_optional(&mut *conn)
                    .await?;
            let Some(is_epic) = is_epic else {
                bail!("error task-not-found task_id={task_id}");
            };
            if is_epic != 0 {
                let children: Vec<TaskId> = sqlx::query_scalar(
                    "SELECT child_task_id FROM task_epic_links
                     WHERE workspace_id = ? AND epic_task_id = ?",
                )
                .bind(&source_workspace.id)
                .bind(&task_id)
                .fetch_all(&mut *conn)
                .await?;
                for child in children {
                    added |= members.insert(child);
                }
            }
        }
        if !added {
            break;
        }
    }
    if members.len() > MAX_MOVE_TASKS {
        bail!("error move-too-many-tasks limit={MAX_MOVE_TASKS}");
    }
    for task_id in &members {
        let recurrence: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM recurrence_occurrences WHERE workspace_id = ? AND task_id = ?)",
        )
        .bind(&source_workspace.id)
        .bind(task_id)
        .fetch_one(&mut *conn)
        .await?;
        if recurrence {
            bail!("error move-recurring-task task_id={task_id}");
        }
        let conflicted: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM conflicts
             WHERE workspace_id = ? AND entity_type = 'task' AND entity_id = ? AND resolved = 0)",
        )
        .bind(&source_workspace.id)
        .bind(task_id)
        .fetch_one(&mut *conn)
        .await?;
        if conflicted {
            bail!("error move-conflicted-task task_id={task_id}");
        }
    }
    Ok(members.into_iter().collect())
}

async fn task_snapshots(
    conn: &mut SqliteConnection,
    source_workspace: &Workspace,
    target_workspace: &Workspace,
    task_ids: &[TaskId],
) -> Result<Vec<MoveTaskSnapshot>> {
    let mut labels_by_task =
        crate::task_enrichment::labels_for_tasks(conn, &source_workspace.id, task_ids).await?;
    let mut metadata_by_task =
        crate::metadata::metadata_by_task_ids(conn, &source_workspace.id, task_ids).await?;
    let mut snapshots = Vec::with_capacity(task_ids.len());
    for task_id in task_ids {
        let labels = labels_by_task.remove(task_id).unwrap_or_default();
        for label in &labels {
            crate::operations::create_label_operation(conn, target_workspace, label).await?;
        }
        let source_metadata = metadata_by_task.remove(task_id).unwrap_or_default();
        let mut metadata = Vec::with_capacity(source_metadata.len());
        for value in source_metadata {
            let version =
                crate::db::field_version(conn, task_id, &format!("metadata:{}", value.field_id))
                    .await?;
            let inputs = [TaskMetadataInput {
                expected_field_id: None,
                key: value.key,
                value: value.value,
            }];
            let resolved =
                crate::metadata::resolve_metadata_inputs(conn, target_workspace, &inputs).await?;
            let value = resolved.into_iter().next().expect("one metadata input");
            metadata.push(MoveTaskMetadataSnapshot {
                field_id: value.field_id,
                key: value.key,
                value: Some(value.value),
                version,
            });
        }
        let versions: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT m.id, m.key, f.version FROM field_versions f
             JOIN metadata_fields m ON m.workspace_id = f.workspace_id AND f.field = 'metadata:' || m.id
             WHERE f.entity_type = 'task' AND f.entity_id = ? AND f.workspace_id = ?")
            .bind(task_id).bind(&source_workspace.id).fetch_all(&mut *conn).await?;
        for (_, key, version) in versions {
            if metadata.iter().any(|value| value.key == key) {
                continue;
            }
            let resolved = crate::metadata::resolve_metadata_inputs(
                conn,
                target_workspace,
                &[TaskMetadataInput {
                    expected_field_id: None,
                    key,
                    value: String::new(),
                }],
            )
            .await?;
            let value = resolved.into_iter().next().expect("one metadata input");
            metadata.push(MoveTaskMetadataSnapshot {
                field_id: value.field_id,
                key: value.key,
                value: None,
                version: Some(version),
            });
        }
        metadata.sort_by(|a, b| a.key.cmp(&b.key));
        snapshots.push(MoveTaskSnapshot {
            task_id: task_id.clone(),
            labels,
            metadata,
        });
    }
    Ok(snapshots)
}

type RelationshipSnapshots = (
    Vec<MoveDependencySnapshot>,
    Vec<MoveRelatedSnapshot>,
    Vec<MoveEpicSnapshot>,
);

async fn relationship_snapshots(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    task_ids: &[TaskId],
) -> Result<RelationshipSnapshots> {
    let members = task_ids.iter().cloned().collect::<BTreeSet<_>>();
    let mut dependencies = BTreeMap::new();
    let mut related = BTreeMap::new();
    let mut epics = BTreeMap::new();
    for task_id in task_ids {
        let rows = sqlx::query(
            "SELECT task_id, depends_on_task_id, created_at FROM task_dependencies
             WHERE workspace_id = ? AND (task_id = ? OR depends_on_task_id = ?)",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(task_id)
        .fetch_all(&mut *conn)
        .await?;
        for row in rows {
            let task_id: TaskId = row.get("task_id");
            let depends_on_task_id: TaskId = row.get("depends_on_task_id");
            if !members.contains(&task_id) || !members.contains(&depends_on_task_id) {
                bail!(
                    "error move-boundary-dependency task_id={task_id} depends_on_task_id={depends_on_task_id} hint=remove-link-or-include-task"
                );
            }
            dependencies
                .entry((task_id.clone(), depends_on_task_id.clone()))
                .or_insert(MoveDependencySnapshot {
                    task_id,
                    depends_on_task_id,
                    created_at: row.get("created_at"),
                });
        }
        let rows = sqlx::query(
            "SELECT task_a_id, task_b_id, linked, last_change_id FROM task_related_links
             WHERE workspace_id = ? AND (task_a_id = ? OR task_b_id = ?)",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(task_id)
        .fetch_all(&mut *conn)
        .await?;
        for row in rows {
            let task_a_id: TaskId = row.get("task_a_id");
            let task_b_id: TaskId = row.get("task_b_id");
            if !members.contains(&task_a_id) || !members.contains(&task_b_id) {
                if row.get::<i64, _>("linked") == 0 {
                    continue;
                }
                bail!(
                    "error move-boundary-related task_id={task_a_id} related_task_id={task_b_id} hint=remove-link-or-include-task"
                );
            }
            related
                .entry((task_a_id.clone(), task_b_id.clone()))
                .or_insert(MoveRelatedSnapshot {
                    task_a_id,
                    task_b_id,
                    linked: row.get::<i64, _>("linked") != 0,
                    last_change_id: row.get("last_change_id"),
                });
        }
        let rows = sqlx::query(
            "SELECT child_task_id, epic_task_id, created_at FROM task_epic_links
             WHERE workspace_id = ? AND (child_task_id = ? OR epic_task_id = ?)",
        )
        .bind(&workspace.id)
        .bind(task_id)
        .bind(task_id)
        .fetch_all(&mut *conn)
        .await?;
        for row in rows {
            let child_task_id: TaskId = row.get("child_task_id");
            let epic_task_id: TaskId = row.get("epic_task_id");
            if !members.contains(&child_task_id) || !members.contains(&epic_task_id) {
                bail!(
                    "error move-boundary-epic child_task_id={child_task_id} epic_task_id={epic_task_id} hint=move-the-epic-or-remove-link"
                );
            }
            epics
                .entry((child_task_id.clone(), epic_task_id.clone()))
                .or_insert(MoveEpicSnapshot {
                    child_task_id,
                    epic_task_id,
                    created_at: row.get("created_at"),
                });
        }
    }
    Ok((
        dependencies.into_values().collect(),
        related.into_values().collect(),
        epics.into_values().collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::choices::TaskSource;
    use crate::metadata::TaskMetadataInput;
    use crate::operations::{TaskDraft, TaskUpdate};

    fn draft(title: &str, project: &str, labels: Vec<String>, is_epic: bool) -> TaskDraft {
        TaskDraft {
            title: title.to_string(),
            description: "description".to_string(),
            project: Some(project.to_string()),
            status: "todo".to_string(),
            priority: "none".to_string(),
            source: TaskSource::Cli,
            labels,
            metadata: vec![TaskMetadataInput {
                expected_field_id: None,
                key: "owner".to_string(),
                value: "Raine".to_string(),
            }],
            available_at: None,
            due_on: None,
            is_epic,
        }
    }

    async fn setup() -> (tempfile::TempDir, Database, Workspace, Workspace) {
        let temp = tempfile::tempdir().unwrap();
        let database = Database::open(&temp.path().join("aven.sqlite"))
            .await
            .unwrap();
        let source = database.list_workspaces().await.unwrap().remove(0);
        let target = database.create_workspace("Target").await.unwrap();
        database
            .create_project(&source, "Source", None)
            .await
            .unwrap();
        database
            .create_project(&target, "Target", None)
            .await
            .unwrap();
        (temp, database, source, target)
    }

    #[tokio::test]
    async fn move_preserves_task_owned_data_and_import_history() {
        let (_temp, database, source, target) = setup().await;
        database.create_label(&source, "important").await.unwrap();
        let task = database
            .create_task(
                &source,
                draft("Move me", "Source", vec!["important".to_string()], false),
            )
            .await
            .unwrap()
            .task;
        database
            .add_note(&source, &task.id, "keep this note".to_string())
            .await
            .unwrap();

        let outcome = database
            .move_tasks(
                &source,
                MoveTasksInput {
                    task_ids: vec![task.id.clone()],
                    target_workspace: target.clone(),
                    target_project: "Target".to_string(),
                },
            )
            .await
            .unwrap();
        assert_eq!(outcome.task_ids, vec![task.id.clone()]);

        let mut conn = database.acquire_reader().await.unwrap();
        let row: (String, String) = sqlx::query_as(
            "SELECT t.workspace_id, p.key FROM tasks t
             JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
             WHERE t.id = ?",
        )
        .bind(&task.id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert_eq!(row, (target.id.to_string(), "target".to_string()));
        for (table, query) in [
            (
                "notes",
                "SELECT count(*) FROM notes WHERE workspace_id = ? AND task_id = ?",
            ),
            (
                "task_labels",
                "SELECT count(*) FROM task_labels WHERE workspace_id = ? AND task_id = ?",
            ),
            (
                "task_metadata",
                "SELECT count(*) FROM task_metadata WHERE workspace_id = ? AND task_id = ?",
            ),
        ] {
            let count: i64 = sqlx::query_scalar(query)
                .bind(&target.id)
                .bind(&task.id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
            assert_eq!(count, 1, "{table}");
        }
        let protocol = crate::db::get_meta(&mut conn, "sync_established_protocol")
            .await
            .unwrap();
        assert_eq!(protocol, None);
        let history: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_workspace_history WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(history, 2);
        drop(conn);

        let export = database.export_data(crate::ids::now()).await.unwrap();
        let imported_temp = tempfile::tempdir().unwrap();
        let imported = Database::open(&imported_temp.path().join("import.sqlite"))
            .await
            .unwrap();
        imported.import_data(&export).await.unwrap();
        let mut imported_conn = imported.acquire_reader().await.unwrap();
        let imported_workspace: String =
            sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = ?")
                .bind(&task.id)
                .fetch_one(&mut *imported_conn)
                .await
                .unwrap();
        assert_eq!(imported_workspace, target.id.to_string());
        let imported_history: i64 =
            sqlx::query_scalar("SELECT count(*) FROM task_workspace_history WHERE task_id = ?")
                .bind(&task.id)
                .fetch_one(&mut *imported_conn)
                .await
                .unwrap();
        assert_eq!(imported_history, 2);
    }

    #[tokio::test]
    async fn boundary_relationship_rejects_the_entire_move() {
        let (_temp, database, source, target) = setup().await;
        let first = database
            .create_task(&source, draft("First", "Source", Vec::new(), false))
            .await
            .unwrap()
            .task;
        let second = database
            .create_task(&source, draft("Second", "Source", Vec::new(), false))
            .await
            .unwrap()
            .task;
        database
            .add_task_dependency(&source, &first.id, &second.id)
            .await
            .unwrap();

        let error = database
            .move_tasks(
                &source,
                MoveTasksInput {
                    task_ids: vec![first.id.clone()],
                    target_workspace: target,
                    target_project: "Target".to_string(),
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("move-boundary-dependency"));
        let mut conn = database.acquire_reader().await.unwrap();
        let workspace_id: String =
            sqlx::query_scalar("SELECT workspace_id FROM tasks WHERE id = ?")
                .bind(&first.id)
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert_eq!(workspace_id, source.id.to_string());
        assert_eq!(
            crate::db::get_meta(&mut conn, "sync_established_protocol")
                .await
                .unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn moving_an_epic_includes_soft_deleted_descendants() {
        let (_temp, database, source, target) = setup().await;
        let epic = database
            .create_task(&source, draft("Epic", "Source", Vec::new(), true))
            .await
            .unwrap()
            .task;
        let child = database
            .create_task(&source, draft("Child", "Source", Vec::new(), false))
            .await
            .unwrap()
            .task;
        database
            .add_task_to_epic(&source, &child.id, &epic.id)
            .await
            .unwrap();
        database
            .update_task(
                &source,
                &child.id,
                TaskUpdate {
                    deleted: Some(true),
                    ..TaskUpdate::default()
                },
            )
            .await
            .unwrap();

        let outcome = database
            .move_tasks(
                &source,
                MoveTasksInput {
                    task_ids: vec![epic.id.clone()],
                    target_workspace: target.clone(),
                    target_project: "Target".to_string(),
                },
            )
            .await
            .unwrap();
        let mut expected = vec![child.id.clone(), epic.id.clone()];
        expected.sort();
        assert_eq!(outcome.task_ids, expected);
        let mut conn = database.acquire_reader().await.unwrap();
        let moved: Vec<(String, i64)> = sqlx::query_as(
            "SELECT workspace_id, deleted FROM tasks WHERE id IN (?, ?) ORDER BY id",
        )
        .bind(&child.id)
        .bind(&epic.id)
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(moved.len(), 2);
        assert!(
            moved
                .iter()
                .all(|(workspace_id, _)| workspace_id == target.id.as_str())
        );
        assert!(moved.iter().any(|(_, deleted)| *deleted == 1));
    }
    #[tokio::test]
    async fn removed_related_boundary_allows_move_and_internal_link_imports() {
        let (_temp, db, source, target) = setup().await;
        let a = db
            .create_task(&source, draft("A", "Source", vec![], false))
            .await
            .unwrap()
            .task;
        let b = db
            .create_task(&source, draft("B", "Source", vec![], false))
            .await
            .unwrap()
            .task;
        db.add_task_related_link(&source, &a.id, &b.id)
            .await
            .unwrap();
        db.remove_task_related_link(&source, &a.id, &b.id)
            .await
            .unwrap();
        db.move_tasks(
            &source,
            MoveTasksInput {
                task_ids: vec![a.id.clone()],
                target_workspace: target.clone(),
                target_project: "Target".into(),
            },
        )
        .await
        .unwrap();
        db.move_tasks(
            &target,
            MoveTasksInput {
                task_ids: vec![a.id.clone()],
                target_workspace: source.clone(),
                target_project: "Source".into(),
            },
        )
        .await
        .unwrap();
        db.add_task_related_link(&source, &a.id, &b.id)
            .await
            .unwrap();
        db.move_tasks(
            &source,
            MoveTasksInput {
                task_ids: vec![a.id, b.id],
                target_workspace: target,
                target_project: "Target".into(),
            },
        )
        .await
        .unwrap();
        let export = db.export_data("now".into()).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let imported = Database::open(&dir.path().join("import.sqlite"))
            .await
            .unwrap();
        imported.import_data(&export).await.unwrap();
    }

    #[tokio::test]
    async fn oversized_snapshot_refuses_without_creating_destination_fields() {
        let (_temp, db, source, target) = setup().await;
        let mut ids = Vec::new();
        for title in ["A", "B"] {
            let mut d = draft(title, "Source", vec![], false);
            d.metadata = (0..8)
                .map(|n| TaskMetadataInput {
                    expected_field_id: None,
                    key: format!("field-{n}"),
                    value: "x".repeat(4096),
                })
                .collect();
            ids.push(db.create_task(&source, d).await.unwrap().task.id);
        }
        let before = db.export_data("before".into()).await.unwrap();
        let error = db
            .move_tasks(
                &source,
                MoveTasksInput {
                    task_ids: ids,
                    target_workspace: target.clone(),
                    target_project: "Target".into(),
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("payload-too-large"));
        let after = db.export_data("after".into()).await.unwrap();
        assert_eq!(before.tables.changes.len(), after.tables.changes.len());
        assert_eq!(
            before.tables.metadata_fields.len(),
            after.tables.metadata_fields.len()
        );
        assert!(
            after
                .tables
                .tasks
                .iter()
                .all(|t| t.workspace_id == source.id)
        );
    }

    #[tokio::test]
    async fn removed_metadata_versions_do_not_consume_live_field_capacity() {
        let (_temp, db, source, target) = setup().await;
        let mut d = draft("A", "Source", vec![], false);
        d.metadata.clear();
        let task = db.create_task(&source, d).await.unwrap().task;
        for n in 0..129 {
            let key = format!("field-{n}");
            db.update_task(
                &source,
                &task.id,
                TaskUpdate {
                    set_metadata: vec![TaskMetadataInput {
                        expected_field_id: None,
                        key: key.clone(),
                        value: "value".into(),
                    }],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            db.update_task(
                &source,
                &task.id,
                TaskUpdate {
                    remove_metadata: vec![key],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        assert!(
            db.task_metadata(&source.id, &task.id)
                .await
                .unwrap()
                .is_empty()
        );
        db.move_tasks(
            &source,
            MoveTasksInput {
                task_ids: vec![task.id.clone()],
                target_workspace: target.clone(),
                target_project: "Target".into(),
            },
        )
        .await
        .unwrap();
        let values = db.task_metadata(&target.id, &task.id).await.unwrap();
        assert!(values.is_empty(), "values={values:?}");
        let snapshot = db.export_data("now".into()).await.unwrap();
        let moved = snapshot
            .tables
            .changes
            .iter()
            .find(|c| c.op_type == "move_tasks")
            .unwrap();
        let payload: serde_json::Value = serde_json::from_str(&moved.payload).unwrap();
        assert_eq!(
            payload["tasks"][0]["metadata"].as_array().unwrap().len(),
            129
        );
    }
}
