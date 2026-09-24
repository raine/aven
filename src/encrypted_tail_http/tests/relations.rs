use super::*;
use aven_core::{ids::TaskId, undo::UndoContext, workspaces::Workspace};

fn label_update(present: bool) -> TaskUpdate {
    if present {
        TaskUpdate {
            add_labels: vec!["tag".into()],
            ..Default::default()
        }
    } else {
        TaskUpdate {
            remove_labels: vec!["tag".into()],
            ..Default::default()
        }
    }
}

async fn set_label(db: &Database, w: &Workspace, task: &TaskId, present: bool) -> bool {
    db.update_task(w, task, label_update(present))
        .await
        .unwrap()
        .changed
}

async fn snapshot_owner(f: &Fixture) -> (Workspace, TaskId) {
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let mut c = aven_core::test_support::acquire(&f.seed).await.unwrap();
    let task: TaskId =
        sqlx::query_scalar("SELECT id FROM tasks WHERE title = 'snapshot relation owner'")
            .fetch_one(&mut *c)
            .await
            .unwrap();
    (w, task)
}

async fn assert_labels(db: &Database, w: &Workspace, task: &TaskId, present: bool) {
    let expected = if present {
        vec!["tag", "untouched"]
    } else {
        vec!["untouched"]
    };
    assert_eq!(db.task_labels(&w.id, task).await.unwrap(), expected);
}

async fn assert_drained(f: &Fixture) {
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM changes WHERE server_seq IS NULL").await,
            0
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM local_e2ee_outbox").await,
            0
        );
    }
}

#[tokio::test]
async fn labels_follow_accepted_order_with_repeated_offline_commands() {
    for seed_first in [true, false] {
        for repetitions in [1, 3] {
            let f = fixture_with_snapshot_content(false, None, true).await;
            let (w, task) = snapshot_owner(&f).await;
            converge(&f).await;
            for db in [&f.seed, &f.peer] {
                assert_labels(db, &w, &task, true).await;
                for _ in 1..repetitions {
                    assert!(set_label(db, &w, &task, false).await);
                    assert!(set_label(db, &w, &task, true).await);
                }
                assert!(set_label(db, &w, &task, false).await);
                assert!(!set_label(db, &w, &task, false).await);
            }
            assert!(set_label(&f.seed, &w, &task, true).await);
            assert!(!set_label(&f.seed, &w, &task, true).await);
            if !seed_first {
                drain(&Client::new(&f.origin).unwrap(), &f.peer_store, &f.peer).await;
            }
            converge(&f).await;
            converge(&f).await;
            assert_drained(&f).await;
            for db in [&f.seed, &f.peer] {
                assert_labels(db, &w, &task, !seed_first).await;
                assert_eq!(
                    scalar(db, "SELECT count(*) FROM task_dependencies").await,
                    1
                );
                assert_eq!(
                    scalar(db, "SELECT count(*) FROM task_related_links WHERE linked=1").await,
                    1
                );
                assert_eq!(scalar(db, "SELECT count(*) FROM task_epic_links").await, 1);
                assert_eq!(
                    scalar(db, "SELECT count(*) FROM tasks WHERE is_epic=1").await,
                    1
                );
            }
        }
    }
}

#[tokio::test]
async fn labels_preserve_undo_and_later_intent_through_acceptance_reopen_and_pages() {
    let f = fixture_with_snapshot_content(false, None, true).await;
    let (w, task) = snapshot_owner(&f).await;
    converge(&f).await;
    // More than one ordinary pull page of unseen accepted relation commands.
    for _ in 0..9 {
        assert!(set_label(&f.peer, &w, &task, false).await);
        assert!(set_label(&f.peer, &w, &task, true).await);
    }
    let client = Client::new(&f.origin).unwrap();
    drain(&client, &f.peer_store, &f.peer).await;
    f.seed
        .mutate_tasks(
            &w,
            vec![(task.clone(), label_update(false))],
            UndoContext::Tui {
                summary: "remove label".into(),
            },
        )
        .await
        .unwrap();
    let record = {
        let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
        head_record(&f.seed, &inputs.authority).await
    };
    f.seed.apply_latest_tui_undo(&w.id).await.unwrap().unwrap();
    assert_labels(&f.seed, &w, &task, true).await;
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
        let cursor = f.seed.encrypted_tail_cursor(a).await.unwrap();
        for _ in 0..2 {
            f.seed
                .verify_encrypted_tail_outcome(a, &accepted)
                .await
                .unwrap();
            assert_eq!(f.seed.encrypted_tail_cursor(a).await.unwrap(), cursor);
            assert_labels(&f.seed, &w, &task, true).await;
        }
    }
    // A second edit after accepted-only verification must also survive the echo.
    assert!(set_label(&f.seed, &w, &task, false).await);
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
            let after = reopened.encrypted_tail_cursor(a).await.unwrap();
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
                sqlx::query("CREATE TRIGGER fail_label_apply BEFORE INSERT ON task_labels BEGIN SELECT RAISE(ABORT, 'injected'); END")
                    .execute(&mut *c).await.unwrap();
                drop(c);
                assert!(reopened.apply_encrypted_tail_page(a, &page).await.is_err());
                assert_eq!(reopened.encrypted_tail_cursor(a).await.unwrap(), after);
                assert_eq!(
                    scalar(&reopened, "SELECT count(*) FROM changes").await,
                    before
                );
                assert_labels(&reopened, &w, &task, false).await;
                let mut c = aven_core::test_support::acquire(&reopened).await.unwrap();
                sqlx::query("DROP TRIGGER fail_label_apply")
                    .execute(&mut *c)
                    .await
                    .unwrap();
            }
            reopened.apply_encrypted_tail_page(a, &page).await.unwrap();
            assert_labels(&reopened, &w, &task, false).await;
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
        assert_labels(db, &w, &task, false).await;
    }
    // Accepted label undo appends compensation rather than deleting history.
    f.seed
        .mutate_tasks(
            &w,
            vec![(task.clone(), label_update(true))],
            UndoContext::Tui {
                summary: "add label".into(),
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
        assert_labels(db, &w, &task, false).await;
    }
}

