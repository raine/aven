use super::*;
use aven_core::sync::seed_claim::membership::{Joiner, Membership, VerifiedKeys};

struct Installed {
    db: Database,
    store: ProtectedLocalKeyStore,
    peer: Joiner,
}
async fn join(
    root: &Path,
    client: &Client,
    name: &str,
    inviter: &Database,
    inviter_store: &ProtectedLocalKeyStore,
    install: bool,
) -> Installed {
    let db = Database::open(&root.join(format!("{name}.sqlite")))
        .await
        .unwrap();
    let store = isolated_store(db.path(), &root.join(format!("{name}-keys")));
    let invitation = client
        .invite(inviter_store, inviter, expiry())
        .await
        .unwrap();
    client.request(&store, &db, Some(invitation)).await.unwrap();
    let peer = store
        .prepare_peer(&db, &client.locator, None)
        .await
        .unwrap();
    assert!(client.admit(inviter_store, inviter).await.unwrap());
    assert!(client.complete(&store, &db).await.unwrap());
    if install {
        client.install(&store, &db).await.unwrap();
    }
    Installed { db, store, peer }
}
async fn checkpoint(
    store: &ProtectedLocalKeyStore,
    db: &Database,
    client: &Client,
) -> (Membership, VerifiedKeys) {
    let inputs = store.active_inputs(db, &client.locator).await.unwrap();
    (
        inputs.membership.clone(),
        VerifiedKeys::from_protected_storage(
            &inputs.membership,
            &inputs.generation_keys().protected_storage_bytes(),
        )
        .unwrap(),
    )
}
fn auth<'a>(m: &Membership, peer: &'a Joiner) -> peer::Authentication<'a> {
    peer::Authentication {
        vault: m.genesis().context().vault_id,
        genesis: m.genesis().commitment(),
        device: peer.device(),
        credential_version: 1,
        head: m.head(),
        bearer: peer.bearer(),
    }
}
async fn rotate(
    server: &Database,
    peer: &Joiner,
    m: &mut Membership,
    keys: &mut VerifiedKeys,
    targets: &[[u8; 32]],
) {
    let revoke = peer.authority().prepare_revoke(m, targets).unwrap();
    server
        .apply_membership_management(&auth(m, peer), &revoke)
        .await
        .unwrap();
    *m = m.append(&[], &[], &revoke).unwrap();
    let prep = server
        .prepare_membership_management(&auth(m, peer))
        .await
        .unwrap();
    let record = peer
        .authority()
        .prepare_rotation(m, keys, prep.high_water)
        .unwrap();
    server
        .apply_membership_management(&auth(m, peer), &record)
        .await
        .unwrap();
    *keys = peer.authority().receive_rotation(m, &record, keys).unwrap();
    *m = m.append(&[], &[], &record).unwrap();
}
fn owned_file(root: &Path, name: &str, suffix: &str) -> std::path::PathBuf {
    std::fs::read_dir(root.join(format!("{name}-keys")))
        .unwrap()
        .map(|e| e.unwrap().path())
        .find(|p| p.file_name().unwrap().to_str().unwrap().ends_with(suffix))
        .unwrap()
}
async fn execute(db: &Database, sql: &str) {
    sqlx::query(sqlx::AssertSqlSafe(sql.to_owned()))
        .execute(&mut *aven_core::test_support::acquire(db).await.unwrap())
        .await
        .unwrap();
}
fn assert_keys(m: &Membership, actual: &VerifiedKeys, expected: &VerifiedKeys) {
    actual.validate(m).unwrap();
    for g in m.generations() {
        assert_eq!(
            actual.key(g.id).unwrap().protected_storage_bytes(),
            expected.key(g.id).unwrap().protected_storage_bytes()
        );
    }
}

