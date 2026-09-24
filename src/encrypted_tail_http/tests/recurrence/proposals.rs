use super::*;
use aven_core::{ids::TaskId, recurrence::RecurrenceDuePolicy, workspaces::Workspace};

async fn text(db: &Database, sql: &str, id: &str) -> Option<String> {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .bind(id)
        .fetch_optional(&mut *c)
        .await
        .unwrap()
}

async fn labels(db: &Database, task: &TaskId) -> Vec<String> {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar("SELECT label FROM task_labels WHERE task_id = ? ORDER BY label")
        .bind(task)
        .fetch_all(&mut *c)
        .await
        .unwrap()
}

async fn task_conflicts(db: &Database, task: &TaskId) -> Vec<String> {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar(
        "SELECT field FROM conflicts WHERE task_id = ? AND resolved = 0 ORDER BY field",
    )
    .bind(task)
    .fetch_all(&mut *c)
    .await
    .unwrap()
}

const KIND: &str = "SELECT m.value FROM task_metadata m JOIN metadata_fields f ON f.id = m.field_id
                    WHERE f.key = 'kind' AND m.task_id = ?";

/// Edits the template differently on each side, including editable schedule
/// fields, then completes the current task so both generate the next occurrence.
async fn diverge(
    f: &Fixture,
    w: &Workspace,
    created: &aven_core::operations::RecurrenceCreateOutcome,
) -> TaskId {
    for (db, side, time, policy) in [
        (&f.seed, "seed", "09:00:00", RecurrenceDuePolicy::SameDay),
        (&f.peer, "peer", "10:30:00", RecurrenceDuePolicy::None),
    ] {
        db.update_recurrence_template(
            w,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                title: Some(format!("{side} successor")),
                labels: Some(vec![
                    "shared".into(),
                    "gone".into(),
                    format!("{side}-label"),
                ]),
                set_metadata: vec![TaskMetadataInput {
                    expected_field_id: None,
                    key: "kind".into(),
                    value: side.into(),
                }],
                available_local_time: Some(Some(time.parse().unwrap())),
                due_policy: Some(policy),
                ..Default::default()
            })
            .with_create_missing_labels(),
        )
        .await
        .unwrap();
        db.update_task(
            w,
            &created.task.id,
            TaskUpdate {
                status: Some("done".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    sqlx::query_scalar("SELECT task_id FROM recurrence_occurrences WHERE task_id <> ?")
        .bind(&created.task.id)
        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
        .await
        .unwrap()
}

async fn available_at(db: &Database, task: &TaskId) -> (String, String) {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_as("SELECT available_at, due_on FROM tasks WHERE id = ?")
        .bind(task)
        .fetch_one(&mut *c)
        .await
        .unwrap()
}

async fn assert_idle(db: &Database) {
    assert_eq!(
        scalar(db, "SELECT count(*) FROM changes WHERE server_seq IS NULL").await,
        0
    );
    assert_eq!(
        scalar(db, "SELECT count(*) FROM local_e2ee_outbox").await,
        0
    );
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Acceptance {
    SeedFirst,
    PeerFirst,
    /// The peer dispatches its own generation after the seed's was accepted.
    PeerFrozenAfterSeed,
}

#[tokio::test]
async fn concurrent_template_and_schedule_edits_converge_on_first_accepted_defaults() {
    for mode in [
        Acceptance::SeedFirst,
        Acceptance::PeerFirst,
        Acceptance::PeerFrozenAfterSeed,
    ] {
        let f = fixture().await;
        converge(&f).await;
        let w = f.seed.list_workspaces().await.unwrap().remove(0);
        let created = create(&f.seed).await;
        converge(&f).await;
        let successor = diverge(&f, &w, &created).await;
        let seed_schedule = available_at(&f.seed, &successor).await;
        let peer_schedule = available_at(&f.peer, &successor).await;
        assert_ne!(seed_schedule, peer_schedule);
        // Explicit edits on the peer's own generated task.
        f.peer
            .update_task(
                &w,
                &successor,
                TaskUpdate {
                    description: Some("peer explicit".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        let note = f
            .peer
            .add_note(&w, &successor, "peer note".into())
            .await
            .unwrap();
        let c = Client::new(&f.origin).unwrap();
        match mode {
            Acceptance::SeedFirst => {
                drain(&c, &f.seed_store, &f.seed).await;
                drain(&c, &f.peer_store, &f.peer).await;
            }
            Acceptance::PeerFirst => {
                drain(&c, &f.peer_store, &f.peer).await;
                drain(&c, &f.seed_store, &f.seed).await;
            }
            Acceptance::PeerFrozenAfterSeed => {
                drain(&c, &f.seed_store, &f.seed).await;
                let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
                for _ in 0..64 {
                    if f.peer
                        .encrypted_round_state(&inputs.authority)
                        .await
                        .unwrap()
                        .idle
                    {
                        break;
                    }
                    c.push(&inputs.authority, &inputs.bearer, &f.peer, &blobs(&f.peer))
                        .await
                        .unwrap();
                }
            }
        }
        converge(&f).await;
        let (winner, schedule) = match mode {
            Acceptance::PeerFirst => ("peer", &peer_schedule),
            _ => ("seed", &seed_schedule),
        };
        for db in [&f.seed, &f.peer] {
            assert_eq!(
                title(db, successor.as_str()).await,
                format!("{winner} successor"),
                "{mode:?}"
            );
            assert_eq!(&available_at(db, &successor).await, schedule, "{mode:?}");
            assert_eq!(
                labels(db, &successor).await,
                vec![
                    "gone".to_string(),
                    format!("{winner}-label"),
                    "shared".into()
                ]
            );
            assert_eq!(
                text(db, KIND, successor.as_str()).await,
                Some(winner.into())
            );
            assert_eq!(
                text(db, "SELECT body FROM notes WHERE id = ?", &note.note_id).await,
                Some("peer note".into())
            );
            assert_eq!(
                scalar(db, "SELECT count(*) FROM recurrence_occurrences").await,
                2
            );
            // Concurrent template edits stay explicit series conflicts.
            assert!(
                !db.recurrence_series_conflicts(&w, &created.series.id, Some("title"))
                    .await
                    .unwrap()
                    .is_empty()
            );
            assert_idle(db).await;
        }
        if winner == "peer" {
            // The explicit edit was based on the accepted defaults.
            for db in [&f.seed, &f.peer] {
                assert!(task_conflicts(db, &successor).await.is_empty());
            }
        } else {
            // An edit based on the losing defaults stays explicit on both sides.
            for db in [&f.seed, &f.peer] {
                assert_eq!(task_conflicts(db, &successor).await, vec!["description"]);
            }
            f.seed
                .resolve_conflict(&w, &successor, "description", "peer explicit")
                .await
                .unwrap();
        }
        // Later ordinary work keeps flowing.
        f.peer
            .update_task(
                &w,
                &successor,
                TaskUpdate {
                    title: Some("later edit".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        converge(&f).await;
        let third = super::super::membership::join(&f, "third", &f.seed, &f.seed_store).await;
        drain(&c, &third.store, &third.db).await;
        for db in [&f.seed, &f.peer, &third.db] {
            assert_eq!(
                title(db, successor.as_str()).await,
                "later edit",
                "{mode:?}"
            );
            assert_eq!(
                text(
                    db,
                    "SELECT description FROM tasks WHERE id = ?",
                    successor.as_str()
                )
                .await,
                Some("peer explicit".into())
            );
            assert_eq!(&available_at(db, &successor).await, schedule);
            assert_eq!(
                text(db, KIND, successor.as_str()).await,
                Some(winner.into())
            );
            assert!(task_conflicts(db, &successor).await.is_empty());
        }
    }
}

#[tokio::test]
async fn losing_generation_keeps_explicit_completion_metadata_and_label_changes() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let created = create(&f.seed).await;
    converge(&f).await;
    let successor = diverge(&f, &w, &created).await;
    // The peer's generation will lose; its explicit work must survive.
    f.peer
        .update_task(
            &w,
            &successor,
            TaskUpdate {
                set_metadata: vec![TaskMetadataInput {
                    expected_field_id: None,
                    key: "kind".into(),
                    value: "peer explicit".into(),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.peer.delete_label(&w, "gone").await.unwrap();
    f.peer
        .rename_label_with_tui_undo(&w, "shared", "renamed")
        .await
        .unwrap();
    f.peer
        .update_task(
            &w,
            &successor,
            TaskUpdate {
                status: Some("done".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let c = Client::new(&f.origin).unwrap();
    drain(&c, &f.seed_store, &f.seed).await;
    drain(&c, &f.peer_store, &f.peer).await;
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            labels(db, &successor).await,
            vec!["renamed".to_string(), "seed-label".into()]
        );
        assert_eq!(
            scalar(
                db,
                "SELECT count(*) FROM labels WHERE name IN ('gone', 'shared')"
            )
            .await,
            0
        );
        assert_eq!(
            text(
                db,
                "SELECT status FROM tasks WHERE id = ?",
                successor.as_str()
            )
            .await,
            Some("done".into())
        );
        assert_eq!(
            text(
                db,
                "SELECT outcome FROM recurrence_occurrences WHERE task_id = ?",
                successor.as_str()
            )
            .await,
            Some("completed".into())
        );
        // The completion generated one more occurrence, which converges too.
        assert_eq!(
            scalar(db, "SELECT count(*) FROM recurrence_occurrences").await,
            3
        );
        let conflicts = task_conflicts(db, &successor).await;
        assert_eq!(conflicts.len(), 1, "{conflicts:?}");
        assert!(conflicts[0].starts_with("metadata:"));
        assert_idle(db).await;
    }
    assert_eq!(
        text(&f.peer, KIND, successor.as_str()).await,
        Some("peer explicit".into())
    );
}

#[tokio::test]
async fn earlier_outcome_applies_after_concurrent_availability_edit() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let created = create(&f.seed).await;
    converge(&f).await;
    f.peer
        .update_recurrence_template(
            &w,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                available_local_time: Some(Some("11:00:00".parse().unwrap())),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    f.seed
        .update_task(
            &w,
            &created.task.id,
            TaskUpdate {
                status: Some("done".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let c = Client::new(&f.origin).unwrap();
    drain(&c, &f.peer_store, &f.peer).await;
    drain(&c, &f.seed_store, &f.seed).await;
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            text(
                db,
                "SELECT outcome FROM recurrence_occurrences WHERE task_id = ?",
                created.task.id.as_str()
            )
            .await,
            Some("completed".into())
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM recurrence_occurrences").await,
            2
        );
        assert_idle(db).await;
    }
}

/// An explicit non-terminal status edit racing a completion from another device.
/// Current policy has no rule for applying that completion over the explicit edit:
/// the editing device stops with pending work retained, while the completing
/// device keeps the completion and shows a status conflict.
#[tokio::test]
async fn status_edit_racing_completion_stops_the_editing_device() {
    for seed_first in [true, false] {
        let f = fixture().await;
        converge(&f).await;
        let w = f.seed.list_workspaces().await.unwrap().remove(0);
        let created = create(&f.seed).await;
        converge(&f).await;
        let task = created.task.id.as_str();
        for (db, status) in [(&f.seed, "active"), (&f.peer, "done")] {
            db.update_task(
                &w,
                &created.task.id,
                TaskUpdate {
                    status: Some(status.into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        let c = Client::new(&f.origin).unwrap();
        if seed_first {
            // The completing device receives the edit first and keeps a conflict.
            drain(&c, &f.seed_store, &f.seed).await;
            drain(&c, &f.peer_store, &f.peer).await;
        } else {
            drain(&c, &f.peer_store, &f.peer).await;
        }
        let cursor = f.seed.meta("sync_cursor").await.unwrap();
        let mut errors = Vec::new();
        for _ in 0..8 {
            if let Err(error) = c.round(&f.seed_store, &f.seed, &blobs(&f.seed)).await {
                errors.push(error.to_string());
            }
        }
        assert_eq!(errors.len(), 8, "seed_first={seed_first}");
        assert!(
            errors.iter().all(|e| e == "error encrypted-tail-apply"),
            "{errors:?}"
        );
        assert_eq!(f.seed.meta("sync_cursor").await.unwrap(), cursor);
        assert_eq!(
            text(&f.seed, "SELECT status FROM tasks WHERE id = ?", task).await,
            Some("active".into())
        );
        assert_eq!(
            text(
                &f.seed,
                "SELECT outcome FROM recurrence_occurrences WHERE task_id = ?",
                task
            )
            .await,
            Some(String::new())
        );
        drain(&c, &f.peer_store, &f.peer).await;
        assert_eq!(
            text(&f.peer, "SELECT status FROM tasks WHERE id = ?", task).await,
            Some("done".into())
        );
        assert_eq!(
            text(
                &f.peer,
                "SELECT outcome FROM recurrence_occurrences WHERE task_id = ?",
                task
            )
            .await,
            Some("completed".into())
        );
        assert_eq!(
            task_conflicts(&f.peer, &created.task.id).await,
            vec!["status"]
        );
    }
}
