use super::*;
use crate::sync::persistence::insert_wire_change;
use serde_json::json;

fn operation(index: i64, op: &str, body: &str, rank: Option<i64>) -> ChangeWire {
    let mut c = crate::sync::encrypted_tail::tests::change();
    c.change_id = format!("{index:016}");
    c.local_seq = index;
    c.op_type = op.into();
    c.field = Some("notes".into());
    c.base_version = None;
    c.server_seq = rank;
    c.payload = json!({"workspace_id": "0000000000000000", "workspace_key": "default",
        "note_id": "NNNNNNNNNNNNNNNN", "body": body, "created_at": "creation",
        "edited_at": "edit", "deleted_at": "deletion"});
    c
}

async fn materialized(conn: &mut SqliteConnection) -> Option<(String, String, String)> {
    sqlx::query_as("SELECT body, created_at, change_id FROM notes WHERE id='NNNNNNNNNNNNNNNN'")
        .fetch_optional(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn snapshot_materialization_is_not_rebuilt_from_prefix_history() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    let prefix = operation(1, op_type::NOTE_ADD, "old retained body", Some(10));
    insert_wire_change(&mut conn, &prefix).await.unwrap();
    sqlx::query(
        "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
        VALUES ('0000000000000000', 'NNNNNNNNNNNNNNNN', 'BBBBBBBBBBBBBBBB',
                'snapshot body', 'snapshot timestamp', 'snapshot lineage')",
    )
    .execute(&mut *conn)
    .await
    .unwrap();
    reconcile(&mut conn, 10, &prefix).await.unwrap();
    assert_eq!(
        materialized(&mut conn).await,
        Some((
            "snapshot body".into(),
            "snapshot timestamp".into(),
            "snapshot lineage".into()
        ))
    );
    let edit = operation(2, op_type::NOTE_EDIT, "tail body", Some(11));
    insert_wire_change(&mut conn, &edit).await.unwrap();
    reconcile(&mut conn, 10, &edit).await.unwrap();
    assert_eq!(
        materialized(&mut conn).await,
        Some((
            "tail body".into(),
            "snapshot timestamp".into(),
            "snapshot lineage".into()
        ))
    );
    // A snapshot-only tombstone has no note row, even if its retained add survives.
    sqlx::query("DELETE FROM notes")
        .execute(&mut *conn)
        .await
        .unwrap();
    reconcile(&mut conn, 10, &edit).await.unwrap();
    assert_eq!(materialized(&mut conn).await, None);
}

#[tokio::test]
async fn ordered_tail_preserves_add_lineage_and_ignores_edits_after_deletion() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    for c in [
        operation(1, op_type::NOTE_ADD, "first add", Some(11)),
        operation(2, op_type::NOTE_EDIT, "first edit", Some(12)),
        operation(3, op_type::NOTE_DELETE, "first edit", Some(13)),
        operation(4, op_type::NOTE_EDIT, "must not resurrect", Some(14)),
    ] {
        insert_wire_change(&mut conn, &c).await.unwrap();
        reconcile(&mut conn, 10, &c).await.unwrap();
    }
    assert_eq!(materialized(&mut conn).await, None);
    let restore = operation(5, op_type::NOTE_ADD, "restore", Some(15));
    insert_wire_change(&mut conn, &restore).await.unwrap();
    let duplicate = operation(6, op_type::NOTE_ADD, "ignored second add", None);
    insert_wire_change(&mut conn, &duplicate).await.unwrap();
    reconcile(&mut conn, 10, &duplicate).await.unwrap();
    assert_eq!(
        materialized(&mut conn).await,
        Some((
            "restore".into(),
            "creation".into(),
            restore.change_id.clone()
        ))
    );
    let pending = operation(7, op_type::NOTE_EDIT, "pending body", None);
    insert_wire_change(&mut conn, &pending).await.unwrap();
    let accepted = operation(8, op_type::NOTE_EDIT, "accepted body", Some(16));
    insert_wire_change(&mut conn, &accepted).await.unwrap();
    reconcile(&mut conn, 10, &accepted).await.unwrap();
    assert_eq!(
        materialized(&mut conn).await,
        Some(("pending body".into(), "creation".into(), restore.change_id))
    );
}

#[tokio::test]
async fn history_and_materialization_remain_task_and_workspace_scoped() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    let own = operation(1, op_type::NOTE_ADD, "own", Some(11));
    insert_wire_change(&mut conn, &own).await.unwrap();
    reconcile(&mut conn, 10, &own).await.unwrap();
    for other_workspace in [false, true] {
        let mut foreign = operation(
            if other_workspace { 3 } else { 2 },
            op_type::NOTE_DELETE,
            "foreign",
            None,
        );
        if other_workspace {
            foreign.payload["workspace_id"] = json!("1111111111111111");
        } else {
            foreign.entity_id = "CCCCCCCCCCCCCCCC".into();
        }
        insert_wire_change(&mut conn, &foreign).await.unwrap();
        reconcile(&mut conn, 10, &foreign).await.unwrap();
    }
    reconcile(&mut conn, 10, &own).await.unwrap();
    assert_eq!(materialized(&mut conn).await.unwrap().0, "own");
}