#[tokio::test]
async fn three_installations_offline_two_rotations_and_fresh_historical_bootstrap_install() {
    let root = tempfile::tempdir().unwrap();
    let (seed_db, seed_store, server, origin, task) = adopted(root.path()).await;
    let seed = seed_store
        .prepare_seed_claim(&seed_db, [9; 32])
        .await
        .unwrap();
    let client = Client::new(&origin).unwrap();
    let second = join(root.path(), &client, "second", &seed_db, &seed_store, true).await;
    let third = join(root.path(), &client, "third", &seed_db, &seed_store, true).await;
    client.refresh(&second.store, &second.db).await.unwrap();
    let (mut m, mut keys) = checkpoint(&second.store, &second.db, &client).await;
    let original_third_verified =
        std::fs::read(owned_file(root.path(), "third", "peer-verified")).unwrap();
    let original_third_installed =
        std::fs::read(owned_file(root.path(), "third", "peer-installed")).unwrap();
    let original_third_response =
        std::fs::read(owned_file(root.path(), "third", "peer-response")).unwrap();
    let workspace = third.db.list_workspaces().await.unwrap().remove(0);
    let task_id = third
        .db
        .export_data("test".into())
        .await
        .unwrap()
        .tables
        .tasks[0]
        .id
        .clone();
    third
        .db
        .add_note(
            &workspace,
            &task_id,
            "offline work before two rotations".into(),
        )
        .await
        .unwrap();
    let tail = third.store.tail_inputs(&third.db, &origin).await.unwrap();
    let frozen = third
        .db
        .prepare_encrypted_push(&tail.authority, &root.path().join("third-blobs"))
        .await
        .unwrap()
        .unwrap()
        .record;
    drop(tail);
    let cursor = third.db.meta("sync_cursor").await.unwrap();
    let identity = third.db.meta("client_id").await.unwrap();
    assert_ne!(identity, second.db.meta("client_id").await.unwrap());
    assert_ne!(identity, seed_db.meta("client_id").await.unwrap());
    assert_ne!(third.peer.bearer().expose(), second.peer.bearer().expose());
    rotate(
        &server,
        &second.peer,
        &mut m,
        &mut keys,
        &[seed.genesis().device_id()],
    )
    .await;
    rotate(&server, &second.peer, &mut m, &mut keys, &[]).await;
    assert_eq!(m.generations().len(), 3);
    assert_eq!(
        third
            .db
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        3
    );
    client.refresh(&third.store, &third.db).await.unwrap();
    let (third_m, third_keys) = checkpoint(&third.store, &third.db, &client).await;
    assert_eq!(third_m.head(), m.head());
    assert_keys(&m, &third_keys, &keys);
    assert_eq!(third.db.meta("sync_cursor").await.unwrap(), cursor);
    let retained: Vec<u8> =
        sqlx::query_scalar("SELECT record FROM local_e2ee_outbox WHERE singleton=1")
            .fetch_one(&mut *aven_core::test_support::acquire(&third.db).await.unwrap())
            .await
            .unwrap();
    assert_eq!(retained, frozen);

    assert_eq!(third.db.meta("client_id").await.unwrap(), identity);
    for (suffix, bytes) in [
        ("peer-verified", original_third_verified),
        ("peer-installed", original_third_installed),
        ("peer-response", original_third_response),
    ] {
        assert_eq!(
            std::fs::read(owned_file(root.path(), "third", suffix)).unwrap(),
            bytes
        );
    }
    let tail = third.store.tail_inputs(&third.db, &origin).await.unwrap();
    assert_eq!(tail.authority.generation(), m.current_generation().id);
    drop(tail);
    let seed_floor = seed_db.membership_checkpoint_mirror().await.unwrap();
    assert!(client.refresh(&seed_store, &seed_db).await.is_err());
    assert_eq!(
        seed_db.membership_checkpoint_mirror().await.unwrap(),
        seed_floor
    );
    // Authenticated public ancestry, not an arbitrary refusal, can establish removal.
    let evidence = server
        .membership_evidence(&auth(&m, &second.peer))
        .await
        .unwrap();
    let mut removed_inputs = seed_store.active_inputs(&seed_db, &origin).await.unwrap();
    assert!(
        seed_store
            .adopt_refresh(&seed_db, &mut removed_inputs, evidence)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("revoked")
    );
    drop(removed_inputs);
    assert!(seed_store.enrollment_readiness(&seed_db).await.is_err());
    assert!(seed_store.active_inputs(&seed_db, &origin).await.is_err());
    client.refresh(&second.store, &second.db).await.unwrap();
    let fresh = join(
        root.path(),
        &client,
        "fresh",
        &second.db,
        &second.store,
        false,
    )
    .await;
    m = server
        .membership_evidence(&auth(&m, &second.peer))
        .await
        .unwrap()
        .verify()
        .unwrap();
    let original = std::fs::read(owned_file(root.path(), "fresh", "peer-verified")).unwrap();
    rotate(&server, &second.peer, &mut m, &mut keys, &[]).await;
    let report = client.install(&fresh.store, &fresh.db).await.unwrap();
    assert!(report.prefix_count > 0);
    let (fresh_m, fresh_keys) = checkpoint(&fresh.store, &fresh.db, &client).await;
    assert_eq!(fresh_m.head(), m.head());
    assert_keys(&m, &fresh_keys, &keys);
    assert_eq!(
        std::fs::read(owned_file(root.path(), "fresh", "peer-verified")).unwrap(),
        original
    );
    let installed = std::fs::read(owned_file(root.path(), "fresh", "peer-installed")).unwrap();
    execute(
        &fresh.db,
        "UPDATE tasks SET title='local edit survives immutable retry'",
    )
    .await;
    let snapshot = fresh.db.export_data("test".into()).await.unwrap();
    let reopened = Database::open(fresh.db.path()).await.unwrap();
    let reopened_store = isolated_store(reopened.path(), &root.path().join("fresh-keys"));
    client.install(&reopened_store, &reopened).await.unwrap();
    assert_eq!(
        serde_json::to_value(reopened.export_data("test".into()).await.unwrap().tables).unwrap(),
        serde_json::to_value(snapshot.tables).unwrap()
    );
    assert_eq!(
        std::fs::read(owned_file(root.path(), "fresh", "peer-installed")).unwrap(),
        installed
    );
    assert_keys(
        &m,
        &checkpoint(&reopened_store, &reopened, &client).await.1,
        &keys,
    );
    // Neither SQLite nor public evidence contains raw generation secrets.
    let sql = serde_json::to_string(&reopened.export_data("test".into()).await.unwrap()).unwrap();
    let mut sqlite = std::fs::read(reopened.path()).unwrap();
    if let Ok(wal) = std::fs::read(format!("{}-wal", reopened.path().display())) {
        sqlite.extend(wal);
    }
    let public = serde_json::to_vec(
        &server
            .membership_evidence(&auth(&m, &second.peer))
            .await
            .unwrap(),
    )
    .unwrap();
    for g in m.generations() {
        let raw = keys.key(g.id).unwrap().protected_storage_bytes();
        assert!(!sql.contains(&hex::encode(raw)));
        assert!(!sqlite.windows(raw.len()).any(|w| w == raw));
        assert!(!public.windows(raw.len()).any(|w| w == raw));
    }
    task.abort();
}

