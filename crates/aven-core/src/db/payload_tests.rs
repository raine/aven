use super::*;
use crate::choices::TaskSource;
use crate::error::ErrorKind;
use crate::operations::{TaskCreationUndo, TaskDraft, TaskUpdate};
use crate::undo::UndoContext;

fn draft(description: String) -> TaskDraft {
    TaskDraft {
        title: "payload test".into(),
        description,
        project: Some("payload-tests".into()),
        status: "inbox".into(),
        priority: "none".into(),
        source: TaskSource::Unknown,
        labels: Vec::new(),
        metadata: Vec::new(),
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}

async fn snapshot(database: &Database) -> Vec<(String, Vec<String>)> {
    let mut conn = database.acquire_reader().await.unwrap();
    let mut snapshot = Vec::new();
    for table in [
        "tasks",
        "notes",
        "projects",
        "labels",
        "task_labels",
        "changes",
        "field_versions",
        "meta",
        "tui_undo_entries",
    ] {
        let columns: Vec<String> =
            sqlx::query(sqlx::AssertSqlSafe(format!("PRAGMA table_info({table})")))
                .fetch_all(&mut *conn)
                .await
                .unwrap()
                .iter()
                .map(|row| format!("\"{}\"", row.get::<String, _>("name")))
                .collect();
        let mut rows: Vec<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT json_array({}) FROM {table}",
            columns.join(",")
        )))
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        rows.sort();
        snapshot.push((table.into(), rows));
    }
    snapshot
}

fn assert_validation(error: anyhow::Error) {
    assert_eq!(
        error.downcast_ref::<CoreError>().unwrap().kind(),
        ErrorKind::Validation
    );
    assert!(error.to_string().contains("payload-too-large"));
}

#[tokio::test]
async fn oversized_note_mutations_roll_back() {
    let database = Database::open(Path::new(":memory:")).await.unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let task = database
        .create_task(&workspace, draft(String::new()))
        .await
        .unwrap()
        .task;
    let note = database
        .add_note_with_tui_undo(&workspace, &task.id, "original".into())
        .await
        .unwrap();
    let before = snapshot(&database).await;
    let body = "\"\\\n😀".repeat(10_000);
    let result = database
        .add_note_with_tui_undo(&workspace, &task.id, body.clone())
        .await;
    assert!(result.is_err(), "oversized note must be rejected");
    assert_validation(result.err().unwrap());
    assert_eq!(snapshot(&database).await, before);
    let result = database
        .edit_note_with_tui_undo(&workspace, &task.id, &note.note_id, body)
        .await;
    assert_validation(result.err().expect("oversized note edit must be rejected"));
    assert_eq!(snapshot(&database).await, before);
}

#[tokio::test]
async fn oversized_description_mutations_roll_back() {
    let database = Database::open(Path::new(":memory:")).await.unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let task = database
        .create_task(&workspace, draft("original".into()))
        .await
        .unwrap()
        .task;
    let other = database
        .create_task(&workspace, draft("other".into()))
        .await
        .unwrap()
        .task;
    let before = snapshot(&database).await;
    let description = "é\n\"".repeat(16_384);
    let result = database
        .mutate_tasks(
            &workspace,
            vec![
                (
                    other.id,
                    TaskUpdate {
                        title: Some("earlier batch write".into()),
                        ..TaskUpdate::default()
                    },
                ),
                (
                    task.id.clone(),
                    TaskUpdate {
                        title: Some("must roll back too".into()),
                        description: Some(description.clone()),
                        ..TaskUpdate::default()
                    },
                ),
            ],
            UndoContext::tui("edit"),
        )
        .await;
    assert!(result.is_err(), "oversized description must be rejected");
    assert_validation(result.err().unwrap());
    assert_eq!(snapshot(&database).await, before);
    let mut oversized_draft = draft(description);
    oversized_draft.project = Some("rolled-back-project".into());
    let result = database
        .create_task_with_undo(&workspace, oversized_draft, TaskCreationUndo::TuiTask)
        .await;
    assert_validation(result.expect_err("oversized task creation must be rejected"));
    assert_eq!(snapshot(&database).await, before);
}