#[tokio::test]
async fn label_reconciliation_preserves_related_and_epic_ordering() {
    for seed_first in [true, false] {
        let f = fixture_with_snapshot_content(false, None, true).await;
        let (w, task) = snapshot_owner(&f).await;
        let target: TaskId = {
            let mut c = aven_core::test_support::acquire(&f.seed).await.unwrap();
            sqlx::query_scalar("SELECT id FROM tasks WHERE title = 'snapshot relation target'")
                .fetch_one(&mut *c)
                .await
                .unwrap()
        };
        converge(&f).await;
        for db in [&f.seed, &f.peer] {
            assert!(set_label(db, &w, &task, false).await);
            db.remove_task_related_link(&w, &task, &target)
                .await
                .unwrap();
            db.remove_task_from_epic(&w, &task, &target).await.unwrap();
        }
        assert!(set_label(&f.seed, &w, &task, true).await);
        f.seed
            .add_task_related_link(&w, &task, &target)
            .await
            .unwrap();
        f.seed.add_task_to_epic(&w, &task, &target).await.unwrap();
        if !seed_first {
            drain(&Client::new(&f.origin).unwrap(), &f.peer_store, &f.peer).await;
        }
        converge(&f).await;
        converge(&f).await;
        assert_drained(&f).await;
        for db in [&f.seed, &f.peer] {
            assert_labels(db, &w, &task, !seed_first).await;
            assert_eq!(
                scalar(db, "SELECT count(*) FROM task_related_links WHERE linked=1").await,
                i64::from(!seed_first)
            );
            assert_eq!(
                scalar(db, "SELECT count(*) FROM task_epic_links").await,
                i64::from(!seed_first)
            );
            assert_eq!(
                scalar(db, "SELECT count(*) FROM tasks WHERE is_epic=1").await,
                1
            );
        }
    }
}

#[tokio::test]
async fn opposite_dependencies_keep_the_minimum_id_direction() {
    for seed_first in [true, false] {
        let f = fixture().await;
        let w = f.seed.list_workspaces().await.unwrap().remove(0);
        let mut tasks = Vec::new();
        for _ in 0..2 {
            tasks.push(
                f.seed
                    .create_task(&w, draft("opposite dependency"))
                    .await
                    .unwrap()
                    .task
                    .id,
            );
        }
        tasks.sort();
        converge(&f).await;
        f.seed
            .add_task_dependency(&w, &tasks[1], &tasks[0])
            .await
            .unwrap();
        f.peer
            .add_task_dependency(&w, &tasks[0], &tasks[1])
            .await
            .unwrap();
        if !seed_first {
            drain(&Client::new(&f.origin).unwrap(), &f.peer_store, &f.peer).await;
        }
        converge(&f).await;
        converge(&f).await;
        assert_drained(&f).await;
        for db in [&f.seed, &f.peer] {
            let mut c = aven_core::test_support::acquire(db).await.unwrap();
            let edges: Vec<(TaskId, TaskId)> = sqlx::query_as(
                "SELECT task_id, depends_on_task_id FROM task_dependencies ORDER BY task_id, depends_on_task_id",
            ).fetch_all(&mut *c).await.unwrap();
            assert_eq!(edges, vec![(tasks[0].clone(), tasks[1].clone())]);
        }
    }
}
