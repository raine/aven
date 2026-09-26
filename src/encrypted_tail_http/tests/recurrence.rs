use super::*;
use aven_core::{
    metadata::TaskMetadataInput,
    operations::{
        CreateRecurrenceSeriesParams, RecurrenceSeriesDraft, RecurrenceTemplateUpdate,
        UpdateRecurrenceTemplateParams,
    },
    recurrence::{RecurrenceDuePolicy, RecurrenceRule, RecurrenceSchedule},
};
use chrono::Utc;

pub(super) async fn create(db: &Database) -> aven_core::operations::RecurrenceCreateOutcome {
    create_at(db, Utc::now()).await
}

async fn create_at(
    db: &Database,
    at: chrono::DateTime<Utc>,
) -> aven_core::operations::RecurrenceCreateOutcome {
    let w = db.list_workspaces().await.unwrap().remove(0);
    db.create_recurrence_series(
        &w,
        CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
            title: "daily".into(),
            description: "template".into(),
            project: "app".into(),
            priority: "none".into(),
            initial_status: "todo".into(),
            labels: vec![],
            metadata: vec![TaskMetadataInput {
                expected_field_id: None,
                key: "kind".into(),
                value: "daily".into(),
            }],
            schedule: RecurrenceSchedule::new(
                RecurrenceRule::daily(),
                "UTC".parse().unwrap(),
                at.date_naive(),
                None,
                RecurrenceDuePolicy::SameDay,
            ),
        })
        .at(at),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn creation_template_completion_and_lifecycle() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let created = create(&f.seed).await;
    converge(&f).await;
    assert_eq!(title(&f.peer, created.task.id.as_str()).await, "daily");
    f.seed
        .update_recurrence_template(
            &w,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                title: Some("updated".into()),
                labels: Some(vec!["updated-label".into()]),
                set_metadata: vec![TaskMetadataInput {
                    expected_field_id: None,
                    key: "kind".into(),
                    value: "updated".into(),
                }],
                ..Default::default()
            })
            .with_create_missing_labels(),
        )
        .await
        .unwrap();
    converge(&f).await;
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
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(
                db,
                "SELECT count(*) FROM recurrence_occurrences WHERE outcome='completed'"
            )
            .await,
            1
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM tasks WHERE title='updated'").await,
            1
        );
        assert_eq!(
            scalar(
                db,
                "SELECT count(*) FROM task_metadata WHERE value='updated'"
            )
            .await,
            1
        );
        assert_eq!(
            scalar(
                db,
                "SELECT count(*) FROM task_labels WHERE label='updated-label'"
            )
            .await,
            1
        );
    }
    f.seed
        .update_recurrence_template(
            &w,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                remove_metadata: vec!["kind".into()],
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(&f.peer, "SELECT count(*) FROM recurrence_series_metadata").await,
        0
    );
    f.seed
        .pause_recurrence_series(&w, &created.series.id)
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM recurrence_series WHERE state='paused'"
        )
        .await,
        1
    );
    f.seed
        .resume_recurrence_series(&w, &created.series.id, Utc::now())
        .await
        .unwrap();
    converge(&f).await;
    f.seed
        .stop_recurrence_series(&w, &created.series.id, false)
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM recurrence_series WHERE state='stopped'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn concurrent_completion_preserves_deterministic_successor_identity() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let created = create(&f.seed).await;
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        db.update_task(
            &w,
            &created.task.id,
            TaskUpdate {
                status: Some("done".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let c = Client::new(&f.origin).unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    // Publish the outcome without pulling, then freeze the independently generated successor.
    for _ in 0..2 {
        c.push(&inputs, &f.peer, &blobs(&f.peer)).await.unwrap();
    }
    let frozen = head_record(&f.peer, &inputs.authority).await;
    drop(inputs);
    let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    while !f
        .seed
        .encrypted_round_state(&inputs.authority)
        .await
        .unwrap()
        .idle
    {
        c.push(&inputs, &f.seed, &blobs(&f.seed)).await.unwrap();
    }
    drop(inputs);
    let id: String = sqlx::query_scalar("SELECT operation_id FROM local_e2ee_outbox")
        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
        .await
        .unwrap();
    let accepted: Vec<u8> =
        sqlx::query_scalar("SELECT record FROM server_e2ee_tail WHERE operation_id=?")
            .bind(id)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
            .await
            .unwrap();
    assert_ne!(frozen, accepted);
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM recurrence_occurrences").await,
            2
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM conflicts WHERE resolved=0").await,
            0
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM changes WHERE server_seq IS NULL").await,
            0
        );
    }
}

