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
