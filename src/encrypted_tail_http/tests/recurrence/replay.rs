use super::*;

async fn push_only(c: &Client, store: &ProtectedLocalKeyStore, db: &Database, origin: &str) {
    let inputs = store.tail_inputs(db, origin).await.unwrap();
    for _ in 0..32 {
        if db.encrypted_tail_idle(&inputs.authority).await.unwrap() {
            return;
        }
        c.push(&inputs.authority, &inputs.bearer, db, None)
            .await
            .unwrap();
    }
    panic!("bounded fixture push budget");
}

fn historical_time() -> chrono::DateTime<Utc> {
    Utc::now()
        .date_naive()
        .and_hms_opt(12, 0, 0)
        .unwrap()
        .and_utc()
        - chrono::Duration::days(2)
}

#[tokio::test]
async fn lifecycle_resolution_reinserts_later_page_records_with_canonical_equality() {
    for unequal in [false, true] {
        let f = fixture().await;
        converge(&f).await;
        let c = Client::new(&f.origin).unwrap();
        let w = f.seed.list_workspaces().await.unwrap().remove(0);
        let old = historical_time();
        let created = create_at(&f.seed, old).await;
        aven_core::test_support::pause_recurrence_series_at(
            &f.seed,
            &w,
            &created.series.id,
            old + chrono::Duration::minutes(1),
        )
        .await
        .unwrap();
        // Deliver the complete paused aggregate in one page, without advancing it to today.
        push_only(&c, &f.seed_store, &f.seed, &f.origin).await;
        assert!(c.pull_only_round(&f.peer_store, &f.peer).await.unwrap());
        assert!(c.pull_only_round(&f.seed_store, &f.seed).await.unwrap());
        f.seed
            .resume_recurrence_series(&w, &created.series.id, old + chrono::Duration::minutes(2))
            .await
            .unwrap();
        f.peer
            .stop_recurrence_series(&w, &created.series.id, false)
            .await
            .unwrap();
        push_only(&c, &f.seed_store, &f.seed, &f.origin).await;
        push_only(&c, &f.peer_store, &f.peer, &f.origin).await;
        assert!(c.pull_only_round(&f.seed_store, &f.seed).await.unwrap());
        assert!(c.pull_only_round(&f.peer_store, &f.peer).await.unwrap());
        for db in [&f.seed, &f.peer] {
            assert_eq!(
                db.recurrence_series_conflicts(&w, &created.series.id, Some("state"))
                    .await
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                scalar(db, "SELECT count(*) FROM recurrence_occurrences").await,
                1
            );
        }
        f.seed
            .resolve_recurrence_conflict(&w, &created.series.id, "state", "active")
            .await
            .unwrap();
        let pending: Vec<(String, String)> = sqlx::query_as(
            "SELECT change_id, op_type FROM changes WHERE server_seq IS NULL ORDER BY local_seq",
        )
        .fetch_all(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap())
        .await
        .unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|(_, op)| op.as_str())
                .collect::<Vec<_>>(),
            [
                "set_recurrence_state",
                "create_task",
                "project_recurrence_occurrence"
            ]
        );
        if unequal {
            // Negative authorized-writer input, still encrypted and accepted through real HTTP.
            sqlx::query("UPDATE changes SET payload=json_set(payload, '$.title', 'unequal generated task') WHERE change_id=?")
                .bind(&pending[1].0).execute(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap()).await.unwrap();
        }
        push_only(&c, &f.seed_store, &f.seed, &f.origin).await;
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        let before = f
            .peer
            .encrypted_tail_cursor(&inputs.authority)
            .await
            .unwrap();
        let Reply::Page(page) = c
            .exchange(
                &inputs.authority.context,
                &inputs.bearer,
                Operation::Pull {
                    after: before,
                    limit: 16,
                    watermark: None,
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            page.records
                .iter()
                .map(|r| &r.mapping.operation_id)
                .collect::<Vec<_>>(),
            pending.iter().map(|(id, _)| id).collect::<Vec<_>>()
        );
        // Neither generated ID existed when the page's initial local-presence pass began.
        for (id, _) in &pending {
            let exists: bool =
                sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM changes WHERE change_id=?)")
                    .bind(id)
                    .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
                    .await
                    .unwrap();
            assert!(!exists);
        }
        let result = f
            .peer
            .apply_encrypted_tail_page(&inputs.authority, &page)
            .await;
        if unequal {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("same-id-divergence")
            );
            assert_eq!(
                f.peer
                    .encrypted_tail_cursor(&inputs.authority)
                    .await
                    .unwrap(),
                before
            );
            assert_eq!(
                scalar(&f.peer, "SELECT count(*) FROM recurrence_occurrences").await,
                1
            );
            assert_eq!(
                f.peer
                    .recurrence_series_conflicts(&w, &created.series.id, Some("state"))
                    .await
                    .unwrap()
                    .len(),
                1
            );
            for (id, _) in &pending {
                let exists: bool =
                    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM changes WHERE change_id=?)")
                        .bind(id)
                        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
                        .await
                        .unwrap();
                assert!(!exists);
            }
        } else {
            result.unwrap();
            assert_eq!(
                f.peer
                    .encrypted_tail_cursor(&inputs.authority)
                    .await
                    .unwrap(),
                page.cursor
            );
            assert_eq!(
                scalar(&f.peer, "SELECT count(*) FROM recurrence_occurrences").await,
                2
            );
            assert_eq!(
                scalar(
                    &f.peer,
                    "SELECT count(*) FROM changes WHERE server_seq IS NULL"
                )
                .await,
                0
            );
            for record in &page.records[1..] {
                let (origin, rank): (String, i64) =
                    sqlx::query_as("SELECT client_id, server_seq FROM changes WHERE change_id=?")
                        .bind(&record.mapping.operation_id)
                        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
                        .await
                        .unwrap();
                // A plain remote insert would carry seed provenance, not this local origin.
                assert_eq!(Some(origin), f.peer.meta("client_id").await.unwrap());
                assert_eq!(rank, record.mapping.sequence);
            }
            assert!(
                f.peer
                    .recurrence_series_conflicts(&w, &created.series.id, None)
                    .await
                    .unwrap()
                    .is_empty()
            );
        }
    }
}

