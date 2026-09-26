use super::*;
use aven_core::{ids::TaskId, undo::UndoContext, workspaces::Workspace};

async fn snapshot_pair(f: &Fixture) -> (Workspace, TaskId, TaskId) {
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let mut c = aven_core::test_support::acquire(&f.peer).await.unwrap();
    let (a, b) = sqlx::query_as("SELECT task_id, depends_on_task_id FROM task_dependencies")
        .fetch_one(&mut *c)
        .await
        .unwrap();
    (w, a, b)
}
async fn edges(db: &Database) -> Vec<(TaskId, TaskId, String)> {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_as("SELECT task_id, depends_on_task_id, created_at FROM task_dependencies ORDER BY workspace_id, task_id, depends_on_task_id").fetch_all(&mut *c).await.unwrap()
}
async fn assert_drained(f: &Fixture) {
    assert_quiescent(&[&f.seed, &f.peer]).await;
    for db in [&f.seed, &f.peer] {
        let mut c = aven_core::test_support::acquire(db).await.unwrap();
        let cycle: bool = sqlx::query_scalar("WITH RECURSIVE paths(workspace_id, a, b) AS (SELECT workspace_id, task_id, depends_on_task_id FROM task_dependencies UNION SELECT p.workspace_id, p.a, d.depends_on_task_id FROM paths p JOIN task_dependencies d ON d.workspace_id=p.workspace_id AND d.task_id=p.b) SELECT EXISTS(SELECT 1 FROM paths WHERE a=b)").fetch_one(&mut *c).await.unwrap();
        assert!(!cycle);
    }
    assert_eq!(edges(&f.seed).await, edges(&f.peer).await);
}

#[tokio::test]
async fn remove_readd_converges_in_both_upload_orders() {
    for seed_first in [true, false] {
        let f = fixture_with(with_relations()).await;
        let (w, a, b) = snapshot_pair(&f).await;
        let baseline = edges(&f.seed).await;
        converge(&f).await;
        assert_eq!(edges(&f.seed).await, baseline);
        assert_eq!(edges(&f.peer).await, baseline);
        for db in [&f.seed, &f.peer] {
            for _ in 0..2 {
                assert!(db.remove_task_dependency(&w, &a, &b).await.unwrap().changed);
                assert!(db.add_task_dependency(&w, &a, &b).await.unwrap().changed);
            }
            assert!(db.remove_task_dependency(&w, &a, &b).await.unwrap().changed);
            assert!(!db.remove_task_dependency(&w, &a, &b).await.unwrap().changed);
        }
        assert!(
            f.seed
                .add_task_dependency(&w, &a, &b)
                .await
                .unwrap()
                .changed
        );
        if !seed_first {
            drain(&Client::new(&f.origin).unwrap(), &f.peer_store, &f.peer).await;
        }
        converge(&f).await;
        converge(&f).await;
        assert_drained(&f).await;
        assert_eq!(edges(&f.seed).await.len(), usize::from(!seed_first));
        for db in [&f.seed, &f.peer] {
            assert_eq!(scalar(db, "SELECT count(*) FROM task_labels").await, 2);
            assert_eq!(
                scalar(db, "SELECT count(*) FROM task_related_links WHERE linked=1").await,
                1
            );
            assert_eq!(scalar(db, "SELECT count(*) FROM task_epic_links").await, 1);
        }
    }
}

#[tokio::test]
async fn three_cycle_uses_sequential_policy_in_both_upload_orders() {
    for seed_first in [true, false] {
        let f = fixture().await;
        let w = f.seed.list_workspaces().await.unwrap().remove(0);
        let mut tasks = Vec::new();
        for _ in 0..3 {
            tasks.push(
                f.seed
                    .create_task(&w, draft("cycle"))
                    .await
                    .unwrap()
                    .task
                    .id,
            );
        }
        tasks.sort();
        let [a, b, c] = tasks.as_slice() else {
            unreachable!()
        };
        converge(&f).await;
        f.seed.add_task_dependency(&w, a, b).await.unwrap();
        f.seed.add_task_dependency(&w, b, c).await.unwrap();
        f.peer.add_task_dependency(&w, c, a).await.unwrap();
        if !seed_first {
            drain(&Client::new(&f.origin).unwrap(), &f.peer_store, &f.peer).await;
        }
        converge(&f).await;
        converge(&f).await;
        assert_drained(&f).await;
        let expected = if seed_first {
            vec![(a.clone(), b.clone()), (b.clone(), c.clone())]
        } else {
            vec![(a.clone(), b.clone()), (c.clone(), a.clone())]
        };
        assert_eq!(
            edges(&f.seed)
                .await
                .into_iter()
                .map(|(a, b, _)| (a, b))
                .collect::<Vec<_>>(),
            expected
        );
        // Removing an accepted edge does not revive a previously rejected addition.
        f.seed.remove_task_dependency(&w, a, b).await.unwrap();
        converge(&f).await;
        assert_drained(&f).await;
        assert_eq!(
            edges(&f.seed)
                .await
                .into_iter()
                .map(|(a, b, _)| (a, b))
                .collect::<Vec<_>>(),
            vec![expected[1].clone()]
        );
        for db in [&f.seed, &f.peer] {
            assert_eq!(
                scalar(
                    db,
                    "SELECT count(*) FROM changes WHERE op_type='dependency_add'"
                )
                .await,
                3
            );
        }
    }
}

