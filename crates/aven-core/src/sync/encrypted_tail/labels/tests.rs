use super::*;
use crate::sync::persistence::insert_wire_change;
use serde_json::json;

fn operation(index: i64, add: bool, rank: Option<i64>) -> ChangeWire {
    let mut c = crate::sync::encrypted_tail::tests::change();
    c.change_id = format!("{index:016}");
    c.local_seq = index;
    c.op_type = if add {
        op_type::LABEL_ADD
    } else {
        op_type::LABEL_REMOVE
    }
    .into();
    c.field = Some("labels".into());
    c.base_version = None;
    c.server_seq = rank;
    c.payload =
        json!({"workspace_id": "0000000000000000", "workspace_key": "default", "label": "tag"});
    c
}

async fn labels(conn: &mut SqliteConnection) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT label FROM task_labels
         WHERE workspace_id = '0000000000000000' AND task_id = 'BBBBBBBBBBBBBBBB'
         ORDER BY label",
    )
    .fetch_all(conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn snapshot_presence_and_absence_are_not_replayed_from_prefix_history() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    let old_remove = operation(1, false, Some(10));
    insert_wire_change(&mut conn, &old_remove).await.unwrap();
    sqlx::query(
        "INSERT INTO task_labels(workspace_id, task_id, label)
         VALUES ('0000000000000000', 'BBBBBBBBBBBBBBBB', 'tag'),
                ('0000000000000000', 'BBBBBBBBBBBBBBBB', 'snapshot only')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    reconcile(&mut conn, 10, &old_remove).await.unwrap();
    assert_eq!(labels(&mut conn).await, ["snapshot only", "tag"]);
    let mut old_add = operation(2, true, Some(9));
    old_add.payload["label"] = json!("absent");
    insert_wire_change(&mut conn, &old_add).await.unwrap();
    reconcile(&mut conn, 10, &old_add).await.unwrap();
    assert_eq!(labels(&mut conn).await, ["snapshot only", "tag"]);
    let remove = operation(3, false, Some(11));
    insert_wire_change(&mut conn, &remove).await.unwrap();
    reconcile(&mut conn, 10, &remove).await.unwrap();
    assert_eq!(labels(&mut conn).await, ["snapshot only"]);
    let add = operation(4, true, None);
    insert_wire_change(&mut conn, &add).await.unwrap();
    reconcile(&mut conn, 10, &add).await.unwrap();
    assert_eq!(labels(&mut conn).await, ["snapshot only", "tag"]);
}

#[tokio::test]
async fn latest_pending_then_accepted_command_assigns_only_its_own_label() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    let add = operation(1, true, Some(15));
    let remove = operation(2, false, Some(11));
    for c in [&add, &remove] {
        insert_wire_change(&mut conn, c).await.unwrap();
    }
    reconcile(&mut conn, 10, &remove).await.unwrap();
    assert_eq!(labels(&mut conn).await, ["tag"]);
    let pending_remove = operation(3, false, None);
    insert_wire_change(&mut conn, &pending_remove)
        .await
        .unwrap();
    reconcile(&mut conn, 10, &add).await.unwrap();
    assert!(labels(&mut conn).await.is_empty());
    let pending_add = operation(4, true, None);
    insert_wire_change(&mut conn, &pending_add).await.unwrap();
    for (i, key, value) in [
        (5, "workspace_id", "1111111111111111"),
        (6, "label", "other"),
        (7, "entity_id", "CCCCCCCCCCCCCCCC"),
    ] {
        let mut foreign = operation(i, false, None);
        if key == "entity_id" {
            foreign.entity_id = value.into();
        } else {
            foreign.payload[key] = json!(value);
        }
        insert_wire_change(&mut conn, &foreign).await.unwrap();
        reconcile(&mut conn, 10, &foreign).await.unwrap();
    }
    reconcile(&mut conn, 10, &add).await.unwrap();
    assert_eq!(labels(&mut conn).await, ["tag"]);
}

