use super::*;
use crate::sync::{LocalSharedStatePackageContext, LocalSharedStatePackageKey};

/// Appends an empty zstd skippable frame to the stored snapshot: the decoded
/// JSON is unchanged but the stored bytes are not.
const INERT_SNAPSHOT_REWRITE: &str =
    "UPDATE local_shared_capture_documents SET snapshot = snapshot || x'502a4d1800000000'";
/// The same inert rewrite of the stored source history and provenance.
const INERT_HISTORY_REWRITE: &str = "UPDATE local_shared_capture_documents
     SET source_history = source_history || x'502a4d1800000000'";
const INERT_PROVENANCE_REWRITE: &str = "UPDATE local_shared_capture_documents
     SET source_provenance = source_provenance || x'502a4d1800000000'";

type Fixture = (
    tempfile::TempDir,
    Database,
    SeedSourceAuthority,
    SeedAuthority,
    LocalSharedStatePackageKey,
    String,
);

async fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let database = Database::open(&root.path().join("seed.sqlite"))
        .await
        .unwrap();
    freeze(root, database).await
}

async fn add_task(database: &Database, title: &str) {
    let workspace = database.list_workspaces().await.unwrap().remove(0);
    database
        .create_task(
            &workspace,
            crate::operations::TaskDraft {
                title: title.into(),
                description: String::new(),
                project: Some("app".into()),
                status: Some("todo".into()),
                priority: "none".into(),
                source: crate::choices::TaskSource::Cli,
                labels: vec![],
                metadata: vec![],
                available_at: None,
                due_on: None,
                is_epic: false,
            },
        )
        .await
        .unwrap();
}

/// A frozen capture with accepted, provenance-carrying and pending history.
async fn history_fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let database = Database::open(&root.path().join("seed.sqlite"))
        .await
        .unwrap();
    add_task(&database, "one").await;
    add_task(&database, "two").await;
    {
        let mut conn = database.acquire_writer().await.unwrap();
        sqlx::query("UPDATE changes SET server_seq = local_seq * 3 WHERE change_id IN (SELECT change_id FROM changes ORDER BY local_seq LIMIT 3)").execute(&mut *conn).await.unwrap();
        sqlx::query("INSERT INTO shared_history_provenance(change_id, source_server_seq, source_pending_rank) SELECT change_id, 999, NULL FROM changes ORDER BY local_seq LIMIT 1").execute(&mut *conn).await.unwrap();
    }
    freeze(root, database).await
}