#[tokio::test]
async fn post_capture_edit_does_not_contaminate_seed_baseline_or_retry() {
    let f = fixture_with(FixtureOptions {
        relations: true,
        dependency_after_capture: true,
        ..Default::default()
    })
    .await;
    assert!(edges(&f.seed).await.is_empty());
    assert_eq!(edges(&f.peer).await.len(), 1);
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM local_e2ee_dependency_edges").await,
            1
        );
    }
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        let export = db.export_data("fixed".into()).await.unwrap();
        assert!(export.tables.task_dependencies.is_empty());
        let json = serde_json::to_string(&export).unwrap();
        assert!(!json.contains("local_e2ee_dependency"));
    }
    let cursor = f.seed.meta("sync_cursor").await.unwrap();
    seed_bootstrap_http::Client::new(&f.origin)
        .unwrap()
        .resume(&f.seed_store, &f.seed)
        .await
        .unwrap();
    crate::peer_enrollment_http::Client::new(&f.origin)
        .unwrap()
        .install(&f.peer_store, &f.peer)
        .await
        .unwrap();
    assert_eq!(f.seed.meta("sync_cursor").await.unwrap(), cursor);
    assert_drained(&f).await;
    assert!(edges(&f.seed).await.is_empty());
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM local_e2ee_dependency_edges").await,
            1
        );
    }
}

async fn set_dependency(
    db: &Database,
    w: &Workspace,
    a: &TaskId,
    b: &TaskId,
    present: bool,
) -> bool {
    if present {
        db.add_task_dependency(w, a, b).await.unwrap().changed
    } else {
        db.remove_task_dependency(w, a, b).await.unwrap().changed
    }
}
async fn assert_dependency(db: &Database, _w: &Workspace, a: &TaskId, b: &TaskId, present: bool) {
    let actual = edges(db)
        .await
        .into_iter()
        .map(|(a, b, _)| (a, b))
        .collect::<Vec<_>>();
    assert_eq!(
        actual,
        if present {
            vec![(a.clone(), b.clone())]
        } else {
            vec![]
        }
    );
}

