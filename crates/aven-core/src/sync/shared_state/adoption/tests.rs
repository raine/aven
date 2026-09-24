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
        .capture_local_shared_state_never_dispatched()
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
async fn history_validation_does_not_read_exported_domain_tables() {
    let (_root, database, source, seed, key, _candidate) = fixture().await;
    let mut conn = database.acquire_writer().await.unwrap();
    sqlx::query("ALTER TABLE projects RENAME TO projects_not_read_by_history_validation")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    database
        .prepare_seed_publication_intent(&source, &seed, &key)
        .await
        .unwrap();
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

#[tokio::test]
async fn lost_intent_row_does_not_restore_local_cancellation_authority() {
    let (_root, database, source, seed, key, candidate) = fixture().await;
    database
        .prepare_seed_publication_intent(&source, &seed, &key)
        .await
        .unwrap();
    let mut conn = database.acquire_writer().await.unwrap();
    sqlx::query("DELETE FROM local_seed_publication_intent")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(
        database
            .cancel_local_shared_state_never_dispatched(&candidate)
            .await
            .is_err()
    );
    assert!(
        database
            .prepare_seed_publication_intent(&source, &seed, &key)
            .await
            .is_err()
    );
    assert!(
        database
            .resume_local_shared_state_never_dispatched()
            .await
            .is_err()
    );
    let mut conn = database.acquire_writer().await.unwrap();
    assert!(
        sqlx::query("DELETE FROM local_shared_capture_journal")
            .execute(&mut *conn)
            .await
            .is_err()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM local_shared_capture_journal")
            .fetch_one(&mut *conn)
            .await
            .unwrap(),
        1
    );
}
