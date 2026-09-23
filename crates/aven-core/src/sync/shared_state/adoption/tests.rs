use super::*;
use crate::sync::{LocalSharedStatePackageContext, LocalSharedStatePackageKey};

async fn fixture() -> (
    tempfile::TempDir,
    Database,
    SeedSourceAuthority,
    SeedAuthority,
    LocalSharedStatePackageKey,
    String,
) {
    let root = tempfile::tempdir().unwrap();
    let database = Database::open(&root.path().join("seed.sqlite"))
        .await
        .unwrap();
    let key = LocalSharedStatePackageKey::new([7; 32]);
    let context = LocalSharedStatePackageContext {
        vault_id: [8; 32],
        generation_id: [9; 32],
    };
    let seed = SeedAuthority::generate(context, &key, [10; 32]).unwrap();
    database
        .pin_local_seed_genesis(seed.genesis())
        .await
        .unwrap();
    let source = SeedSourceAuthority::generate([11; 32], seed.genesis()).unwrap();
    {
        let installation = db::installation::InstallationGuard::acquire(database.path()).unwrap();
        database
            .bind_seed_source(&source, &installation)
            .await
            .unwrap();
    }
    let capture = database
        .capture_local_shared_state_never_dispatched(root.path())
        .await
        .unwrap();
    database
        .package_local_shared_state_never_dispatched(
            root.path(),
            context,
            &key,
            seed.genesis().commitment(),
        )
        .await
        .unwrap();
    (
        root,
        database,
        source,
        seed,
        key,
        capture.candidate_id().to_string(),
    )
}

#[tokio::test]
async fn committed_intent_blocks_actual_cancellation_across_pools() {
    let (_root, database, source, seed, key, candidate) = fixture().await;
    let other = Database::open(database.path()).await.unwrap();
    let (entered, waiting) = tokio::sync::oneshot::channel();
    let (release, resume) = tokio::sync::oneshot::channel();
    *INTENT_BARRIER.lock().unwrap() = Some((candidate.clone(), entered, resume));
    let preparing = tokio::spawn(async move {
        database
            .prepare_seed_publication_intent(&source, &seed, &key)
            .await
    });
    waiting.await.unwrap();
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let cancel = tokio::spawn(async move {
        started.send(()).unwrap();
        other
            .cancel_local_shared_state_never_dispatched(&candidate)
            .await
    });
    started_rx.await.unwrap();
    release.send(()).unwrap();
    preparing.await.unwrap().unwrap();
    let error = cancel.await.unwrap().unwrap_err();
    assert!(error.to_string().contains("intent-owned"));
}

#[tokio::test]
async fn cancellation_commit_prevents_intent_creation() {
    let (_root, database, source, seed, key, candidate) = fixture().await;
    let other = Database::open(database.path()).await.unwrap();
    assert!(
        other
            .cancel_local_shared_state_never_dispatched(&candidate)
            .await
            .unwrap()
    );
    assert!(
        database
            .prepare_seed_publication_intent(&source, &seed, &key)
            .await
            .is_err()
    );
    assert!(
        database
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn source_fence_rejects_real_metadata_and_blob_entry_points() {
    let (root, database, _source, _seed, _key, _candidate) = fixture().await;
    assert!(
        database
            .prepare_sync_discovery("http://localhost")
            .await
            .is_err()
    );
    assert!(
        database
            .prepare_client_sync_page("http://localhost".into(), 1, 1)
            .await
            .is_err()
    );
    assert!(database.missing_local_blob_page(1).await.is_err());
    let contract = crate::sync::wire::BlobUploadContract {
        workspace_id: crate::workspaces::Workspace::default().id.to_string(),
        sha256: "11".repeat(32),
        byte_size: 1,
        media_type: "image/png".into(),
        width: 1,
        height: 1,
    };
    assert!(
        database
            .prepare_blob_upload(root.path(), &contract)
            .await
            .unwrap_err()
            .to_string()
            .contains("e2ee-installation-fenced")
    );
    let mut image = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::new(1, 1))
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    let bytes = image.into_inner();
    let blob = crate::sync::blob::MissingLocalBlob {
        sha256: crate::attachments::storage::sha256_hex(&bytes),
        byte_size: bytes.len() as i64,
        media_type: "image/png".into(),
        width: Some(1),
        height: Some(1),
    };
    assert!(
        database
            .store_downloaded_blob(root.path(), Default::default(), &blob, bytes)
            .await
            .unwrap_err()
            .to_string()
            .contains("e2ee-installation-fenced")
    );
    assert!(
        !crate::attachments::storage::object_path(root.path(), &blob.sha256)
            .unwrap()
            .exists()
    );
}

#[tokio::test]
async fn intent_sql_failure_preserves_capture_and_allows_local_cancel() {
    let (_root, database, source, seed, key, candidate) = fixture().await;
    let mut conn = database.acquire_writer().await.unwrap();
    sqlx::query("CREATE TRIGGER fail_intent BEFORE INSERT ON local_seed_publication_intent BEGIN SELECT RAISE(ABORT, 'injected'); END").execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        database
            .prepare_seed_publication_intent(&source, &seed, &key)
            .await
            .is_err()
    );
    assert!(
        database
            .seed_publication_intent_bytes()
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        database
            .cancel_local_shared_state_never_dispatched(&candidate)
            .await
            .unwrap()
    );
}