#[tokio::test]
async fn snapshot_baseline_continues_without_replaying_prefix() {
    let f = fixture_with(FixtureOptions {
        recurrence: true,
        ..Default::default()
    })
    .await;
    converge(&f).await;
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    let task: aven_core::ids::TaskId =
        sqlx::query_scalar("SELECT task_id FROM recurrence_occurrences")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    assert_eq!(
        title(&f.peer, task.as_str()).await,
        "snapshot occurrence edit"
    );
    f.peer
        .update_task(
            &w,
            &task,
            TaskUpdate {
                status: Some("done".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(title(db, task.as_str()).await, "snapshot occurrence edit");
        assert_eq!(
            scalar(db, "SELECT count(*) FROM recurrence_occurrences").await,
            2
        );
    }
    assert_eq!(scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail t JOIN server_bootstrap_prefix p ON p.operation_id=t.operation_id").await, 0);
}

#[tokio::test]
async fn outcome_and_template_conflicts_remain_explicit_and_resolve() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let created = create(&f.seed).await;
    converge(&f).await;
    for (db, status, title) in [
        (&f.seed, "done", "seed title"),
        (&f.peer, "canceled", "peer title"),
    ] {
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
        db.update_recurrence_template(
            &w,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                title: Some(title.into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    }
    converge(&f).await;
    let outcome = format!("outcome:{}", created.occurrence.slot_on);
    for db in [&f.seed, &f.peer] {
        let conflicts = db
            .recurrence_series_conflicts(&w, &created.series.id, None)
            .await
            .unwrap();
        assert_eq!(conflicts.len(), 2);
    }
    f.seed
        .resolve_recurrence_conflict(&w, &created.series.id, &outcome, "completed")
        .await
        .unwrap();
    f.seed
        .resolve_recurrence_conflict(&w, &created.series.id, "title", "seed title")
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert!(
            db.recurrence_series_conflicts(&w, &created.series.id, None)
                .await
                .unwrap()
                .is_empty()
        );
    }
}

/// Occurrence-form identities ignore template content. Unequal history that shares
/// those identities is refused, never rewritten.
#[tokio::test]
async fn occurrence_form_generations_from_different_templates_still_refuse() {
    let f = fixture().await;
    for db in [&f.seed, &f.peer] {
        aven_core::test_support::use_occurrence_form_generation(db)
            .await
            .unwrap();
    }
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let created = create(&f.seed).await;
    converge(&f).await;
    for (db, title) in [(&f.seed, "seed successor"), (&f.peer, "peer successor")] {
        db.update_recurrence_template(
            &w,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                title: Some(title.into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        db.update_task(
            &w,
            &created.task.id,
            TaskUpdate {
                status: Some("done".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let c = Client::new(&f.origin).unwrap();
    drain(&c, &f.seed_store, &f.seed).await;
    let cursor = f.peer.meta("sync_cursor").await.unwrap();
    let err = c
        .round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("same-id-divergence"), "{err:#}");
    assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), cursor);
    assert!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await
            > 0
    );
}

#[tokio::test]
async fn lost_ack_reopen_reuses_frozen_recurrence_bytes() {
    let f = fixture().await;
    converge(&f).await;
    create(&f.peer).await;
    let c = Client::new(&f.origin).unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let record = head_record(&f.peer, &inputs.authority).await;
    let Reply::Appended(mapping) = c
        .exchange(
            &inputs.authority.context,
            &inputs.bearer,
            Operation::Append {
                ticket: None,
                record: record.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    drop(inputs);
    let reopened = Database::open(f.peer.path()).await.unwrap();
    let inputs = f
        .peer_store
        .tail_inputs(&reopened, &f.origin)
        .await
        .unwrap();
    assert_eq!(
        reopened
            .prepare_encrypted_push(&inputs.authority, &blobs(&reopened))
            .await
            .unwrap()
            .map(|push| push.record),
        Some(record.clone())
    );
    let Reply::Appended(retry) = c
        .exchange(
            &inputs.authority.context,
            &inputs.bearer,
            Operation::Append {
                ticket: None,
                record,
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(mapping == retry);
    drop(inputs);
    drain(&c, &f.peer_store, &reopened).await;
    converge(&f).await;
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM recurrence_series").await,
        1
    );
}

#[tokio::test]
async fn malformed_compound_template_rolls_back_page_and_cursor() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let created = create(&f.seed).await;
    converge(&f).await;
    f.seed
        .update_recurrence_template(
            &w,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                title: Some("must roll back".into()),
                project: Some("other".into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    // A validly encrypted but state-invalid compound operation from an authorized writer.
    sqlx::query("UPDATE changes SET payload=json_set(payload, '$.fields[1][1]', 'AAAAAAAAAAAAAAAA') WHERE op_type='update_recurrence_template' AND server_seq IS NULL")
        .execute(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap()).await.unwrap();
    let c = Client::new(&f.origin).unwrap();
    drain(&c, &f.seed_store, &f.seed).await;
    let cursor = f.peer.meta("sync_cursor").await.unwrap();
    let history = scalar(&f.peer, "SELECT count(*) FROM changes").await;
    assert!(c.pull_only_round(&f.peer_store, &f.peer).await.is_err());
    assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), cursor);
    assert_eq!(
        scalar(&f.peer, "SELECT count(*) FROM changes").await,
        history
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM recurrence_series WHERE title='daily'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn malformed_recurrence_suffix_blocks_the_entire_pending_prefix() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    f.seed
        .create_task(&w, draft("unrelated pending task"))
        .await
        .unwrap();
    create(&f.seed).await;
    sqlx::query("UPDATE changes SET payload=json_set(payload, '$.task_change_id', 'AAAAAAAAAAAAAAAA') WHERE op_type='project_recurrence_occurrence' AND server_seq IS NULL")
        .execute(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap()).await.unwrap();
    let before = scalar(&f.server, "SELECT high_water FROM server_e2ee_allocator").await;
    assert!(
        Client::new(&f.origin)
            .unwrap()
            .round(&f.seed_store, &f.seed, f.root.path())
            .await
            .is_err()
    );
    assert_eq!(
        scalar(&f.server, "SELECT high_water FROM server_e2ee_allocator").await,
        before
    );
    assert_eq!(
        scalar(&f.seed, "SELECT count(*) FROM local_e2ee_outbox").await,
        0
    );
}

#[tokio::test]
async fn lifecycle_conflict_resolution_and_unrelated_work_converge() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let created = create(&f.seed).await;
    converge(&f).await;
    f.seed
        .pause_recurrence_series(&w, &created.series.id)
        .await
        .unwrap();
    f.peer
        .stop_recurrence_series(&w, &created.series.id, false)
        .await
        .unwrap();
    f.peer
        .create_task(&w, draft("unrelated work"))
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            db.recurrence_series_conflicts(&w, &created.series.id, Some("state"))
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            scalar(
                db,
                "SELECT count(*) FROM tasks WHERE title='unrelated work'"
            )
            .await,
            1
        );
    }
    f.peer
        .resolve_recurrence_conflict(&w, &created.series.id, "state", "stopped")
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert!(
            db.recurrence_series_conflicts(&w, &created.series.id, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            scalar(
                db,
                "SELECT count(*) FROM recurrence_series WHERE state='stopped'"
            )
            .await,
            1
        );
    }
}

#[tokio::test]
async fn occurrence_images_survive_completion_and_follow_explicit_deletion() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    let created = create(&f.peer).await;
    converge(&f).await;
    let bytes = files(&f.root.path().join("objects/sha256")).remove(0);
    f.peer
        .add_task_attachment(
            &w,
            &f.root.path().join("peer-blobs"),
            Default::default(),
            &created.task.id,
            aven_core::operations::AttachmentAddInput {
                filename: None,
                alt_text: None,
                declared_media_type: None,
                bytes,
                optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        )
        .await
        .unwrap();
    let c = Client::new(&f.origin).unwrap();
    c.round(&f.peer_store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    c.round(&f.seed_store, &f.seed, f.root.path())
        .await
        .unwrap();
    f.peer
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
    converge(&f).await;
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_parents WHERE deleted=1"
        )
        .await,
        0
    );
    f.peer
        .update_task(
            &w,
            &created.task.id,
            TaskUpdate {
                deleted: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted FROM server_e2ee_image_parents WHERE parent=?")
            .bind(&created.task.id)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
            .await
            .unwrap();
    assert!(deleted);
    // Completion and parent deletion retain reference identity; only Unref deletes it.
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_image_references WHERE deleted=1"
        )
        .await,
        0
    );
}

mod proposals;
mod replay;