#[tokio::test]
async fn protected_coverage_precedes_mirror_and_missing_or_corrupt_established_keys_refuse() {
    let root = tempfile::tempdir().unwrap();
    let (seed_db, seed_store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let second = join(root.path(), &client, "second", &seed_db, &seed_store, true).await;
    let third = join(root.path(), &client, "third", &seed_db, &seed_store, true).await;
    client.refresh(&second.store, &second.db).await.unwrap();
    let (mut m, mut keys) = checkpoint(&second.store, &second.db, &client).await;
    let cursor = third.db.meta("sync_cursor").await.unwrap();
    let old_mirror = third.db.membership_checkpoint_mirror().await.unwrap();
    execute(
        &third.db,
        "UPDATE tasks SET title='survives protected checkpoint failure'",
    )
    .await;
    rotate(&server, &second.peer, &mut m, &mut keys, &[]).await;
    rotate(&server, &second.peer, &mut m, &mut keys, &[]).await;
    let evidence = server
        .membership_evidence(&auth(&m, &second.peer))
        .await
        .unwrap();
    let mut invalid = evidence.clone();
    invalid.transitions.pop();
    invalid.transitions.last_mut().unwrap().record[0] ^= 1;
    let mut inputs = third.store.active_inputs(&third.db, &origin).await.unwrap();
    assert!(
        third
            .store
            .adopt_refresh(&third.db, &mut inputs, invalid)
            .await
            .is_err()
    );
    drop(inputs);
    assert_eq!(
        third.db.membership_checkpoint_mirror().await.unwrap(),
        old_mirror
    );
    execute(&third.db, "CREATE TRIGGER mirror_fault BEFORE UPDATE ON local_membership_checkpoint WHEN NEW.sequence=7 BEGIN SELECT RAISE(ABORT,'coverage mirror fault'); END").await;
    assert!(client.refresh(&third.store, &third.db).await.is_err());
    assert_eq!(
        third.db.membership_checkpoint_mirror().await.unwrap(),
        old_mirror
    );
    let path = owned_file(root.path(), "third", "membership-coverage-7");
    let protected = std::fs::read(&path).unwrap();
    assert_eq!(
        protected.len(),
        44 + aven_core::sync::seed_claim::membership::MAX_COVERAGE_BYTES
    );
    assert!(owned_file(root.path(), "third", "membership-floor-7").exists());
    execute(&third.db, "DROP TRIGGER mirror_fault").await;
    let reopened = Database::open(third.db.path()).await.unwrap();
    let store = isolated_store(reopened.path(), &root.path().join("third-keys"));
    // Local reopen repairs only the public mirror after validating protected coverage.
    let (recovered, actual) = checkpoint(&store, &reopened, &client).await;
    assert_eq!(recovered.head(), m.head());
    assert_keys(&m, &actual, &keys);
    assert_eq!(
        reopened
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        7
    );
    assert_eq!(reopened.meta("sync_cursor").await.unwrap(), cursor);
    let state =
        serde_json::to_value(reopened.export_data("test".into()).await.unwrap().tables).unwrap();
    for corrupt in [false, true] {
        if corrupt {
            let mut bytes = protected.clone();
            bytes[100] ^= 1;
            std::fs::write(&path, bytes).unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        assert!(store.active_inputs(&reopened, &origin).await.is_err());
        assert!(store.enrollment_readiness(&reopened).await.is_err());
        assert!(client.refresh(&store, &reopened).await.is_err());
        assert!(client.install(&store, &reopened).await.is_err());
        assert_eq!(
            serde_json::to_value(reopened.export_data("test".into()).await.unwrap().tables)
                .unwrap(),
            state
        );
        std::fs::write(&path, &protected).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert_keys(&m, &checkpoint(&store, &reopened, &client).await.1, &keys);
    task.abort();
}

#[tokio::test]
#[ignore = "subprocess protected rotation coverage worker"]
async fn coverage_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_ROTATION_ROOT").unwrap());
    let origin = std::env::var("AVEN_ROTATION_ORIGIN").unwrap();
    let db = Database::open(&root.join("third.sqlite")).await.unwrap();
    let store = isolated_store(db.path(), &root.join("third-keys"));
    Client::new(&origin)
        .unwrap()
        .refresh(&store, &db)
        .await
        .unwrap();
    panic!("coverage crash not reached");
}

#[tokio::test]
async fn crash_after_coverage_and_floor_before_sqlite_mirror_recovers_without_reinstall() {
    let root = tempfile::tempdir().unwrap();
    let (seed_db, seed_store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let second = join(root.path(), &client, "second", &seed_db, &seed_store, true).await;
    let third = join(root.path(), &client, "third", &seed_db, &seed_store, true).await;
    client.refresh(&second.store, &second.db).await.unwrap();
    let (mut m, mut keys) = checkpoint(&second.store, &second.db, &client).await;
    rotate(&server, &second.peer, &mut m, &mut keys, &[]).await;
    rotate(&server, &second.peer, &mut m, &mut keys, &[]).await;
    let receipt = std::fs::read(owned_file(root.path(), "third", "peer-installed")).unwrap();
    let cursor = third.db.meta("sync_cursor").await.unwrap();
    for boundary in [
        "membership-coverage-7",
        "membership-evidence",
        "membership-floor-7",
    ] {
        let output = e2ee_http::worker("peer_enrollment_http::tests::rotation::coverage_worker")
            .env("AVEN_ROTATION_ROOT", root.path())
            .env("AVEN_ROTATION_ORIGIN", &origin)
            .env("AVEN_PEER_CRASH_KIND", boundary)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(79),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            third
                .db
                .membership_checkpoint_mirror()
                .await
                .unwrap()
                .unwrap()
                .1,
            3
        );
        assert_eq!(third.db.meta("sync_cursor").await.unwrap(), cursor);
    }
    let reopened = Database::open(third.db.path()).await.unwrap();
    let store = isolated_store(reopened.path(), &root.path().join("third-keys"));
    assert_keys(&m, &checkpoint(&store, &reopened, &client).await.1, &keys);
    assert_eq!(
        reopened
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        7
    );
    assert_eq!(
        std::fs::read(owned_file(root.path(), "third", "peer-installed")).unwrap(),
        receipt
    );
    task.abort();
}

#[tokio::test]
async fn signed_head_33_refresh_and_reopen_preserve_complete_coverage_and_receipt() {
    let root = tempfile::tempdir().unwrap();
    let (seed_db, seed_store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let second = join(root.path(), &client, "second", &seed_db, &seed_store, true).await;
    let third = join(root.path(), &client, "third", &seed_db, &seed_store, true).await;
    client.refresh(&second.store, &second.db).await.unwrap();
    let (mut m, mut keys) = checkpoint(&second.store, &second.db, &client).await;
    assert_eq!(m.sequence(), 3);
    let receipt = std::fs::read(owned_file(root.path(), "third", "peer-installed")).unwrap();
    let cursor = third.db.meta("sync_cursor").await.unwrap();
    let domain =
        serde_json::to_value(third.db.export_data("test".into()).await.unwrap().tables).unwrap();
    for _ in 0..15 {
        rotate(&server, &second.peer, &mut m, &mut keys, &[]).await;
    }
    assert_eq!(m.sequence(), 33);
    assert_eq!(m.generations().len(), 16);
    let result = client.refresh(&third.store, &third.db).await;
    assert!(owned_file(root.path(), "third", "membership-coverage-33").exists());
    assert!(owned_file(root.path(), "third", "membership-floor-33").exists());
    result.unwrap();
    assert_eq!(
        third
            .db
            .membership_checkpoint_mirror()
            .await
            .unwrap()
            .unwrap()
            .1,
        33
    );
    let reopened = Database::open(third.db.path()).await.unwrap();
    let store = isolated_store(reopened.path(), &root.path().join("third-keys"));
    let (recovered, actual) = checkpoint(&store, &reopened, &client).await;
    assert_eq!(recovered.head(), m.head());
    assert_keys(&m, &actual, &keys);
    client.refresh(&store, &reopened).await.unwrap();
    assert_eq!(reopened.meta("sync_cursor").await.unwrap(), cursor);
    assert_eq!(
        std::fs::read(owned_file(root.path(), "third", "peer-installed")).unwrap(),
        receipt
    );
    assert_eq!(
        serde_json::to_value(reopened.export_data("test".into()).await.unwrap().tables).unwrap(),
        domain
    );
    task.abort();
}