fn administration(index: i64, op: &str, name: &str, extra: serde_json::Value) -> ChangeWire {
    let mut c = operation(index, false, None);
    c.op_type = op.into();
    c.entity_type = "label".into();
    c.entity_id = name.into();
    c.field = None;
    c.payload =
        json!({"workspace_id": "0000000000000000", "workspace_key": "default", "name": name});
    c.payload
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    c
}

async fn label_rows(conn: &mut SqliteConnection) -> Vec<String> {
    sqlx::query_scalar("SELECT name FROM labels ORDER BY name")
        .fetch_all(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn label_administration_assigns_presence_and_existence_in_tail_order() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    // A remote add applied after a pending delete recreated both rows.
    let add = operation(1, true, Some(11));
    let delete = administration(2, op_type::LABEL_DELETE, "tag", json!({"deleted_at": "t"}));
    for c in [&add, &delete] {
        insert_wire_change(&mut conn, c).await.unwrap();
    }
    crate::sync::apply::apply_remote_change_quiet(&mut conn, &add)
        .await
        .unwrap();
    assert_eq!(labels(&mut conn).await, ["tag"]);
    reconcile(&mut conn, 10, &add).await.unwrap();
    assert!(labels(&mut conn).await.is_empty());
    assert!(label_rows(&mut conn).await.is_empty());
    // Restoration lists the task, so it assigns presence and the label row.
    let restore = administration(
        3,
        op_type::LABEL_RESTORE,
        "tag",
        json!({"created_at": "c", "task_ids": ["BBBBBBBBBBBBBBBB"], "series_ids": [], "restored_at": "t"}),
    );
    insert_wire_change(&mut conn, &restore).await.unwrap();
    reconcile(&mut conn, 10, &add).await.unwrap();
    assert_eq!(labels(&mut conn).await, ["tag"]);
    assert_eq!(label_rows(&mut conn).await, ["tag"]);
    // A rename away removes the pair; a rename into leaves the moved presence.
    let away = administration(
        4,
        op_type::SET_LABEL_NAME,
        "tag",
        json!({"new_name": "topic", "renamed_at": "t"}),
    );
    insert_wire_change(&mut conn, &away).await.unwrap();
    crate::sync::apply::apply_remote_change_quiet(&mut conn, &away)
        .await
        .unwrap();
    crate::sync::apply::apply_remote_change_quiet(&mut conn, &add)
        .await
        .unwrap();
    assert_eq!(labels(&mut conn).await, ["tag", "topic"]);
    reconcile(&mut conn, 10, &away).await.unwrap();
    assert_eq!(labels(&mut conn).await, ["topic"]);
    assert_eq!(label_rows(&mut conn).await, ["topic"]);
    let mut accepted_remove = operation(5, false, Some(12));
    accepted_remove.payload["label"] = json!("topic");
    insert_wire_change(&mut conn, &accepted_remove)
        .await
        .unwrap();
    reconcile(&mut conn, 10, &accepted_remove).await.unwrap();
    assert_eq!(labels(&mut conn).await, ["topic"]);
}

#[tokio::test]
async fn reapplied_rename_keeps_later_pending_commands_on_the_new_name() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    // Local pending rename, then a pending removal from the renamed label.
    let rename = administration(
        1,
        op_type::SET_LABEL_NAME,
        "tag",
        json!({"new_name": "topic", "renamed_at": "t"}),
    );
    let mut remove_topic = operation(2, false, None);
    remove_topic.payload["label"] = json!("topic");
    // A remote add of the old name that the server orders before both.
    let remote_add = operation(3, true, Some(11));
    for c in [&rename, &remove_topic, &remote_add] {
        insert_wire_change(&mut conn, c).await.unwrap();
    }
    sqlx::query("INSERT INTO labels(workspace_id, name, created_at) VALUES ('0000000000000000', 'topic', 'c')")
        .execute(&mut *conn)
        .await
        .unwrap();
    crate::sync::apply::apply_remote_change_quiet(&mut conn, &remote_add)
        .await
        .unwrap();
    reconcile(&mut conn, 10, &remote_add).await.unwrap();
    // Ordered replay: add tag, rename tag to topic, remove topic.
    assert!(labels(&mut conn).await.is_empty());
    assert_eq!(label_rows(&mut conn).await, ["topic"]);
}