#[tokio::test]
async fn dependencies_preserve_undo_and_later_intent_through_acceptance_reopen_and_pages() {
    let f = fixture_with(with_relations()).await;
    let (w, task, target) = snapshot_pair(&f).await;
    converge(&f).await;
    // More than one ordinary pull page of unseen accepted relation commands.
    for _ in 0..9 {
        assert!(set_dependency(&f.peer, &w, &task, &target, false).await);
        assert!(set_dependency(&f.peer, &w, &task, &target, true).await);
    }
    let client = Client::new(&f.origin).unwrap();
    drain(&client, &f.peer_store, &f.peer).await;
    f.seed
        .remove_task_dependency_with_undo(
            &w,
            &task,
            &target,
            UndoContext::Tui {
                summary: "remove dependency".into(),
            },
        )
        .await
        .unwrap();
    let record = {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        head_record(&f.seed, &inputs.authority).await
    };
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    assert_dependency(&f.seed, &w, &task, &target, true).await;
    {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        let a = &inputs.authority;
        assert_eq!(head_record(&f.seed, a).await, record);
        let mut outcomes = Vec::new();
        for _ in 0..2 {
            let Reply::Appended(mapping) = client
                .exchange(
                    &a.context,
                    &inputs.bearer,
                    Operation::Append {
                        ticket: None,
                        record: record.clone(),
                    },
                )
                .await
                .unwrap()
            else {
                panic!("append")
            };
            outcomes.push(mapping);
        }
        assert!(outcomes[0] == outcomes[1]);
        let mapping = outcomes.remove(0);
        assert_eq!(
            f.seed.observe_encrypted_tail(a, &mapping).await.unwrap(),
            record
        );
        let Reply::Found(accepted) = client
            .exchange(
                &a.context,
                &inputs.bearer,
                Operation::Lookup {
                    operation_id: mapping.operation_id.clone(),
                    expected: Some(mapping),
                },
            )
            .await
            .unwrap()
        else {
            panic!("lookup")
        };
        let cursor = f.seed.encrypted_round_state(a).await.unwrap().cursor;
        let accepted_before = scalar(&f.seed, "SELECT count(*) FROM local_e2ee_accepted").await;
        {
            let mut c = aven_core::test_support::acquire(&f.seed).await.unwrap();
            sqlx::query("CREATE TRIGGER fail_dependency_outcome BEFORE DELETE ON task_dependencies BEGIN SELECT RAISE(ABORT, 'injected'); END").execute(&mut *c).await.unwrap();
        }
        assert!(
            f.seed
                .verify_encrypted_tail_outcome(a, &accepted)
                .await
                .is_err()
        );
        assert_eq!(
            scalar(&f.seed, "SELECT count(*) FROM local_e2ee_accepted").await,
            accepted_before
        );
        assert_eq!(
            scalar(&f.seed, "SELECT count(*) FROM local_e2ee_outbox").await,
            1
        );
        assert_eq!(
            f.seed.encrypted_round_state(a).await.unwrap().cursor,
            cursor
        );
        assert_dependency(&f.seed, &w, &task, &target, true).await;
        {
            let mut c = aven_core::test_support::acquire(&f.seed).await.unwrap();
            sqlx::query("DROP TRIGGER fail_dependency_outcome")
                .execute(&mut *c)
                .await
                .unwrap();
        }
        for _ in 0..2 {
            f.seed
                .verify_encrypted_tail_outcome(a, &accepted)
                .await
                .unwrap();
            assert_eq!(
                f.seed.encrypted_round_state(a).await.unwrap().cursor,
                cursor
            );
            assert_dependency(&f.seed, &w, &task, &target, true).await;
        }
    }
    // A second edit after accepted-only verification must also survive the echo.
    assert!(set_dependency(&f.seed, &w, &task, &target, false).await);
    let reopened = Database::open(f.seed.path()).await.unwrap();
    {
        let inputs = f
            .seed_store
            .tail_inputs(&reopened, &f.origin)
            .await
            .unwrap();
        let a = &inputs.authority;
        let mut watermark = None;
        let mut pages = 0;
        loop {
            let after = reopened.encrypted_round_state(a).await.unwrap().cursor;
            let Reply::Page(page) = client
                .exchange(
                    &a.context,
                    &inputs.bearer,
                    Operation::Pull {
                        after,
                        limit: 16,
                        watermark,
                    },
                )
                .await
                .unwrap()
            else {
                panic!("pull")
            };
            if pages == 0 {
                assert!(page.has_more);
                let before = scalar(&reopened, "SELECT count(*) FROM changes").await;
                let mut c = aven_core::test_support::acquire(&reopened).await.unwrap();
                sqlx::query("CREATE TRIGGER fail_dependency_apply BEFORE INSERT ON task_dependencies BEGIN SELECT RAISE(ABORT, 'injected'); END")
                    .execute(&mut *c).await.unwrap();
                drop(c);
                assert!(reopened.apply_encrypted_tail_page(a, &page).await.is_err());
                assert_eq!(
                    reopened.encrypted_round_state(a).await.unwrap().cursor,
                    after
                );
                assert_eq!(
                    scalar(&reopened, "SELECT count(*) FROM changes").await,
                    before
                );
                assert_dependency(&reopened, &w, &task, &target, false).await;
                let mut c = aven_core::test_support::acquire(&reopened).await.unwrap();
                sqlx::query("DROP TRIGGER fail_dependency_apply")
                    .execute(&mut *c)
                    .await
                    .unwrap();
            }
            reopened.apply_encrypted_tail_page(a, &page).await.unwrap();
            assert_dependency(&reopened, &w, &task, &target, false).await;
            assert_eq!(
                scalar(
                    &reopened,
                    "SELECT count(*) FROM changes WHERE server_seq IS NULL"
                )
                .await,
                2
            );
            watermark = Some(page.watermark);
            pages += 1;
            if !page.has_more {
                break;
            }
        }
        assert_eq!(pages, 2);
    }
    converge(&f).await;
    converge(&f).await;
    assert_drained(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_dependency(db, &w, &task, &target, false).await;
    }
    // Accepted dependency undo appends compensation rather than deleting history.
    f.seed
        .add_task_dependency_with_undo(
            &w,
            &task,
            &target,
            UndoContext::Tui {
                summary: "add dependency".into(),
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    let accepted_count = scalar(&f.seed, "SELECT count(*) FROM local_e2ee_accepted").await;
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM local_e2ee_accepted").await,
        accepted_count
    );
    converge(&f).await;
    assert_drained(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_dependency(db, &w, &task, &target, false).await;
    }
}

#[tokio::test]
async fn missing_or_mismatched_baseline_refuses_sync_and_exact_install_retry() {
    for fault in [
        "DELETE FROM local_e2ee_dependency_baseline",
        "UPDATE local_e2ee_dependency_baseline SET association = 'wrong'",
        "UPDATE local_e2ee_dependency_baseline SET sync_generation = sync_generation + 1",
        "UPDATE local_e2ee_dependency_baseline SET prefix_count = prefix_count + 1",
    ] {
        let f = fixture_with(with_relations()).await;
        let (w, a, b) = snapshot_pair(&f).await;
        f.peer.remove_task_dependency(&w, &a, &b).await.unwrap();
        let before = edges(&f.peer).await;
        let history = scalar(&f.peer, "SELECT count(*) FROM changes").await;
        let cursor = f.peer.meta("sync_cursor").await.unwrap();
        {
            let mut c = aven_core::test_support::acquire(&f.peer).await.unwrap();
            sqlx::query(fault).execute(&mut *c).await.unwrap();
        }
        let error = Client::new(&f.origin)
            .unwrap()
            .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("reinitialization-required"),
            "{error:#}"
        );
        let error = crate::peer_enrollment_http::Client::new(&f.origin)
            .unwrap()
            .install(&f.peer_store, &f.peer)
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("reinitialization-required"),
            "{error:#}"
        );
        assert_eq!(edges(&f.peer).await, before);
        assert_eq!(
            scalar(&f.peer, "SELECT count(*) FROM changes").await,
            history
        );
        assert_eq!(
            scalar(&f.peer, "SELECT count(*) FROM local_e2ee_outbox").await,
            0
        );
        assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), cursor);
        assert_eq!(
            scalar(&f.peer, "SELECT count(*) FROM local_e2ee_dependency_edges").await,
            1
        );
    }
}

