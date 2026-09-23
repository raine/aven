//! Synthetic storage/reducer fixtures; acceptance ranks are not HTTP evidence.
use super::*;
use crate::sync::persistence::insert_wire_change;
use serde_json::json;

const W: &str = "0000000000000000";
const A: &str = "AAAAAAAAAAAAAAAA";
const B: &str = "BBBBBBBBBBBBBBBB";
const C: &str = "CCCCCCCCCCCCCCCC";

async fn setup(conn: &mut SqliteConnection) {
    for id in [A, B, C] {
        sqlx::query("INSERT INTO tasks(id, workspace_id, title, description, project_id, status, priority, created_at, updated_at) VALUES (?, ?, 'task', '', '0000000000000000', 'inbox', 'none', 'original', 'original')")
            .bind(id).bind(W).execute(&mut *conn).await.unwrap();
    }
    crate::db::set_meta(conn, "e2ee_association", "test")
        .await
        .unwrap();
    crate::db::set_meta(conn, "sync_generation", "1")
        .await
        .unwrap();
}
fn edge(a: &str, b: &str) -> TaskDependencyRow {
    TaskDependencyRow {
        workspace_id: W.parse().unwrap(),
        task_id: a.parse().unwrap(),
        depends_on_task_id: b.parse().unwrap(),
        created_at: "baseline".into(),
    }
}
fn command(n: i64, a: &str, b: &str, add: bool, seq: Option<i64>) -> ChangeWire {
    let mut c = crate::sync::encrypted_tail::tests::change();
    c.change_id = format!("{n:016}");
    c.entity_id = a.into();
    c.local_seq = n;
    c.field = Some("dependencies".into());
    c.base_version = None;
    c.op_type = if add {
        "dependency_add"
    } else {
        "dependency_remove"
    }
    .into();
    c.payload = json!({"workspace_id":W,"workspace_key":"default","depends_on_task_id":b});
    c.server_seq = seq;
    c
}
async fn graph(conn: &mut SqliteConnection) -> Vec<(String, String, String)> {
    sqlx::query_as("SELECT task_id, depends_on_task_id, created_at FROM task_dependencies WHERE workspace_id = ? ORDER BY task_id, depends_on_task_id")
        .bind(W).fetch_all(conn).await.unwrap()
}
#[tokio::test]
async fn baseline_is_not_prefix_history_and_rejected_edges_do_not_revive() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    setup(&mut conn).await;
    initialize(&mut conn, "test", 1, 10, &[edge(A, B), edge(B, C)])
        .await
        .unwrap();
    validate(&mut conn, "test", 10).await.unwrap();
    // The materialized baseline deliberately cannot be recovered from this prefix.
    insert_wire_change(&mut conn, &command(1, A, B, false, Some(10)))
        .await
        .unwrap();
    reconcile(&mut conn, 10, W).await.unwrap();
    assert_eq!(
        graph(&mut conn).await,
        vec![
            (A.into(), B.into(), "baseline".into()),
            (B.into(), C.into(), "baseline".into())
        ]
    );
    insert_wire_change(&mut conn, &command(2, C, A, true, Some(11)))
        .await
        .unwrap();
    insert_wire_change(&mut conn, &command(3, B, C, false, None))
        .await
        .unwrap();
    reconcile(&mut conn, 10, W).await.unwrap();
    assert_eq!(
        graph(&mut conn).await,
        vec![(A.into(), B.into(), "baseline".into())]
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM changes")
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>("SELECT updated_at FROM tasks WHERE id = ?")
            .bind(A)
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        "original"
    );
    // Exact initialization must never reset an established baseline.
    assert!(initialize(&mut conn, "test", 1, 10, &[]).await.is_err());
    assert!(validate(&mut conn, "other", 10).await.is_err());
    assert!(validate(&mut conn, "test", 9).await.is_err());
}

#[tokio::test]
async fn initialization_and_dangling_tail_failure_roll_back() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    setup(&mut conn).await;
    assert!(
        validate(&mut conn, "test", 10)
            .await
            .unwrap_err()
            .to_string()
            .contains("reinitialization-required")
    );
    {
        let mut tx = crate::db::begin_immediate(&mut conn).await.unwrap();
        assert!(
            initialize(&mut tx, "test", 1, 10, &[edge(A, B), edge(A, B)])
                .await
                .is_err()
        );
        tx.rollback().await.unwrap();
    }
    assert!(validate(&mut conn, "test", 10).await.is_err());
    initialize(&mut conn, "test", 1, 10, &[edge(A, B)])
        .await
        .unwrap();
    reconcile(&mut conn, 10, W).await.unwrap();
    let before = graph(&mut conn).await;
    {
        let mut tx = crate::db::begin_immediate(&mut conn).await.unwrap();
        insert_wire_change(&mut tx, &command(1, C, "DDDDDDDDDDDDDDDD", true, Some(11)))
            .await
            .unwrap();
        assert!(reconcile(&mut tx, 10, W).await.is_err());
        tx.rollback().await.unwrap();
    }
    assert_eq!(graph(&mut conn).await, before);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM changes")
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn batches_preserve_pending_order_timestamps_and_workspace_isolation() {
    let (_temp, mut conn) = crate::test_support::test_conn().await;
    setup(&mut conn).await;
    let foreign = "1111111111111111";
    sqlx::query("INSERT INTO task_dependencies(workspace_id, task_id, depends_on_task_id, created_at) VALUES (?, ?, ?, 'foreign')")
        .bind(foreign).bind(A).bind(B).execute(&mut *conn).await.unwrap();
    initialize(&mut conn, "test", 1, 10, &[edge(A, B)])
        .await
        .unwrap();
    // Multiple read batches, with deliberately non-ordering timestamps.
    for n in 11..=140 {
        let mut c = command(n, A, B, n % 2 == 0, Some(n));
        c.created_at = format!("reverse-{}", 1000 - n);
        insert_wire_change(&mut conn, &c).await.unwrap();
    }
    let mut other = command(141, A, B, false, Some(141));
    other.payload["workspace_id"] = json!(foreign);
    insert_wire_change(&mut conn, &other).await.unwrap();
    insert_wire_change(&mut conn, &command(142, A, B, false, None))
        .await
        .unwrap();
    insert_wire_change(&mut conn, &command(143, A, B, true, None))
        .await
        .unwrap();
    reconcile(&mut conn, 10, W).await.unwrap();
    assert_eq!(
        graph(&mut conn).await,
        vec![(
            A.into(),
            B.into(),
            command(143, A, B, true, None).created_at
        )]
    );
    assert_eq!(
        sqlx::query_scalar::<_, String>(
            "SELECT created_at FROM task_dependencies WHERE workspace_id = ?"
        )
        .bind(foreign)
        .fetch_one(&mut *conn)
        .await
        .unwrap(),
        "foreign"
    );
}
