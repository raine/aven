use super::*;

#[tokio::test]
async fn publish_restarts_exactly_and_transfers_ownership_without_snapshot_image_pins() {
    let f = Fixture::new().await;
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
    let outcome = f.publish(&p, s.epoch).await.unwrap();
    outcome
        .validate_expected(f.seed.genesis(), &f.package.descriptor)
        .unwrap();
    let mut wrong = f.package.descriptor.clone();
    wrong[39] ^= 1;
    assert!(outcome.validate_expected(f.seed.genesis(), &wrong).is_err());
    let mut conn = f.server.acquire_writer().await.unwrap();
    let (n, high): (i64, i64) =
        sqlx::query_as("SELECT prefix_count, high_water FROM server_e2ee_allocator")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(n as u64, p.binding().prefix_count);
    assert_eq!(high, n);
    let ranks: Vec<i64> =
        sqlx::query_scalar("SELECT rank FROM server_bootstrap_prefix ORDER BY rank")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(ranks, (1..=n).collect::<Vec<_>>());
    let grace: Vec<Option<i64>> = sqlx::query_scalar(
        "SELECT unreferenced_at FROM server_e2ee_images ORDER BY unreferenced_at",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap();
    assert_eq!(grace.len(), 2);
    assert!(grace[0].is_none());
    let published_at: i64 =
        sqlx::query_scalar("SELECT published_at FROM server_bootstrap_publication")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(grace[1], Some(published_at));
    let staged_images: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM server_bootstrap_chunks WHERE length(component) = 33",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(staged_images, 0);
    for image in &f.package.images {
        let records: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT bytes FROM server_e2ee_image_chunks WHERE object = ? ORDER BY chunk_index",
        )
        .bind(image.object_id.as_slice())
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(records, image.records);
    }
    for (component, expected) in f
        .components()
        .into_iter()
        .filter(|(c, _)| !matches!(c, Component::Image(_)))
    {
        let stored: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT bytes FROM server_bootstrap_chunks WHERE component = ? ORDER BY chunk_index",
        )
        .bind(component.key())
        .fetch_all(&mut *conn)
        .await
        .unwrap();
        assert_eq!(
            stored.iter().map(Vec::as_slice).collect::<Vec<_>>(),
            expected
        );
    }
    // Model later allocation and legitimate image lifecycle progress. Retries
    // must not reset either, even if an old image is no longer available.
    sqlx::query("UPDATE server_e2ee_allocator SET high_water = high_water + 7")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DELETE FROM server_e2ee_image_chunks")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE server_e2ee_images SET unreferenced_at = 123")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let server = Database::open(&f.dir.path().join("server.sqlite"))
        .await
        .unwrap();
    let retry = server
        .publish_bootstrap(
            &f.auth(),
            f.publish_request(&p, 0),
            PublicationPolicy {
                workspace_quota_bytes: 0,
            },
        )
        .await
        .unwrap();
    assert_eq!(retry, outcome);
    assert_eq!(
        server
            .bootstrap_staging_status(&f.auth(), f.id)
            .await
            .unwrap(),
        Status::Published(outcome.clone())
    );
    assert_eq!(
        server
            .cancel_bootstrap_staging(&f.auth(), f.id)
            .await
            .unwrap(),
        Status::Published(outcome)
    );
    let mut conn = server.acquire_reader().await.unwrap();
    let high: i64 = sqlx::query_scalar("SELECT high_water FROM server_e2ee_allocator")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(high, n + 7);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_chunks")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
    let grace: Vec<i64> = sqlx::query_scalar("SELECT unreferenced_at FROM server_e2ee_images")
        .fetch_all(&mut *conn)
        .await
        .unwrap();
    assert_eq!(grace, [123, 123]);
}

#[tokio::test]
async fn deleted_parent_with_incomplete_history_keeps_selected_extra_protected() {
    let mut f = Fixture::new().await;
    f.source
        .cancel_local_shared_state_never_dispatched(&hex::encode(f.id))
        .await
        .unwrap();
    // Snapshot baselines need not be derivable from retained history. This
    // deleted baseline has no matching deletion event, so it is conservative.
    let mut conn = f.source.acquire_writer().await.unwrap();
    sqlx::query("UPDATE tasks SET deleted = 1")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    f.source
        .capture_local_shared_state_never_dispatched(f.dir.path())
        .await
        .unwrap();
    let local = f
        .source
        .package_local_shared_state_never_dispatched(
            f.dir.path(),
            f.seed.genesis().context(),
            &f.key,
            f.seed.genesis().commitment(),
        )
        .await
        .unwrap();
    f.id = hex::decode(local.candidate_id())
        .unwrap()
        .try_into()
        .unwrap();
    f.package = local.upload_package();
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
    f.publish(&p, s.epoch).await.unwrap();
    let mut conn = f.server.acquire_reader().await.unwrap();
    let (deleted, protected): (bool, bool) =
        sqlx::query_as("SELECT deleted, protected FROM server_e2ee_image_parents")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(deleted && protected);
    let protected_count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM server_e2ee_images WHERE unreferenced_at IS NULL")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!(protected_count, 1);
}

#[tokio::test]
async fn shared_references_charge_distinct_object_once_per_workspace() {
    let mut f = Fixture::new().await;
    f.source
        .cancel_local_shared_state_never_dispatched(&hex::encode(f.id))
        .await
        .unwrap();
    let workspace = f.source.list_workspaces().await.unwrap().remove(0);
    let mut conn = f.source.acquire_reader().await.unwrap();
    let id: String = sqlx::query_scalar("SELECT id FROM tasks LIMIT 1")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(2, 1))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    f.source
        .add_task_attachment(
            &workspace,
            f.dir.path(),
            Default::default(),
            &id.parse().unwrap(),
            crate::operations::AttachmentAddInput {
                filename: None,
                alt_text: None,
                declared_media_type: None,
                bytes: bytes.into_inner(),
                optimization_policy: crate::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        )
        .await
        .unwrap();
    f.source
        .capture_local_shared_state_never_dispatched(f.dir.path())
        .await
        .unwrap();
    let local = f
        .source
        .package_local_shared_state_never_dispatched(
            f.dir.path(),
            f.seed.genesis().context(),
            &f.key,
            f.seed.genesis().commitment(),
        )
        .await
        .unwrap();
    f.id = hex::decode(local.candidate_id())
        .unwrap()
        .try_into()
        .unwrap();
    f.package = local.upload_package();
    let d = crate::sync::bootstrap_format::staging::DeclarationView::decode(&f.package.descriptor)
        .unwrap();
    let images = d.image_rows(&f.package.catalogs[2]).unwrap();
    let current = images.objects.iter().find(|i| i.selection == 1).unwrap();
    let bytes: u64 = current.artifact.chunks.iter().map(|c| c.length).sum();
    assert_eq!(
        images
            .references
            .iter()
            .filter(|r| !r.deleted && r.object == Some(current.id))
            .count(),
        2
    );
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
    assert!(
        f.server
            .publish_bootstrap(
                &f.auth(),
                f.publish_request(&p, s.epoch),
                PublicationPolicy {
                    workspace_quota_bytes: bytes - 1
                }
            )
            .await
            .is_err()
    );
    f.unpublished().await;
    f.server
        .publish_bootstrap(
            &f.auth(),
            f.publish_request(&p, s.epoch),
            PublicationPolicy {
                workspace_quota_bytes: bytes,
            },
        )
        .await
        .unwrap();
}
