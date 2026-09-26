use super::*;

#[tokio::test]
async fn fresh_install_after_legitimate_bootstrap_image_prune_keeps_metadata_and_tail() {
    use aven_core::sync::encrypted_tail::{self as tail, attachments as images};
    let f = enrolled().await;
    let seed_store = isolated_store(&f.source, &f.root.path().join("keys")).await;
    let workspace = f.source.list_workspaces().await.unwrap().remove(0);
    let reference: String = sqlx::query_scalar("SELECT attachment_id FROM task_attachments")
        .fetch_one(&mut *aven_core::test_support::acquire(&f.source).await.unwrap())
        .await
        .unwrap();
    f.source
        .delete_task_attachment(&workspace, &reference)
        .await
        .unwrap();
    {
        let inputs = seed_store
            .tail_inputs(&f.source, &f.client.locator)
            .await
            .unwrap();
        let record = f
            .source
            .prepare_encrypted_push(&inputs.authority, f.root.path())
            .await
            .unwrap()
            .unwrap()
            .record;
        let tail::Reply::Appended(mapping) = f
            .server
            .encrypted_tail_exchange(
                &inputs.authority.context,
                &inputs.bearer,
                tail::Operation::Append {
                    ticket: None,
                    record: record.clone(),
                },
            )
            .await
            .unwrap()
        else {
            panic!("accepted Unref")
        };
        f.source
            .observe_encrypted_tail(&inputs.authority, &mapping)
            .await
            .unwrap();
        f.source
            .verify_encrypted_tail_outcome(&inputs.authority, &tail::Accepted { mapping, record })
            .await
            .unwrap();
        assert!(count(&f.server, "server_e2ee_image_chunks").await > 0);
        let mut policy = crate::config::AttachmentLifecycleConfig::default().server_policy();
        policy.grace = std::time::Duration::ZERO;
        let images::Reply::Pruned(pruned) = f
            .server
            .encrypted_image_exchange(
                &inputs.authority.context,
                &inputs.bearer,
                images::Operation::Prune { limit: 128 },
                policy,
            )
            .await
            .unwrap()
        else {
            panic!("prune result")
        };
        assert_eq!(pruned, 1);
        assert_eq!(count(&f.server, "server_e2ee_image_chunks").await, 0);
        assert_eq!(count(&f.server, "server_e2ee_images").await, 1);
    }
    assert_eq!(count(&f.peer, "tasks").await, 0);
    let installed = f.client.install(&f.store, &f.peer).await;
    assert!(
        installed.is_ok(),
        "legitimate pruning must not block metadata installation: {installed:?}"
    );
    assert!(count(&f.peer, "tasks").await > 0);
    let transfer = crate::encrypted_tail_http::Client::new(&f.client.locator)
        .unwrap()
        .round(&f.store, &f.peer, &f.root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert!(transfer.metadata_caught_up);
    assert_eq!(
        transfer.images,
        crate::encrypted_tail_http::ImageTransfer::Complete
    );
    let inputs = f
        .store
        .tail_inputs(&f.peer, &f.client.locator)
        .await
        .unwrap();
    let deleted: bool =
        sqlx::query_scalar("SELECT deleted FROM task_attachments WHERE attachment_id=?")
            .bind(&reference)
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    assert!(deleted);
    assert!(
        !f.peer
            .encrypted_round_state(&inputs.authority)
            .await
            .unwrap()
            .downloads
            .unwrap()
            .pending
    );
}

async fn publish_source(f: &Fixture) {
    use aven_core::sync::encrypted_tail::{Accepted, Operation, Reply};
    let store = isolated_store(&f.source, &f.root.path().join("keys")).await;
    let inputs = store
        .tail_inputs(&f.source, &f.client.locator)
        .await
        .unwrap();
    for _ in 0..1024 {
        let Some(record) = f
            .source
            .prepare_encrypted_push(&inputs.authority, f.root.path())
            .await
            .unwrap()
            .map(|push| push.record)
        else {
            return;
        };
        let Reply::Appended(mapping) = f
            .server
            .encrypted_tail_exchange(
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
            panic!("accepted source edit")
        };
        f.source
            .observe_encrypted_tail(&inputs.authority, &mapping)
            .await
            .unwrap();
        f.source
            .verify_encrypted_tail_outcome(&inputs.authority, &Accepted { mapping, record })
            .await
            .unwrap();
    }
    panic!("source edits did not drain");
}

async fn edit_source_descriptions(f: &Fixture, start: usize, end: usize) {
    let workspace = f.source.list_workspaces().await.unwrap().remove(0);
    let task: String = sqlx::query_scalar("SELECT task_id FROM task_attachments LIMIT 1")
        .fetch_one(&mut *aven_core::test_support::acquire(&f.source).await.unwrap())
        .await
        .unwrap();
    for n in start..end {
        f.source
            .update_task(
                &workspace,
                &task.parse().unwrap(),
                aven_core::operations::TaskUpdate {
                    description: Some(format!("tail edit {n}")),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
    }
    publish_source(f).await;
}

#[tokio::test]
#[ignore = "subprocess initial tail persistence worker"]
async fn initial_tail_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_SNAPSHOT_ROOT").unwrap());
    let origin = std::env::var("AVEN_SNAPSHOT_ORIGIN").unwrap();
    let peer = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let store = isolated_store(&peer, &root.join("peer-keys")).await;
    crate::encrypted_tail_http::Client::new(&origin)
        .unwrap()
        .round(&store, &peer, &root.join("peer-blobs"))
        .await
        .unwrap();
    panic!("expected process exit");
}

#[tokio::test]
async fn initial_image_demand_replays_delete_before_download() {
    initial_image_demand_has_a_fixed_restart_safe_watermark(true).await;
}

#[tokio::test]
async fn initial_image_demand_does_not_chase_a_moving_head() {
    initial_image_demand_has_a_fixed_restart_safe_watermark(false).await;
}

async fn initial_image_demand_has_a_fixed_restart_safe_watermark(delete: bool) {
    use crate::encrypted_tail_http::ImageTransfer;
    let page = aven_core::sync::encrypted_tail::PAGE_COUNT;
    let f = enrolled().await;
    edit_source_descriptions(&f, 0, page + 1).await;
    if delete {
        let workspace = f.source.list_workspaces().await.unwrap().remove(0);
        let reference: String =
            sqlx::query_scalar("SELECT attachment_id FROM task_attachments LIMIT 1")
                .fetch_one(&mut *aven_core::test_support::acquire(&f.source).await.unwrap())
                .await
                .unwrap();
        f.source
            .delete_task_attachment(&workspace, &reference)
            .await
            .unwrap();
        publish_source(&f).await;
    }
    let blobs = f.root.path().join("peer-blobs");
    let report = f.client.install(&f.store, &f.peer).await.unwrap();
    let expected = report.prefix_count + page as u64 + 1 + u64::from(delete);
    for stage in ["before-page-commit", "after-page-commit"] {
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "peer_enrollment_http::tests::install::attachments::initial_tail_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("AVEN_SNAPSHOT_ROOT", f.root.path())
            .env("AVEN_SNAPSHOT_ORIGIN", &f.client.locator)
            .env("AVEN_TAIL_CRASH", stage)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(84),
            "{stage}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let peer = Database::open(f.peer.path()).await.unwrap();
        let mark = peer
            .meta("e2ee_initial_image_watermark")
            .await
            .unwrap()
            .unwrap();
        if stage == "before-page-commit" {
            assert_eq!(mark, "pending");
            assert_eq!(
                peer.meta("sync_cursor").await.unwrap().unwrap(),
                report.prefix_count.to_string()
            );
        } else {
            assert_eq!(mark, expected.to_string());
            assert_eq!(
                peer.meta("sync_cursor").await.unwrap().unwrap(),
                (report.prefix_count + page as u64).to_string()
            );
            let inputs = f.store.tail_inputs(&peer, &f.client.locator).await.unwrap();
            assert!(
                peer.prepare_encrypted_image_download(&inputs.authority)
                    .await
                    .is_err()
            );
        }
        assert!(!blobs.join("objects/sha256").exists());
        assert_eq!(f.client.install(&f.store, &peer).await.unwrap(), report);
        assert_eq!(
            peer.meta("e2ee_initial_image_watermark")
                .await
                .unwrap()
                .unwrap(),
            mark
        );
    }
    // The second call must reach the saved target even though a full new page exists.
    edit_source_descriptions(&f, page + 1, 2 * page + 5).await;
    let peer = Database::open(f.peer.path()).await.unwrap();
    let client = crate::encrypted_tail_http::Client::new(&f.client.locator).unwrap();
    let transfer = client.round(&f.store, &peer, &blobs).await.unwrap();
    assert!(transfer.metadata_caught_up);
    assert_eq!(transfer.images, ImageTransfer::Complete);
    assert_eq!(
        peer.meta("sync_cursor").await.unwrap().unwrap(),
        expected.to_string()
    );
    assert_eq!(
        peer.meta("e2ee_initial_image_watermark")
            .await
            .unwrap()
            .as_deref(),
        Some("ready")
    );
    let available: i64 =
        sqlx::query_scalar("SELECT count(*) FROM blob_inventory WHERE available=1")
            .fetch_one(&mut *aven_core::test_support::acquire(&peer).await.unwrap())
            .await
            .unwrap();
    assert_eq!(available, i64::from(!delete));
    if delete {
        assert!(!blobs.join("objects/sha256").exists());
    } else {
        for image in std::fs::read_dir(f.root.path().join("objects/sha256")).unwrap() {
            let image = image.unwrap();
            assert_eq!(
                std::fs::read(image.path()).unwrap(),
                std::fs::read(blobs.join("objects/sha256").join(image.file_name())).unwrap()
            );
        }
    }
    assert!(!client.pull_only_round(&f.store, &peer).await.unwrap());
}

#[tokio::test]
async fn live_bootstrap_mapping_survives_missing_bytes_and_later_exact_repair() {
    use crate::encrypted_tail_http::ImageTransfer;
    let f = enrolled().await;
    // Simulate unavailable storage, not authenticated absence or a new mapping.
    let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
    sqlx::query("DELETE FROM server_e2ee_image_chunks")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE server_e2ee_images SET complete=0")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let blobs = f.root.path().join("peer-blobs");
    let report = f.client.install(&f.store, &f.peer).await.unwrap();
    let client = crate::encrypted_tail_http::Client::new(&f.client.locator).unwrap();
    let before = shared(&f.peer).await;
    for _ in 0..2 {
        let transfer = client.round(&f.store, &f.peer, &blobs).await.unwrap();
        assert!(transfer.metadata_caught_up);
        assert_eq!(transfer.images, ImageTransfer::Unavailable);
        assert_eq!(shared(&f.peer).await, before);
    }
    let mapping: (String, String, Vec<u8>) =
        sqlx::query_as("SELECT workspace,reference,object FROM local_e2ee_image_references")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap();
    let seed_store = isolated_store(&f.source, &f.root.path().join("keys")).await;
    client
        .repair_attachment(
            &seed_store,
            &f.source,
            f.root.path(),
            &mapping.0,
            &mapping.1,
        )
        .await
        .unwrap();
    let peer = Database::open(f.peer.path()).await.unwrap();
    assert_eq!(f.client.install(&f.store, &peer).await.unwrap(), report);
    assert_eq!(
        client.round(&f.store, &peer, &blobs).await.unwrap().images,
        ImageTransfer::Complete
    );
    let after: (String, String, Vec<u8>) =
        sqlx::query_as("SELECT workspace,reference,object FROM local_e2ee_image_references")
            .fetch_one(&mut *aven_core::test_support::acquire(&peer).await.unwrap())
            .await
            .unwrap();
    assert_eq!(mapping, after);
    assert_eq!(shared(&peer).await, before);
    for image in std::fs::read_dir(f.root.path().join("objects/sha256")).unwrap() {
        let image = image.unwrap();
        assert_eq!(
            std::fs::read(image.path()).unwrap(),
            std::fs::read(blobs.join("objects/sha256").join(image.file_name())).unwrap()
        );
    }
}

#[tokio::test]
async fn missing_initial_catch_up_marker_refuses_without_reinstalling() {
    let f = enrolled().await;
    f.client.install(&f.store, &f.peer).await.unwrap();
    let before = shared(&f.peer).await;
    sqlx::query("DELETE FROM meta WHERE key='e2ee_initial_image_watermark'")
        .execute(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
        .await
        .unwrap();
    assert!(f.client.install(&f.store, &f.peer).await.is_err());
    assert!(
        f.store
            .tail_inputs(&f.peer, &f.client.locator)
            .await
            .is_err()
    );
    assert_eq!(shared(&f.peer).await, before);
    assert_eq!(
        f.peer.meta("e2ee_initial_image_watermark").await.unwrap(),
        None
    );
    assert_eq!(count(&f.peer, "local_peer_snapshot_install").await, 1);
}

#[tokio::test]
async fn pull_only_refreshes_old_head_without_uploading_or_extending_initial_watermark() {
    let page = aven_core::sync::encrypted_tail::PAGE_COUNT;
    let f = enrolled().await;
    edit_source_descriptions(&f, 0, page + 1).await;
    let receipt = f.client.install(&f.store, &f.peer).await.unwrap();
    let client = crate::encrypted_tail_http::Client::new(&f.client.locator).unwrap();
    let workspace = f.peer.list_workspaces().await.unwrap().remove(0);
    let task: String = sqlx::query_scalar("SELECT task_id FROM task_attachments LIMIT 1")
        .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
        .await
        .unwrap();
    f.peer
        .add_note(
            &workspace,
            &task.parse().unwrap(),
            "pending frozen note".into(),
        )
        .await
        .unwrap();
    let inputs = f
        .store
        .tail_inputs(&f.peer, &f.client.locator)
        .await
        .unwrap();
    let frozen = f
        .peer
        .prepare_encrypted_push(&inputs.authority, &f.root.path().join("peer-blobs"))
        .await
        .unwrap()
        .unwrap()
        .record;
    drop(inputs);
    f.peer
        .add_note(
            &workspace,
            &task.parse().unwrap(),
            "pending unfrozen note".into(),
        )
        .await
        .unwrap();
    let pending = async || -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.peer).await.unwrap())
            .await
            .unwrap()
    };
    let original_pending = pending().await;
    assert!(!client.pull_only_round(&f.store, &f.peer).await.unwrap());
    let watermark = f
        .peer
        .meta("e2ee_initial_image_watermark")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        watermark,
        (receipt.prefix_count + page as u64 + 1).to_string()
    );
    edit_source_descriptions(&f, page + 1, page + 4).await;

    let seed_keys = isolated_store(&f.source, &f.root.path().join("keys")).await;
    let third = Database::open(&f.root.path().join("third.sqlite"))
        .await
        .unwrap();
    let third_keys = isolated_store(&third, &f.root.path().join("third-keys")).await;
    let invitation = f
        .client
        .invite(&seed_keys, &f.source, expiry())
        .await
        .unwrap();
    f.client
        .request(&third_keys, &third, Some(invitation))
        .await
        .unwrap();
    f.client.admit(&seed_keys, &f.source).await.unwrap();
    f.client.complete(&third_keys, &third).await.unwrap();
    f.client.install(&third_keys, &third).await.unwrap();
    assert_eq!(
        f.peer
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        2
    );
    let high_water = async || -> i64 {
        sqlx::query_scalar("SELECT high_water FROM server_e2ee_allocator WHERE singleton=1")
            .fetch_one(&mut *aven_core::test_support::acquire(&f.server).await.unwrap())
            .await
            .unwrap()
    };
    let before = high_water().await;

    // Call the public pull-only entry directly, with no intervening refresh round.
    assert!(client.pull_only_round(&f.store, &f.peer).await.unwrap());
    assert_eq!(
        f.peer
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        3
    );
    assert_eq!(
        f.peer.meta("sync_cursor").await.unwrap().unwrap(),
        watermark
    );
    assert_eq!(
        f.peer
            .meta("e2ee_initial_image_watermark")
            .await
            .unwrap()
            .as_deref(),
        Some("ready")
    );
    assert_eq!(pending().await, original_pending);
    assert_eq!(high_water().await, before);
    let inputs = f
        .store
        .tail_inputs(&f.peer, &f.client.locator)
        .await
        .unwrap();
    assert_eq!(
        f.peer
            .encrypted_tail_frozen_record(&inputs.authority)
            .await
            .unwrap()
            .unwrap()
            .1,
        frozen
    );
}