#[tokio::test]
async fn task_endpoints_created_in_later_pages_are_replayed_after_creation() {
    let f = fixture_with(with_relations()).await;
    let (w, a, b) = snapshot_pair(&f).await;
    let baseline = edges(&f.seed).await;
    let mut tasks = Vec::new();
    for _ in 0..18 {
        tasks.push(
            f.seed
                .create_task(&w, draft("later endpoint"))
                .await
                .unwrap()
                .task
                .id,
        );
    }
    f.seed
        .add_task_dependency(&w, &tasks[16], &tasks[17])
        .await
        .unwrap();
    f.peer.remove_task_dependency(&w, &a, &b).await.unwrap();
    // Upload all seed work before the peer's multi-page catchup.
    drain(&Client::new(&f.origin).unwrap(), &f.seed_store, &f.seed).await;
    converge(&f).await;
    assert_drained(&f).await;
    assert_eq!(edges(&f.peer).await.len(), 1);
    assert_eq!(edges(&f.peer).await[0].0, tasks[16]);
    assert_eq!(edges(&f.peer).await[0].1, tasks[17]);
    assert_eq!(baseline.len(), 1);
}

#[tokio::test]
async fn missing_seed_baseline_refuses_adoption_retry_without_resetting_edits() {
    let f = fixture_with(with_relations()).await;
    let (w, a, b) = snapshot_pair(&f).await;
    f.seed.remove_task_dependency(&w, &a, &b).await.unwrap();
    let cursor = f.seed.meta("sync_cursor").await.unwrap();
    let history = scalar(&f.seed, "SELECT count(*) FROM changes").await;
    {
        let mut c = aven_core::test_support::acquire(&f.seed).await.unwrap();
        sqlx::query("DELETE FROM local_e2ee_dependency_baseline")
            .execute(&mut *c)
            .await
            .unwrap();
    }
    let error = seed_bootstrap_http::Client::new(&f.origin)
        .unwrap()
        .resume(&f.seed_store, &f.seed)
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("reinitialization-required"),
        "{error:#}"
    );
    assert!(edges(&f.seed).await.is_empty());
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM local_e2ee_dependency_edges").await,
        1
    );
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM changes").await,
        history
    );
    assert_eq!(f.seed.meta("sync_cursor").await.unwrap(), cursor);
}