async fn freeze(root: tempfile::TempDir, database: Database) -> Fixture {
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

async fn sql(database: &Database, statement: &'static str) {
    let mut conn = database.acquire_writer().await.unwrap();
    sqlx::query(statement).execute(&mut *conn).await.unwrap();
}

async fn intent_error(fixture: &Fixture) -> Option<String> {
    let (_, database, source, seed, key, _) = fixture;
    database
        .prepare_seed_publication_intent(source, seed, key)
        .await
        .err()
        .map(|error| format!("{error:#}"))
}

#[tokio::test]
async fn history_validation_accepts_later_pending_edits() {
    let fixture = history_fixture().await;
    add_task(&fixture.1, "after capture").await;
    assert_eq!(intent_error(&fixture).await, None);
}

#[tokio::test]
async fn history_validation_compares_payloads_semantically() {
    let fixture = history_fixture().await;
    let mut conn = fixture.1.acquire_writer().await.unwrap();
    let rows: Vec<(String, String)> =
        sqlx::query_as("SELECT change_id, payload FROM changes WHERE payload LIKE '{%'")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    let mut reordered = 0;
    for (change_id, payload) in rows {
        let serde_json::Value::Object(map) = serde_json::from_str(&payload).unwrap() else {
            continue;
        };
        if map.len() < 2 {
            continue;
        }
        let entries = map
            .iter()
            .rev()
            .map(|(key, value)| format!("{} : {}", serde_json::to_string(key).unwrap(), value))
            .collect::<Vec<_>>();
        let rewritten = format!("{{ {} }}", entries.join(" ,\n "));
        assert_ne!(rewritten, payload);
        sqlx::query("UPDATE changes SET payload = ? WHERE change_id = ?")
            .bind(rewritten)
            .bind(change_id)
            .execute(&mut *conn)
            .await
            .unwrap();
        reordered += 1;
    }
    drop(conn);
    assert!(reordered > 0);
    assert_eq!(intent_error(&fixture).await, None);
}

#[tokio::test]
async fn history_validation_refuses_semantic_payload_change() {
    let fixture = history_fixture().await;
    sql(
        &fixture.1,
        "UPDATE changes SET payload = json_set(payload, '$.injected', 1)
         WHERE change_id = (SELECT change_id FROM changes WHERE payload LIKE '{%' LIMIT 1)",
    )
    .await;
    let error = intent_error(&fixture).await.unwrap();
    assert!(error.contains("seed-captured-history-changed"), "{error}");
}

#[tokio::test]
async fn history_validation_refuses_changed_original_sequence() {
    let fixture = history_fixture().await;
    sql(
        &fixture.1,
        "UPDATE changes SET server_seq = server_seq + 100000
         WHERE change_id = (SELECT change_id FROM changes WHERE server_seq IS NOT NULL LIMIT 1)",
    )
    .await;
    let error = intent_error(&fixture).await.unwrap();
    assert!(error.contains("seed-captured-history-changed"), "{error}");
}

#[tokio::test]
async fn history_validation_refuses_missing_captured_row() {
    let fixture = history_fixture().await;
    sql(
        &fixture.1,
        "DELETE FROM changes WHERE change_id = (SELECT max(change_id) FROM changes)",
    )
    .await;
    let error = intent_error(&fixture).await.unwrap();
    assert!(error.contains("seed-captured-history-changed"), "{error}");
}

#[tokio::test]
async fn history_validation_refuses_uncaptured_accepted_history() {
    let fixture = history_fixture().await;
    add_task(&fixture.1, "after capture").await;
    sql(
        &fixture.1,
        "UPDATE changes SET server_seq = 500000 WHERE change_id NOT IN
         (SELECT change_id FROM local_shared_capture_changes) AND change_id =
         (SELECT max(change_id) FROM changes WHERE change_id NOT IN
          (SELECT change_id FROM local_shared_capture_changes))",
    )
    .await;
    let error = intent_error(&fixture).await.unwrap();
    assert!(
        error.contains("seed-uncaptured-accepted-history"),
        "{error}"
    );
}

#[tokio::test]
async fn history_validation_refuses_changed_provenance() {
    for statement in [
        "UPDATE shared_history_provenance SET source_server_seq = 998",
        "DELETE FROM shared_history_provenance",
        "INSERT INTO shared_history_provenance(change_id, source_server_seq, source_pending_rank)
         SELECT change_id, NULL, 7 FROM changes WHERE change_id NOT IN
         (SELECT change_id FROM shared_history_provenance) LIMIT 1",
    ] {
        let fixture = history_fixture().await;
        sql(&fixture.1, statement).await;
        let error = intent_error(&fixture).await.unwrap();
        assert!(error.contains("seed-source-provenance-changed"), "{error}");
    }
}

#[tokio::test]
async fn history_validation_refuses_changed_capture_map() {
    let fixture = history_fixture().await;
    sql(
        &fixture.1,
        "DELETE FROM local_shared_capture_changes WHERE change_id =
         (SELECT max(change_id) FROM local_shared_capture_changes)",
    )
    .await;
    assert!(intent_error(&fixture).await.is_some());
}

async fn ranks(conn: &mut SqliteConnection) -> Vec<(String, Option<i64>)> {
    sqlx::query_as("SELECT change_id, server_seq FROM changes ORDER BY change_id")
        .fetch_all(conn)
        .await
        .unwrap()
}

#[tokio::test]
async fn captured_history_adoption_assigns_prefix_ranks_only_to_captured_rows() {
    let fixture = history_fixture().await;
    add_task(&fixture.1, "after capture").await;
    let mut conn = fixture.1.acquire_writer().await.unwrap();
    let mut tx = db::begin_immediate(&mut conn).await.unwrap();
    let before = ranks(&mut tx).await;
    let expected: Vec<(String, i64)> = sqlx::query_as(
        "SELECT change_id, prefix_rank FROM local_shared_capture_changes ORDER BY change_id",
    )
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    // The fixture's accepted rows sit at 3, 6, 9 and must move to dense ranks
    // that collide with their old values under the unique index.
    assert!(before.iter().any(|(_, seq)| *seq == Some(3)));
    adopt_captured_history(&mut tx, &fixture.5, expected.len() as u64)
        .await
        .unwrap();
    let after = ranks(&mut tx).await;
    let captured = expected
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    for ((id, old), (_, new)) in before.iter().zip(&after) {
        match captured.get(id) {
            Some(rank) => assert_eq!(*new, Some(*rank)),
            None => assert_eq!(new, old),
        }
    }
    let provenance: i64 = sqlx::query_scalar("SELECT count(*) FROM shared_history_provenance")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(provenance as usize, captured.len());
}

#[tokio::test]
async fn captured_history_adoption_refuses_one_provenance_conflict_atomically() {
    let fixture = history_fixture().await;
    let mut conn = fixture.1.acquire_writer().await.unwrap();
    sqlx::query("UPDATE shared_history_provenance SET source_server_seq = 998")
        .execute(&mut *conn)
        .await
        .unwrap();
    let before = ranks(&mut conn).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_changes")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    {
        let mut tx = db::begin_immediate(&mut conn).await.unwrap();
        let error = adopt_captured_history(&mut tx, &fixture.5, count as u64)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("seed-provenance-changed"));
    }
    assert_eq!(ranks(&mut conn).await, before);
    let provenance: i64 = sqlx::query_scalar("SELECT count(*) FROM shared_history_provenance")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(provenance, 1);
}

