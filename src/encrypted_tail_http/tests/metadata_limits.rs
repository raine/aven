use super::*;
use aven_core::metadata::TaskMetadataInput;

fn input(key: impl Into<String>, value: impl Into<String>) -> TaskMetadataInput {
    TaskMetadataInput {
        expected_field_id: None,
        key: key.into(),
        value: value.into(),
    }
}

async fn drain_metadata(c: &Client, store: &ProtectedLocalKeyStore, db: &Database) {
    for _ in 0..512 {
        if c.round(store, db, &blobs(db))
            .await
            .unwrap()
            .metadata_caught_up
        {
            return;
        }
    }
    panic!("metadata round budget");
}

async fn assert_integrity(db: &Database) {
    let report = db.database_integrity_report().await.unwrap();
    assert!(report.quick_check_ok);
    assert!(report.checks.iter().all(|check| check.ok), "{report:?}");
}

async fn concurrent_additions(bytes: bool, peer_first: bool) {
    let f = fixture().await;
    converge(&f).await;
    let c = Client::new(&f.origin).unwrap();
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    // Define keys before going offline so each addition is one value operation.
    let mut definitions = draft("field definitions");
    definitions.metadata = vec![input("seed-key", "x"), input("peer-key", "x")];
    f.seed.create_task(&w, definitions).await.unwrap();
    let mut d = draft("concurrent metadata");
    let (base_count, value) = if bytes {
        (7, "x".repeat(4096))
    } else {
        (127, "x".into())
    };
    d.metadata = (0..base_count)
        .map(|i| input(format!("base-{i}"), value.clone()))
        .collect();
    let task = f.seed.create_task(&w, d).await.unwrap().task;
    drain_metadata(&c, &f.seed_store, &f.seed).await;
    drain_metadata(&c, &f.peer_store, &f.peer).await;
    let baseline = f.seed.task_metadata(&w.id, &task.id).await.unwrap();
    assert_eq!(
        baseline,
        f.peer.task_metadata(&w.id, &task.id).await.unwrap()
    );
    for (db, key) in [(&f.seed, "seed-key"), (&f.peer, "peer-key")] {
        db.update_task(
            &w,
            &task.id,
            TaskUpdate {
                set_metadata: vec![input(key, value.clone())],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let devices = if peer_first {
        [(&f.peer, &f.peer_store), (&f.seed, &f.seed_store)]
    } else {
        [(&f.seed, &f.seed_store), (&f.peer, &f.peer_store)]
    };
    for (db, store) in devices {
        drain_metadata(&c, store, db).await;
    }
    converge(&f).await;
    let merged = f.seed.task_metadata(&w.id, &task.id).await.unwrap();
    assert_eq!(merged.len(), base_count + 2);
    assert_eq!(
        merged.iter().map(|v| v.value.len()).sum::<usize>(),
        (base_count + 2) * value.len()
    );
    for original in baseline {
        assert!(merged.contains(&original));
    }
    for key in ["seed-key", "peer-key"] {
        assert!(merged.iter().any(|v| v.key == key && v.value == value));
    }
    for (index, (db, store)) in devices.into_iter().enumerate() {
        for _ in 0..3 {
            assert!(
                c.round(store, db, &blobs(db))
                    .await
                    .unwrap()
                    .metadata_caught_up
            );
            assert_eq!(merged, db.task_metadata(&w.id, &task.id).await.unwrap());
        }
        assert!(
            db.task_conflicts(&w, &task.id, None)
                .await
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM changes WHERE server_seq IS NULL").await,
            0
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM local_e2ee_outbox").await,
            0
        );
        let integrity = db.database_integrity_report().await.unwrap();
        assert!(integrity.quick_check_ok);
        // Live replicas have independent local_seq counters; check the metadata
        // invariants here, and the complete integrity report after installation.
        assert!(
            integrity
                .checks
                .iter()
                .filter(|check| check.label.contains("metadata"))
                .all(|check| check.ok),
            "{integrity:?}"
        );

        // Transient capture/install is a shared-data contract, not E2EE recovery.
        let capture = db.capture_shared_state().await.unwrap();
        let installed = Database::open(&f.root.path().join(format!("captured-{index}.sqlite")))
            .await
            .unwrap();
        installed.install_shared_state(&capture).await.unwrap();
        assert_eq!(
            merged,
            installed.task_metadata(&w.id, &task.id).await.unwrap()
        );
        assert_integrity(&installed).await;
        let portable = installed
            .export_data("2026-09-22T00:00:00Z".into())
            .await
            .unwrap();
        let imported = Database::open(&f.root.path().join(format!("imported-{index}.sqlite")))
            .await
            .unwrap();
        imported.validate_import_data(&portable).await.unwrap();
        imported.import_data(&portable).await.unwrap();
        assert_eq!(
            merged,
            imported.task_metadata(&w.id, &task.id).await.unwrap()
        );
        assert_integrity(&imported).await;
        let bound_export = db.export_data("2026-09-22T00:00:00Z".into()).await.unwrap();
        assert_eq!(
            imported
                .validate_import_data(&bound_export)
                .await
                .unwrap_err()
                .to_string(),
            "error e2ee-data-only-import-unavailable"
        );
    }

    // A byte reduction need not reach the aggregate ceiling in one edit.
    f.seed
        .update_task(
            &w,
            &task.id,
            TaskUpdate {
                set_metadata: vec![input("base-0", &value[..value.len() - 1])],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.peer
        .update_task(
            &w,
            &task.id,
            TaskUpdate {
                title: Some("sync continues".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    let reduced = f.seed.task_metadata(&w.id, &task.id).await.unwrap();
    assert_eq!(reduced.len(), merged.len());
    assert_eq!(
        reduced.iter().map(|v| v.value.len()).sum::<usize>(),
        (base_count + 2) * value.len() - 1
    );
    assert_eq!(
        reduced,
        f.peer.task_metadata(&w.id, &task.id).await.unwrap()
    );
    for (db, store) in devices {
        assert_eq!(title(db, task.id.as_str()).await, "sync continues");
        assert!(
            c.round(store, db, &blobs(db))
                .await
                .unwrap()
                .metadata_caught_up
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM changes WHERE server_seq IS NULL").await,
            0
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM local_e2ee_outbox").await,
            0
        );
    }
    assert_eq!(
        f.seed.meta("sync_cursor").await.unwrap(),
        f.peer.meta("sync_cursor").await.unwrap()
    );
}

#[tokio::test]
async fn count_seed_first() {
    concurrent_additions(false, false).await;
}

#[tokio::test]
async fn count_peer_first() {
    concurrent_additions(false, true).await;
}

#[tokio::test]
async fn bytes_seed_first() {
    concurrent_additions(true, false).await;
}

#[tokio::test]
async fn bytes_peer_first() {
    concurrent_additions(true, true).await;
}
