use super::*;
use crate::operations::{TaskDraft, TaskUpdate};

fn input(key: &str, value: String) -> TaskMetadataInput {
    TaskMetadataInput {
        expected_field_id: None,
        key: key.into(),
        value,
    }
}

async fn fixture(
    conn: &mut SqliteConnection,
    values: Vec<TaskMetadataInput>,
    absent: bool,
    remote: Option<&str>,
) -> (Workspace, TaskId, MetadataFieldId, String) {
    let workspace = crate::workspaces::ensure_default_workspace(conn)
        .await
        .unwrap();
    let task = crate::operations::create_task(
        conn,
        &workspace,
        TaskDraft {
            title: "metadata limits".into(),
            description: String::new(),
            project: Some("app".into()),
            status: "todo".into(),
            priority: "none".into(),
            source: crate::choices::TaskSource::Cli,
            labels: vec![],
            metadata: vec![input("owner", "initial".into())],
            available_at: None,
            due_on: None,
            is_epic: false,
        },
    )
    .await
    .unwrap()
    .task;
    crate::operations::update_task(
        conn,
        &workspace,
        &task.id,
        TaskUpdate {
            set_metadata: values,
            remove_metadata: if absent { vec!["owner".into()] } else { vec![] },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let field = crate::metadata::metadata_field_by_key(conn, &workspace.id, "owner")
        .await
        .unwrap()
        .unwrap();
    let identity = format!("metadata:{}", field.id);
    // Synthetic variants isolate resolution admission; HTTP tests create real conflicts.
    sqlx::query(
        "INSERT INTO conflicts(workspace_id, entity_type, entity_id, task_id, field,
        local_value, remote_value, remote_change_id, variant_a, variant_b, created_at)
        VALUES (?, 'task', ?, ?, ?, ?, ?, 'remote', 'a', 'b', 't')",
    )
    .bind(&workspace.id)
    .bind(&task.id)
    .bind(&task.id)
    .bind(&identity)
    .bind(encode_metadata_conflict_value(if absent { None } else { Some("initial") }).unwrap())
    .bind(encode_metadata_conflict_value(remote).unwrap())
    .execute(&mut *conn)
    .await
    .unwrap();
    (workspace, task.id, field.id, identity)
}

async fn snapshot(conn: &mut SqliteConnection) -> Vec<String> {
    let mut result = Vec::new();
    for query in [
        "SELECT json_group_array(json_array(id, updated_at)) FROM tasks",
        "SELECT json_group_array(json_array(workspace_id, task_id, field_id, value, created_at, updated_at)) FROM task_metadata",
        "SELECT json_group_array(json_array(change_id, client_id, local_seq, entity_type, entity_id, field, op_type, payload, base_version, created_at, server_seq)) FROM changes",
        "SELECT json_group_array(json_array(workspace_id, entity_type, entity_id, field, version)) FROM field_versions",
        "SELECT json_group_array(json_array(id, local_value, remote_value, resolved)) FROM conflicts",
        "SELECT json_group_array(json_array(key, value)) FROM meta",
    ] {
        result.push(
            sqlx::query_scalar::<_, String>(query)
                .fetch_one(&mut *conn)
                .await
                .unwrap(),
        );
    }
    result
}

#[tokio::test]
async fn value_limits_use_utf8_bytes_and_refusal_is_atomic() {
    for value in [
        "x".repeat(4096),
        "x".repeat(4097),
        "é".repeat(2048),
        format!("{}x", "é".repeat(2048)),
    ] {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let (w, task, field, identity) = fixture(&mut conn, vec![], false, Some("remote")).await;
        let before = snapshot(&mut conn).await;
        let result = resolve_metadata_conflict_value(
            &mut conn,
            &w,
            &task,
            &identity,
            field,
            ConflictResolutionValue::Explicit(&value),
        )
        .await;
        if value.len() > 4096 {
            assert!(
                result
                    .err()
                    .unwrap()
                    .to_string()
                    .contains("metadata-value-too-large")
            );
            assert_eq!(snapshot(&mut conn).await, before);
        } else {
            result.unwrap();
            let actual: String =
                sqlx::query_scalar("SELECT value FROM task_metadata WHERE task_id=?")
                    .bind(&task)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            assert_eq!(actual, value);
        }
    }
}

#[tokio::test]
async fn aggregate_limits_include_existing_values_and_replacements() {
    for (count, bytes, absent, value, expected) in [
        (7, 4096, false, 4096, None),
        (8, 4096, true, 1, Some("metadata-values-too-large")),
        (127, 1, true, 1, None),
        (128, 1, true, 1, Some("too-many-metadata-values")),
        (127, 1, false, 2, None),
    ] {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let values = (0..count)
            .map(|i| input(&format!("key{i}"), "x".repeat(bytes)))
            .collect();
        let (w, task, field, identity) = fixture(&mut conn, values, absent, Some("remote")).await;
        let before = snapshot(&mut conn).await;
        let result = resolve_metadata_conflict_value(
            &mut conn,
            &w,
            &task,
            &identity,
            field,
            ConflictResolutionValue::Explicit(&"x".repeat(value)),
        )
        .await;
        if let Some(expected) = expected {
            assert!(result.err().unwrap().to_string().contains(expected));
            assert_eq!(snapshot(&mut conn).await, before);
        } else {
            result.unwrap();
        }
    }
}

#[tokio::test]
async fn selected_variants_and_removal_share_validation() {
    for remote in [None, Some(""), Some("remote")] {
        for selection in [0, 1, 2] {
            let (_temp, mut conn) = crate::test_support::test_conn().await;
            let (w, task, field, identity) = fixture(&mut conn, vec![], false, remote).await;
            let encoded = encode_metadata_conflict_value(remote).unwrap();
            let (resolution, expected) = match selection {
                0 => (ConflictResolutionValue::Local, Some("initial")),
                1 => (ConflictResolutionValue::Remote, remote),
                _ => (ConflictResolutionValue::Explicit(&encoded), remote),
            };
            resolve_metadata_conflict_value(&mut conn, &w, &task, &identity, field, resolution)
                .await
                .unwrap();
            let actual: Option<String> =
                sqlx::query_scalar("SELECT value FROM task_metadata WHERE task_id=?")
                    .bind(&task)
                    .fetch_optional(&mut *conn)
                    .await
                    .unwrap();
            assert_eq!(actual.as_deref(), expected);
        }
    }
    // A valid remote variant can exceed the result budget after unrelated local additions.
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    let values = (0..128)
        .map(|i| input(&format!("key{i}"), "x".into()))
        .collect();
    let (w, task, field, identity) = fixture(&mut conn, values, true, Some("remote")).await;
    let before = snapshot(&mut conn).await;
    assert!(
        resolve_metadata_conflict_value(
            &mut conn,
            &w,
            &task,
            &identity,
            field.clone(),
            ConflictResolutionValue::Remote
        )
        .await
        .err()
        .unwrap()
        .to_string()
        .contains("too-many-metadata-values")
    );
    assert_eq!(snapshot(&mut conn).await, before);
    resolve_metadata_conflict_value(
        &mut conn,
        &w,
        &task,
        &identity,
        field,
        ConflictResolutionValue::Local,
    )
    .await
    .unwrap();
}