#[tokio::test]
async fn captured_history_adoption_refuses_prefix_count_mismatch() {
    let fixture = history_fixture().await;
    let mut conn = fixture.1.acquire_writer().await.unwrap();
    let mut tx = db::begin_immediate(&mut conn).await.unwrap();
    let error = adopt_captured_history(&mut tx, &fixture.5, 1)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("seed-prefix-count-mismatch"));
}

async fn fixture_proof(fixture: &Fixture) -> ProofCache {
    let (_, database, _, seed, key, _) = fixture;
    ProofCache::new(Some(
        super::super::validated::ValidatedSeed::load(database, key, seed.genesis().commitment())
            .await
            .unwrap(),
    ))
}

async fn intent_error_with(fixture: &Fixture, proofs: &mut ProofCache) -> String {
    let (_, database, source, seed, key, _) = fixture;
    format!(
        "{:#}",
        database
            .prepare_seed_publication_intent_with(source, seed, key, proofs)
            .await
            .map(drop)
            .unwrap_err()
    )
}

#[tokio::test]
async fn proof_detects_snapshot_rewrite_before_writer_use() {
    let fixture = history_fixture().await;
    let mut proofs = fixture_proof(&fixture).await;
    // Semantically inert for the package, but not the bytes that were proved.
    sql(&fixture.1, INERT_SNAPSHOT_REWRITE).await;
    let error = intent_error_with(&fixture, &mut proofs).await;
    assert!(error.contains("seed-capture-changed"), "{error}");
}

#[tokio::test]
async fn proof_detects_history_rewrite_before_writer_use() {
    for rewrite in [INERT_HISTORY_REWRITE, INERT_PROVENANCE_REWRITE] {
        let fixture = history_fixture().await;
        let mut proofs = fixture_proof(&fixture).await;
        sql(&fixture.1, rewrite).await;
        let error = intent_error_with(&fixture, &mut proofs).await;
        assert!(error.contains("seed-capture-changed"), "{rewrite}: {error}");
    }
}

