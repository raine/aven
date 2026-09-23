use super::*;

#[tokio::test]
async fn protected_seed_publishes_frozen_images_through_core_and_recovers_lost_reply() {
    use aven_core::sync::bootstrap_staging::{
        Authentication, Budget, Component, PublishBootstrap, PutChunk, Status,
    };
    use aven_core::sync::seed_claim::{ClaimAuthentication, Secret, SetupAuthority};

    let root = tempfile::tempdir().unwrap();
    let client = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let workspace = client.list_workspaces().await.unwrap().remove(0);
    let task = client
        .create_task(
            &workspace,
            aven_core::operations::TaskDraft {
                title: "PRIVATE-PUBLICATION-TITLE".into(),
                description: String::new(),
                project: Some("app".into()),
                status: "todo".into(),
                priority: "none".into(),
                source: aven_core::choices::TaskSource::Cli,
                labels: vec![],
                metadata: vec![],
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap()
        .task;
    for width in [2, 3, 4] {
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::new(width, 1))
            .write_to(&mut bytes, image::ImageFormat::Png)
            .unwrap();
        let attachment = client
            .add_task_attachment(
                &workspace,
                root.path(),
                Default::default(),
                &task.id,
                aven_core::operations::AttachmentAddInput {
                    filename: Some("PRIVATE-PUBLICATION-IMAGE.png".into()),
                    alt_text: None,
                    declared_media_type: None,
                    bytes: bytes.into_inner(),
                    optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                    dedupe_existing: false,
                },
            )
            .await
            .unwrap()
            .outcome
            .attachment;
        if width != 2 {
            client
                .delete_task_attachment(&workspace, &attachment.attachment_id)
                .await
                .unwrap();
        }
        if width == 4 {
            let mut conn = aven_core::test_support::acquire(&client).await.unwrap();
            sqlx::query("UPDATE blob_inventory SET available = 0 WHERE sha256 = ?")
                .bind(&attachment.sha256)
                .execute(&mut *conn)
                .await
                .unwrap();
            fs::remove_file(
                aven_core::attachments::object_path(root.path(), &attachment.sha256).unwrap(),
            )
            .unwrap();
        }
    }
    let store = isolated_store(client.path(), &root.path().join("keys"));
    let original = store.prepare_seed_claim(&client, [9; 32]).await.unwrap();
    let protected = original.protected_storage_bytes();
    drop(original);
    client
        .capture_local_shared_state_never_dispatched(root.path())
        .await
        .unwrap();
    let local = store
        .package_seed_capture(&client, root.path(), [9; 32])
        .await
        .unwrap();
    let package = local.upload_package();
    assert_eq!(package.images.len(), 2);
    drop(client);
    drop(store);
    let client = Database::open(&root.path().join("client.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(client.path(), &root.path().join("keys"));
    let seed = store.prepare_seed_claim(&client, [9; 32]).await.unwrap();
    assert_eq!(protected, seed.protected_storage_bytes());
    let reopened = store
        .package_seed_capture(&client, root.path(), [9; 32])
        .await
        .unwrap();
    assert_eq!(local, reopened);
    let authority = store.load_required().unwrap();
    let signed = seed
        .prepare_bootstrap_publication(&reopened.upload_package(), authority.package_key())
        .unwrap();
    let server_path = root.path().join("server.sqlite");
    let server = Database::open(&server_path).await.unwrap();
    let setup_secret = Secret::generate().unwrap();
    let setup =
        SetupAuthority::from_verifier([9; 32], SetupAuthority::verifier([9; 32], &setup_secret));
    server
        .admit_seed_claim(
            &seed.genesis().claim_bytes(),
            Some(&setup),
            ClaimAuthentication::SetupSecret(&setup_secret),
        )
        .await
        .unwrap();
    let auth = Authentication {
        vault_id: seed.genesis().context().vault_id,
        genesis_commitment: seed.genesis().commitment(),
        bearer: seed.bearer(),
    };
    let binding = signed.binding();
    let mut components = Vec::new();
    for (index, component) in [
        Component::DataCatalog,
        Component::PrefixCatalog,
        Component::ImageCatalog,
    ]
    .into_iter()
    .enumerate()
    {
        components.push((
            component,
            package.catalogs[index]
                .chunks(1_048_576)
                .collect::<Vec<_>>(),
        ));
    }
    components.push((
        Component::Manifest,
        package.manifest.iter().map(Vec::as_slice).collect(),
    ));
    components.push((
        Component::State,
        package.state.iter().map(Vec::as_slice).collect(),
    ));
    for image in &package.images {
        components.push((
            Component::Image(image.object_id),
            image.records.iter().map(Vec::as_slice).collect(),
        ));
    }
    let budget = Budget {
        bytes: components
            .iter()
            .flat_map(|(_, chunks)| chunks)
            .map(|b| b.len() as u64)
            .sum(),
        chunks: components
            .iter()
            .map(|(_, chunks)| chunks.len() as u64)
            .sum(),
    };
    let staging = server
        .declare_bootstrap_staging(&auth, &package.descriptor, budget)
        .await
        .unwrap();
    for (component, chunks) in components {
        for (index, bytes) in chunks.into_iter().enumerate() {
            server
                .put_bootstrap_chunk(
                    &auth,
                    PutChunk {
                        bootstrap_id: binding.bootstrap_id,
                        descriptor_commitment: binding.descriptor_commitment,
                        epoch: staging.epoch,
                        component,
                        index: index as u64,
                        bytes,
                    },
                )
                .await
                .unwrap();
        }
    }
    let request = || PublishBootstrap {
        bootstrap_id: binding.bootstrap_id,
        descriptor_commitment: binding.descriptor_commitment,
        epoch: staging.epoch,
        record: signed.record(),
    };
    let accepted = server
        .publish_bootstrap(&auth, request(), Default::default())
        .await
        .unwrap();
    drop(server);
    // The observed response is deliberately unused for recovery; exact frozen
    // intent and protected seed are sufficient to ask the reopened core.
    let server = Database::open(&server_path).await.unwrap();
    assert_eq!(
        server
            .bootstrap_staging_status(&auth, binding.bootstrap_id)
            .await
            .unwrap(),
        Status::Published(accepted.clone())
    );
    assert_eq!(
        server
            .publish_bootstrap(&auth, request(), Default::default())
            .await
            .unwrap(),
        accepted
    );
    accepted
        .validate_expected(seed.genesis(), &package.descriptor)
        .unwrap();
    let mut conn = aven_core::test_support::acquire(&server).await.unwrap();
    let unmapped: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM server_e2ee_image_references WHERE object IS NULL",
    )
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(unmapped, 1);
    let (owned, grace): (i64, i64) =
        sqlx::query_as("SELECT count(*), count(unreferenced_at) FROM server_e2ee_images")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!((owned, grace), (2, 1));
    drop(conn);
    assert_eq!(seed.protected_storage_bytes(), protected);
    // This direct test invocation is not a production dispatcher. The local
    // never-dispatched journal is unchanged, and cannot authorize real dispatch.
    assert!(
        client
            .has_local_shared_state_package_never_dispatched()
            .await
            .unwrap()
    );
}