#[tokio::test]
async fn payload_boundaries_match_wire_for_both_local_insertions() {
    use crate::sync::wire::validate_pushed_change;

    let database = Database::open(Path::new(":memory:")).await.unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let task = database
        .create_task(&workspace, draft(String::new()))
        .await
        .unwrap()
        .task;
    let note = database
        .add_note(&workspace, &task.id, String::new())
        .await
        .unwrap();
    let page = database
        .prepare_client_sync_page("http://localhost:3000".into(), 256, 512)
        .await
        .unwrap();
    let template = page
        .request
        .changes
        .into_iter()
        .find(|change| change.change_id == note.change_id)
        .unwrap();

    let mut description_template = template.clone();
    description_template.field = Some("description".into());
    description_template.op_type = crate::change_log::op_type::SET_FIELD.into();
    description_template.payload = crate::task_fields::TaskField::Description
        .scalar_payload(&workspace.id, &workspace.key, "")
        .unwrap();
    for (template, value_key) in [(template, "body"), (description_template, "value")] {
        for text in ["a", "\"\\\n\u{0001}", "é😀"] {
            for size in [65_535, 65_536, 65_537] {
                for identified in [false, true] {
                    let mut change = template.clone();
                    change.change_id = new_id();
                    let overhead = serde_json::to_vec(&change.payload).unwrap().len();
                    let unit_bytes = serde_json::to_vec(text).unwrap().len() - 2;
                    let body = text.repeat((size - overhead) / unit_bytes)
                        + &"a".repeat((size - overhead) % unit_bytes);
                    change.payload[value_key] = Value::String(body);
                    assert_eq!(serde_json::to_vec(&change.payload).unwrap().len(), size);
                    assert_eq!(validate_pushed_change(&change).is_ok(), size <= 65_536);
                    let before = snapshot(&database).await;
                    let mut conn = database.acquire_writer().await.unwrap();
                    let result = if identified {
                        insert_change_with_identity(
                            &mut conn,
                            IdentifiedChange {
                                change_id: &change.change_id,
                                entity_type: &change.entity_type,
                                entity_id: &change.entity_id,
                                field: change.field.as_deref(),
                                op_type: &change.op_type,
                                payload: change.payload.clone(),
                                base_version: change.base_version.as_deref(),
                                created_at: &change.created_at,
                            },
                        )
                        .await
                    } else {
                        insert_change(
                            &mut conn,
                            &change.entity_type,
                            &change.entity_id,
                            change.field.as_deref(),
                            &change.op_type,
                            change.payload.clone(),
                            change.base_version.as_deref(),
                        )
                        .await
                        .map(|_| ())
                    };
                    drop(conn);
                    if size > 65_536 {
                        assert_validation(result.unwrap_err());
                        assert_eq!(snapshot(&database).await, before);
                    } else {
                        result.unwrap();
                        let page = database
                            .prepare_client_sync_page("http://localhost:3000".into(), 256, 512)
                            .await
                            .unwrap();
                        let stored = page.request.changes.last().unwrap();
                        assert_eq!(stored.payload, change.payload);
                        validate_pushed_change(stored).unwrap();
                    }
                }
            }
        }
    }
}

#[tokio::test]
async fn shortening_legacy_oversized_notes_preserves_pending_history() {
    let database = Database::open(Path::new(":memory:")).await.unwrap();
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    let task = database
        .create_task(&workspace, draft(String::new()))
        .await
        .unwrap()
        .task;
    let note = database
        .add_note(&workspace, &task.id, "original".into())
        .await
        .unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    let payload: String = sqlx::query_scalar("SELECT payload FROM changes WHERE change_id = ?")
        .bind(&note.change_id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    let mut payload: Value = serde_json::from_str(&payload).unwrap();
    let body = "x".repeat(65_536);
    payload["body"] = Value::String(body.clone());
    let payload = payload.to_string();
    sqlx::query("UPDATE changes SET payload = ? WHERE change_id = ?")
        .bind(&payload)
        .bind(&note.change_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE notes SET body = ? WHERE id = ?")
        .bind(body)
        .bind(&note.note_id)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    database
        .edit_note(&workspace, &task.id, &note.note_id, "short".into())
        .await
        .unwrap();
    let page = database
        .prepare_client_sync_page("http://localhost:3000".into(), 256, 512)
        .await
        .unwrap();
    let legacy = page
        .request
        .changes
        .iter()
        .find(|change| change.change_id == note.change_id)
        .unwrap();
    assert_eq!(legacy.payload.to_string(), payload);
    assert!(legacy.server_seq.is_none());
    assert!(crate::sync::wire::validate_pushed_change(legacy).is_err());
    let shortened = page.request.changes.last().unwrap();
    assert_eq!(shortened.payload["body"], "short");
    crate::sync::wire::validate_pushed_change(shortened).unwrap();
}