#[tokio::test]
async fn proof_detects_rank_table_mutation_before_writer_use() {
    let fixture = history_fixture().await;
    let mut proofs = fixture_proof(&fixture).await;
    sql(
        &fixture.1,
        "UPDATE local_shared_capture_changes SET source_server_seq = source_server_seq + 1000
         WHERE source_server_seq IS NOT NULL",
    )
    .await;
    let error = intent_error_with(&fixture, &mut proofs).await;
    assert!(error.contains("seed-capture-changed"), "{error}");
}

#[tokio::test]
async fn proof_requires_the_package_key() {
    let fixture = history_fixture().await;
    let wrong = LocalSharedStatePackageKey::new([1; 32]);
    assert!(
        super::super::validated::ValidatedSeed::load(
            &fixture.1,
            &wrong,
            fixture.3.genesis().commitment()
        )
        .await
        .is_err()
    );
}

#[tokio::test]
async fn proof_refuses_tampered_frozen_record_without_a_key() {
    let fixture = history_fixture().await;
    sql(
        &fixture.1,
        "UPDATE local_shared_capture_package_records SET record = zeroblob(length(record))
         WHERE component = 'state'",
    )
    .await;
    crate::sync::shared_state::counters::take();
    let error = super::super::validated::ValidatedSeed::load(
        &fixture.1,
        &fixture.4,
        fixture.3.genesis().commitment(),
    )
    .await
    .err()
    .map(|error| format!("{error:#}"))
    .unwrap();
    assert!(error.contains("frozen-records-invalid"), "{error}");
    assert_eq!(crate::sync::shared_state::counters::take().0, 0);
}

#[tokio::test]
async fn in_process_proof_prepares_intent_without_another_keyed_pass() {
    let fixture = history_fixture().await;
    let mut proofs = fixture_proof(&fixture).await;
    crate::sync::shared_state::counters::take();
    let (_, database, source, seed, key, _) = &fixture;
    database
        .prepare_seed_publication_intent_with(source, seed, key, &mut proofs)
        .await
        .unwrap();
    database
        .prepare_seed_publication_intent_with(source, seed, key, &mut proofs)
        .await
        .unwrap();
    assert_eq!(crate::sync::shared_state::counters::take().0, 0);
}

/// A frozen capture with history and two selected images.
async fn image_fixture() -> Fixture {
    let (root, database, task) = super::super::package::test_support::source_with_history().await;
    super::super::package::test_support::add_selected_images(root.path(), &database, &task).await;
    freeze(root, database).await
}

/// Prepares the intent with one keyed pass and returns it.
async fn bound_intent(fixture: &Fixture) -> SeedPublicationIntent {
    let (_, database, source, seed, key, _) = fixture;
    database
        .prepare_seed_publication_intent(source, seed, key)
        .await
        .unwrap()
}

/// A cache that trusts `intent` as if read back from protected storage.
fn trusting(intent: &SeedPublicationIntent) -> ProofCache {
    let mut proofs = ProofCache::default();
    proofs.trust_protected(intent);
    proofs
}