#[tokio::test]
async fn historical_small_pages_expose_premature_clock_projection() {
    let f = fixture().await;
    converge(&f).await;
    let c = Client::new(&f.origin).unwrap();
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let created = create_at(&f.seed, historical_time()).await;
    f.seed
        .update_recurrence_template(
            &w,
            &created.series.id,
            UpdateRecurrenceTemplateParams::new(RecurrenceTemplateUpdate {
                title: Some("sequentially edited template".into()),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
    let current = f
        .seed
        .reconcile_recurrence_series(&w, &created.series.id, Utc::now())
        .await
        .unwrap();
    let task = current.occurrence.unwrap().task_id.unwrap();
    assert_eq!(
        title(&f.seed, task.as_str()).await,
        "sequentially edited template"
    );
    let task_change: String = sqlx::query_scalar(
        "SELECT change_id FROM changes WHERE entity_id=? AND op_type='create_task'",
    )
    .bind(&task)
    .fetch_one(&mut *aven_core::test_support::acquire(&f.seed).await.unwrap())
    .await
    .unwrap();
    // One author, all records accepted before the peer starts replaying the backlog.
    push_only(&c, &f.seed_store, &f.seed, &f.origin).await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let mut watermark = None;
    let mut prematurely_created = false;
    for _ in 0..16 {
        let before = f
            .peer
            .encrypted_tail_cursor(&inputs.authority)
            .await
            .unwrap();
        let Reply::Page(page) = c
            .exchange(
                &inputs.authority.context,
                &inputs.bearer,
                Operation::Pull {
                    after: before,
                    limit: 1,
                    watermark,
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        watermark = Some(page.watermark);
        assert_eq!(page.records.len(), 1);
        if let Err(error) = f
            .peer
            .apply_encrypted_tail_page(&inputs.authority, &page)
            .await
        {
            assert!(
                error.to_string().contains("same-id-divergence"),
                "{error:#}"
            );
            assert!(prematurely_created);
            assert_eq!(page.records[0].mapping.operation_id, task_change);
            assert_eq!(
                f.peer
                    .encrypted_tail_cursor(&inputs.authority)
                    .await
                    .unwrap(),
                before
            );
            assert_eq!(title(&f.peer, task.as_str()).await, "daily");
            return;
        }
        let early: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM tasks WHERE id=? AND title='daily')")
                .bind(&task)
                .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
                .await
                .unwrap();
        prematurely_created |= early;
        assert!(
            page.has_more,
            "expected current-slot divergence before backlog end"
        );
    }
    panic!("bounded fixture pull budget");
}