#[tokio::test]
async fn bound_intent_resumes_with_hash_checks_only() {
    let fixture = image_fixture().await;
    let intent = bound_intent(&fixture).await;
    let binding = intent.freeze_binding().unwrap();
    assert_eq!(binding.validation_version, VALIDATION_VERSION);
    let (_, database, source, seed, key, _) = &fixture;
    crate::sync::shared_state::counters::take();
    let resumed = database
        .prepare_seed_publication_intent_with(source, seed, key, &mut trusting(&intent))
        .await
        .unwrap();
    assert_eq!(
        resumed.protected_storage_bytes(),
        intent.protected_storage_bytes()
    );
    assert_eq!(crate::sync::shared_state::counters::take().0, 0);
    // The rebuilt proof carries the same attachment mappings as a keyed pass.
    let keyed =
        super::super::validated::ValidatedSeed::load(database, key, seed.genesis().commitment())
            .await
            .unwrap();
    let hashed = super::super::validated::ValidatedSeed::load_from_intent(database, binding)
        .await
        .unwrap();
    let pairs = |proof: &super::super::validated::ValidatedSeed| {
        proof
            .attachments()
            .objects
            .iter()
            .map(|(image, sha256)| (image.id, sha256.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(pairs(&keyed).len(), 2);
    assert_eq!(pairs(&keyed), pairs(&hashed));
    assert_eq!(keyed.binding(), hashed.binding());
}

#[tokio::test]
async fn bound_intent_refuses_each_tampered_input() {
    let tampers: [&[&'static str]; 7] = [
        &[INERT_SNAPSHOT_REWRITE],
        &[INERT_HISTORY_REWRITE],
        &[INERT_PROVENANCE_REWRITE],
        &[
            "UPDATE local_shared_capture_images SET classification = 'extra_selected'
           WHERE classification = 'current_selected'",
        ],
        &["UPDATE local_shared_capture_changes SET prefix_rank = prefix_rank + 100000"],
        &[
            "UPDATE local_shared_capture_images SET object_id = zeroblob(32)
           WHERE object_id IS NOT NULL AND sha256 = (SELECT min(sha256)
               FROM local_shared_capture_images WHERE object_id IS NOT NULL)",
        ],
        // Swaps the object IDs of the two selected images.
        &[
            "CREATE TEMP TABLE swapped AS SELECT sha256, object_id FROM local_shared_capture_images
             WHERE object_id IS NOT NULL",
            "UPDATE local_shared_capture_images SET object_id = (
                 SELECT object_id FROM swapped
                 WHERE swapped.sha256 != local_shared_capture_images.sha256)
             WHERE object_id IS NOT NULL",
        ],
    ];
    for statements in tampers {
        let fixture = image_fixture().await;
        let intent = bound_intent(&fixture).await;
        {
            let mut conn = fixture.1.acquire_writer().await.unwrap();
            for statement in statements {
                sqlx::query(*statement).execute(&mut *conn).await.unwrap();
            }
        }
        crate::sync::shared_state::counters::take();
        let error = intent_error_with(&fixture, &mut trusting(&intent)).await;
        // Capture loading and the source history check refuse some tampering
        // before the binding is checked.
        assert!(
            [
                "seed-capture-changed",
                "local-shared-capture-image-set-mismatch",
                "local-shared-capture-history-mismatch",
                "seed-intent-source-changed",
            ]
            .iter()
            .any(|code| error.contains(code)),
            "{statements:?}: {error}"
        );
        assert_eq!(crate::sync::shared_state::counters::take().0, 0);
    }
}

#[tokio::test]
async fn bound_intent_upload_refuses_a_tampered_record() {
    let fixture = image_fixture().await;
    let intent = bound_intent(&fixture).await;
    let (_, database, source, seed, key, _) = &fixture;
    database
        .seal_seed_publication_intent(source, &intent)
        .await
        .unwrap();
    sql(
        database,
        "UPDATE local_shared_capture_package_records SET record = zeroblob(length(record))
         WHERE component = 'image'",
    )
    .await;
    let upload = database
        .seed_publication_upload(source, &intent, seed, key, &mut trusting(&intent))
        .await
        .unwrap()
        .unwrap();
    let error = upload.read(database, upload.slots()).await.unwrap_err();
    assert!(
        format!("{error:#}").contains("seed-package-mismatch"),
        "{error:#}"
    );
}

#[tokio::test]
async fn published_descriptor_adopts_without_reading_corrupt_local_ciphertext() {
    let fixture = image_fixture().await;
    let intent = bound_intent(&fixture).await;
    let (_, database, source, seed, key, _) = &fixture;
    database
        .seal_seed_publication_intent(source, &intent)
        .await
        .unwrap();
    let publication = intent.publication(seed.genesis()).unwrap();
    let outcome = PublicationOutcome::from_response(
        seed.genesis(),
        intent.descriptor(),
        publication.record(),
    )
    .unwrap();
    sql(
        database,
        "UPDATE local_shared_capture_package_records SET record = zeroblob(length(record))
         WHERE component = 'image'",
    )
    .await;
    assert!(
        database
            .adopt_seed_publication(source, &intent, seed, key, &outcome, &mut trusting(&intent),)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn unsupported_receipt_version_fails_without_touching_frozen_bytes() {
    let fixture = image_fixture().await;
    let intent = bound_intent(&fixture).await;
    let (_, database, source, seed, _, _) = &fixture;
    let text = String::from_utf8(intent.protected_storage_bytes().to_vec()).unwrap();
    let future = text.replace("\"validation_version\":1", "\"validation_version\":2");
    assert_ne!(future, text);
    {
        let mut conn = database.acquire_writer().await.unwrap();
        sqlx::query("UPDATE local_seed_publication_intent SET intent = ?")
            .bind(future.as_bytes())
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let future =
        SeedPublicationIntent::from_protected_storage(future.as_bytes(), source, seed.genesis())
            .unwrap();
    let frozen = |database: &Database| {
        let database = database.clone();
        async move {
            let mut conn = database.acquire_reader().await.unwrap();
            type Row = (Vec<u8>, Option<Vec<u8>>, Option<Vec<u8>>);
            let rows: Vec<Row> = sqlx::query_as(
                "SELECT d.snapshot, j.frozen_descriptor_commitment, j.frozen_capture_commitment
                 FROM local_shared_capture_journal j
                 JOIN local_shared_capture_documents d USING (candidate_id)",
            )
            .fetch_all(&mut *conn)
            .await
            .unwrap();
            let records: Vec<Vec<u8>> =
                sqlx::query_scalar("SELECT record FROM local_shared_capture_package_records")
                    .fetch_all(&mut *conn)
                    .await
                    .unwrap();
            (rows, records)
        }
    };
    let before = frozen(database).await;
    let error = intent_error_with(&fixture, &mut trusting(&future)).await;
    assert!(error.contains("seed-intent-receipt-unsupported"), "{error}");
    // A keyed pass cannot promote it either.
    let error = intent_error_with(&fixture, &mut ProofCache::default()).await;
    assert!(error.contains("seed-intent-receipt-unsupported"), "{error}");
    assert_eq!(frozen(database).await, before);
}

#[tokio::test]
async fn older_freeze_without_commitments_fails_closed() {
    let fixture = image_fixture().await;
    sql(
        &fixture.1,
        "UPDATE local_shared_capture_journal SET frozen_capture_commitment = NULL",
    )
    .await;
    let error = intent_error(&fixture).await.unwrap();
    assert!(error.contains("seed-freeze-unsupported"), "{error}");
    let (root, database, _, seed, key, _) = &fixture;
    let error = database
        .package_local_shared_state_never_dispatched(
            root.path(),
            seed.genesis().context(),
            key,
            seed.genesis().commitment(),
        )
        .await
        .err()
        .map(|error| format!("{error:#}"))
        .unwrap();
    assert!(
        error
            .contains("seed-freeze-unsupported hint=cancel-never-dispatched-capture-and-recapture"),
        "{error}"
    );
}

/// Real histories carry thousands of provenance rows; validation must stay
/// linear in them. Comparing against a per-row `json_each` join took over
/// twenty seconds for 7,676 rows and stalled setup before its upload.
#[tokio::test]
async fn history_validation_stays_linear_in_provenance() {
    let root = tempfile::tempdir().unwrap();
    let database = Database::open(&root.path().join("seed.sqlite"))
        .await
        .unwrap();
    add_task(&database, "many").await;
    {
        let mut conn = database.acquire_writer().await.unwrap();
        sqlx::query(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 8000)
             INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id,
                                 field, op_type, payload, base_version, created_at)
             SELECT printf('bulk-%05d', i), c.client_id, 1000 + i, c.entity_type, c.entity_id,
                    c.field, c.op_type, c.payload, c.base_version, c.created_at
             FROM n, (SELECT * FROM changes ORDER BY local_seq DESC LIMIT 1) c",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO shared_history_provenance(change_id, source_server_seq, source_pending_rank)
             SELECT change_id, NULL, local_seq FROM changes WHERE change_id LIKE 'bulk-%'",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
    }
    let (_root, database, source, _, _, candidate) = freeze(root, database).await;
    // Counts SQLite VM instructions, a deterministic measure of the work
    // validation asks of SQLite, independent of build profile and machine.
    let steps = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
    let mut conn = database.acquire_writer().await.unwrap();
    let counter = steps.clone();
    conn.lock_handle()
        .await
        .unwrap()
        .set_progress_handler(1, move || {
            counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            true
        });
    validate_history(&mut conn, &candidate, &source)
        .await
        .unwrap();
    let steps = steps.load(std::sync::atomic::Ordering::Relaxed);
    assert!(steps < 8000 * 200, "{steps} VM steps for 8000 changes");
}

#[tokio::test]
async fn history_stored_as_legacy_value_text_still_validates() {
    let root = tempfile::tempdir().unwrap();
    let database = Database::open(&root.path().join("seed.sqlite"))
        .await
        .unwrap();
    add_task(&database, "legacy").await;
    let (_root, database, source, _, _, candidate) = freeze(root, database).await;
    let mut conn = database.acquire_writer().await.unwrap();
    let stored: Vec<u8> =
        sqlx::query_scalar("SELECT source_history FROM local_shared_capture_documents")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    // Older captures stored sorted-key JSON values with parsed payloads.
    let rows: Vec<serde_json::Value> =
        serde_json::from_str(&crate::sync::shared_state::unpack_document(&stored).unwrap())
            .unwrap();
    assert!(!rows.is_empty());
    let legacy = serde_json::to_string(&rows).unwrap();
    assert!(legacy.starts_with(r#"[{"base_version""#), "{legacy}");
    sqlx::query("UPDATE local_shared_capture_documents SET source_history = ?")
        .bind(legacy.as_bytes())
        .execute(&mut *conn)
        .await
        .unwrap();
    validate_history(&mut conn, &candidate, &source)
        .await
        .unwrap();
}

#[tokio::test]
async fn captured_history_adoption_leaves_rows_already_at_their_rank() {
    let fixture = history_fixture().await;
    let mut conn = fixture.1.acquire_writer().await.unwrap();
    let mut tx = db::begin_immediate(&mut conn).await.unwrap();
    // Moves one row onto its rank, so rows both at and off their rank remain.
    let (id, rank): (String, i64) = sqlx::query_as(
        "SELECT c.change_id, c.prefix_rank FROM local_shared_capture_changes c
         WHERE NOT EXISTS(SELECT 1 FROM changes WHERE server_seq = c.prefix_rank)
         ORDER BY c.prefix_rank LIMIT 1",
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query("UPDATE changes SET server_seq = ? WHERE change_id = ?")
        .bind(rank)
        .bind(&id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE shared_history_provenance SET source_server_seq = ? WHERE change_id = ?")
        .bind(rank)
        .bind(&id)
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE local_shared_capture_changes SET source_server_seq = ? WHERE change_id = ?",
    )
    .bind(rank)
    .bind(&id)
    .execute(&mut *tx)
    .await
    .unwrap();
    for statement in [
        "CREATE TEMP TABLE rewritten(change_id TEXT)",
        "CREATE TEMP TRIGGER record_rewrite AFTER UPDATE ON changes
         BEGIN INSERT INTO rewritten VALUES (NEW.change_id); END",
    ] {
        sqlx::query(statement).execute(&mut *tx).await.unwrap();
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM local_shared_capture_changes")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    adopt_captured_history(&mut tx, &fixture.5, count as u64)
        .await
        .unwrap();
    let rewritten: Vec<String> = sqlx::query_scalar("SELECT DISTINCT change_id FROM rewritten")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    assert!(!rewritten.is_empty());
    assert!(!rewritten.contains(&id), "{id} was already at rank {rank}");
    assert!(ranks(&mut tx).await.contains(&(id, Some(rank))));
}
