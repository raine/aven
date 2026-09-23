use super::super::tests::{captured_database, isolated_store};
use super::*;

fn seed_path(store: &ProtectedLocalKeyStore) -> PathBuf {
    let Backend::File(file) = store.seed_backend() else {
        panic!("isolated file backend required")
    };
    file.path
}

fn package_path(store: &ProtectedLocalKeyStore) -> PathBuf {
    let Backend::File(file) = &store.backend else {
        panic!("isolated file backend required")
    };
    file.path.clone()
}

#[tokio::test]
async fn seed_reopens_reuses_authority_and_repairs_absent_database_pin() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db.sqlite");
    let db = Database::open(&path).await.unwrap();
    let store = isolated_store(&path, &root.path().join("keys"));
    let package = store.load_or_create().unwrap();
    let original_package = fs::read(package_path(&store)).unwrap();
    let seed = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    assert_eq!(seed.genesis().context(), package.context());
    assert_eq!(original_package, fs::read(package_path(&store)).unwrap());
    let saved = seed.protected_storage_bytes();
    drop(seed);
    drop(db);
    let db = Database::open(&path).await.unwrap();
    let retry = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    assert_eq!(saved, retry.protected_storage_bytes());
    assert_eq!(
        Some(retry.genesis().commitment()),
        db.local_seed_genesis_commitment().await.unwrap()
    );
    assert!(store.prepare_seed_claim(&db, [8; 32]).await.is_err());
    assert_eq!(
        saved,
        store
            .prepare_seed_claim(&db, [9; 32])
            .await
            .unwrap()
            .protected_storage_bytes()
    );
    drop(db);
    let db = Database::open(&path).await.unwrap();
    {
        let mut conn = aven_core::test_support::acquire(&db).await.unwrap();
        sqlx::query("DELETE FROM local_seed_genesis_pin")
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    // A replaced task database has no cached pin. External authority still wins.
    assert_eq!(
        saved,
        store
            .prepare_seed_claim(&db, [9; 32])
            .await
            .unwrap()
            .protected_storage_bytes()
    );
}

#[tokio::test]
async fn missing_seed_or_package_authority_never_regenerates() {
    for remove_marker in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let db = Database::open(&root.path().join("db.sqlite"))
            .await
            .unwrap();
        let store = isolated_store(db.path(), &root.path().join("keys"));
        store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
        fs::remove_file(seed_path(&store)).unwrap();
        if remove_marker {
            fs::remove_file(store.seed_marker_path()).unwrap();
        }
        assert!(store.prepare_seed_claim(&db, [9; 32]).await.is_err());
        assert!(!seed_path(&store).exists());
        fs::remove_file(package_path(&store)).unwrap();
        fs::remove_file(store.marker_path()).unwrap();
        assert!(store.prepare_seed_claim(&db, [9; 32]).await.is_err());
        assert!(
            store
                .package_local_capture(&db, root.path(), [0; 32])
                .await
                .is_err()
        );
        assert!(!package_path(&store).exists());
    }
}

#[tokio::test]
async fn orphan_seed_repairs_marker_and_db_pin_without_replacement() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("db.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(db.path(), &root.path().join("keys"));
    let package = store.load_or_create().unwrap();
    let seed = SeedAuthority::generate(package.context(), package.package_key(), [9; 32]).unwrap();
    store
        .seed_backend()
        .create(&store.encode_seed(&seed).unwrap())
        .unwrap();
    assert!(!store.seed_marker_path().exists());
    assert!(db.local_seed_genesis_commitment().await.unwrap().is_none());
    let retry = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    assert_eq!(
        seed.protected_storage_bytes(),
        retry.protected_storage_bytes()
    );
    assert!(store.seed_marker_path().exists());
    fs::remove_file(store.seed_marker_path()).unwrap();
    assert_eq!(
        seed.protected_storage_bytes(),
        store
            .prepare_seed_claim(&db, [9; 32])
            .await
            .unwrap()
            .protected_storage_bytes()
    );
}

#[tokio::test]
async fn marker_and_database_write_failures_resume_saved_seed() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("db.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(db.path(), &root.path().join("keys"));
    let package = store.load_or_create().unwrap();
    let seed = SeedAuthority::generate(package.context(), package.package_key(), [9; 32]).unwrap();
    store
        .seed_backend()
        .create(&store.encode_seed(&seed).unwrap())
        .unwrap();
    fs::create_dir(store.seed_marker_path()).unwrap();
    assert!(store.prepare_seed_claim(&db, [9; 32]).await.is_err());
    fs::remove_dir(store.seed_marker_path()).unwrap();
    {
        let mut conn = aven_core::test_support::acquire(&db).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_pin AFTER INSERT ON local_seed_genesis_pin BEGIN SELECT RAISE(ABORT, 'injected pin failure'); END")
            .execute(&mut *conn).await.unwrap();
    }
    assert!(store.prepare_seed_claim(&db, [9; 32]).await.is_err());
    assert!(db.local_seed_genesis_commitment().await.unwrap().is_none());
    assert!(store.seed_marker_path().exists());
    {
        let mut conn = aven_core::test_support::acquire(&db).await.unwrap();
        sqlx::query("DROP TRIGGER fail_pin")
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    assert_eq!(
        seed.protected_storage_bytes(),
        store
            .prepare_seed_claim(&db, [9; 32])
            .await
            .unwrap()
            .protected_storage_bytes()
    );
}

#[tokio::test]
async fn corrupt_unavailable_and_wrong_installation_refuse_without_replacement() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("db.sqlite"))
        .await
        .unwrap();
    let other = Database::open(&root.path().join("other.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(db.path(), &root.path().join("keys"));
    assert!(store.prepare_seed_claim(&other, [9; 32]).await.is_err());
    assert!(!store.directory.exists());
    let seed = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    let mut raw = store.encode_seed(&seed).unwrap();
    raw[0] ^= 1;
    fs::write(seed_path(&store), &raw).unwrap();
    assert!(store.prepare_seed_claim(&db, [9; 32]).await.is_err());
    assert_eq!(raw.as_slice(), fs::read(seed_path(&store)).unwrap());
    let other_store = isolated_store(other.path(), &store.directory);
    assert!(
        other_store
            .decode_seed(
                &store.encode_seed(&seed).unwrap(),
                &store.load_required().unwrap()
            )
            .is_err()
    );
    let unavailable = ProtectedLocalKeyStore {
        account: store.account.clone(),
        directory: store.directory.clone(),
        backend: Backend::Unavailable,
    };
    assert!(unavailable.prepare_seed_claim(&db, [9; 32]).await.is_err());
    let failed = ProtectedLocalKeyStore {
        account: store.account.clone(),
        directory: store.directory.clone(),
        backend: Backend::FailWrite,
    };
    assert!(
        failed
            .load_or_create_seed(&store.load_required().unwrap(), [9; 32], None)
            .is_err()
    );
    let diagnostic = format!("{seed:?}");
    assert!(!diagnostic.contains(&hex::encode(seed.bearer().expose())));
}

#[tokio::test]
async fn frozen_incompatible_package_refuses_and_explicit_recapture_preserves_keys() {
    let root = tempfile::tempdir().unwrap();
    let db = captured_database(root.path()).await;
    let store = isolated_store(db.path(), &root.path().join("keys"));
    let frozen = store
        .package_local_capture(&db, root.path(), [1; 32])
        .await
        .unwrap();
    let original = fs::read(package_path(&store)).unwrap();
    let seed = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    assert!(
        store
            .package_seed_capture(&db, root.path(), [9; 32])
            .await
            .is_err()
    );
    assert_eq!(
        frozen,
        store
            .package_local_capture(&db, root.path(), [1; 32])
            .await
            .unwrap()
    );
    db.cancel_local_shared_state_never_dispatched(frozen.candidate_id())
        .await
        .unwrap();
    db.capture_local_shared_state_never_dispatched(root.path())
        .await
        .unwrap();
    let package = store
        .package_seed_capture(&db, root.path(), [9; 32])
        .await
        .unwrap();
    assert_ne!(package.candidate_id(), frozen.candidate_id());
    assert_ne!(package.stream_id(), frozen.stream_id());
    assert_eq!(package.context(), frozen.context());
    assert_eq!(original, fs::read(package_path(&store)).unwrap());
    assert_eq!(
        seed.protected_storage_bytes(),
        store
            .prepare_seed_claim(&db, [9; 32])
            .await
            .unwrap()
            .protected_storage_bytes()
    );
    assert_eq!(
        package,
        store
            .package_seed_capture(&db, root.path(), [9; 32])
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn seed_secrets_stay_outside_database_export_and_backup() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("db.sqlite"))
        .await
        .unwrap();
    let store = isolated_store(db.path(), &root.path().join("keys"));
    let seed = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    let secrets = seed.protected_storage_bytes();
    let export =
        serde_json::to_vec(&db.export_data("2026-09-22T00:00:00Z".into()).await.unwrap()).unwrap();
    let archive_path = root.path().join("backup.tar.zst");
    db.create_backup_archive(&root.path().join("blobs"), &archive_path)
        .await
        .unwrap();
    let mut surfaces = vec![export];
    let mut archive =
        tar::Archive::new(zstd::Decoder::new(File::open(&archive_path).unwrap()).unwrap());
    for entry in archive.entries().unwrap() {
        let mut bytes = Vec::new();
        entry.unwrap().read_to_end(&mut bytes).unwrap();
        surfaces.push(bytes);
    }
    for path in [
        db.path().to_path_buf(),
        PathBuf::from(format!("{}-wal", db.path().display())),
    ] {
        if let Ok(bytes) = fs::read(path) {
            surfaces.push(bytes);
        }
    }
    for secret in secrets[..96].as_chunks::<32>().0 {
        for surface in &surfaces {
            assert!(!surface.windows(32).any(|window| window == secret));
        }
    }
}

#[tokio::test]
async fn seed_survives_process_exit_and_stopped_same_path_database_replacement() {
    for replace_database in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "protected_local_keys::seed::tests::seed_exit_worker",
                "--ignored",
            ])
            .env("AVEN_SEED_AUTHORITY_TEST_ROOT", root.path())
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(23),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let path = root.path().join("db.sqlite");
        if replace_database {
            for suffix in ["", "-wal", "-shm"] {
                let path = PathBuf::from(format!("{}{suffix}", path.display()));
                match fs::remove_file(path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => panic!("cannot remove stopped test database: {error}"),
                }
            }
        }
        let db = Database::open(&path).await.unwrap();
        let store = isolated_store(&path, &root.path().join("keys"));
        let before = fs::read(package_path(&store)).unwrap();
        let seed = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
        assert_eq!(
            seed.genesis().record().as_slice(),
            fs::read(root.path().join("public-genesis")).unwrap()
        );
        assert_eq!(before, fs::read(package_path(&store)).unwrap());
    }
}

#[tokio::test]
#[ignore = "subprocess worker exits without destructors; invoked with an isolated test root"]
async fn seed_exit_worker() {
    let Some(root) = std::env::var_os("AVEN_SEED_AUTHORITY_TEST_ROOT") else {
        return;
    };
    let root = PathBuf::from(root);
    let db = Database::open(&root.join("db.sqlite")).await.unwrap();
    let store = isolated_store(db.path(), &root.join("keys"));
    let seed = store.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    fs::write(root.join("public-genesis"), seed.genesis().record()).unwrap();
    std::process::exit(23);
}

#[test]
fn seed_create_failure_and_unsafe_files_do_not_replace_authority() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("db.sqlite");
    File::create(&path).unwrap();
    let store = isolated_store(&path, &root.path().join("keys"));
    let package = store.load_or_create().unwrap();
    let failed = ProtectedLocalKeyStore {
        account: store.account.clone(),
        directory: root.path().join("failed"),
        backend: Backend::FailWrite,
    };
    assert_eq!(
        failed
            .load_or_create_seed(&package, [9; 32], None)
            .unwrap_err()
            .kind(),
        ProtectedLocalKeyStoreErrorKind::WriteFailed
    );
    assert!(!failed.seed_marker_path().exists());
    let seed = store.load_or_create_seed(&package, [9; 32], None).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(seed_path(&store), fs::Permissions::from_mode(0o644)).unwrap();
    assert_eq!(
        store
            .load_or_create_seed(&package, [9; 32], None)
            .unwrap_err()
            .kind(),
        ProtectedLocalKeyStoreErrorKind::Corrupt
    );
    fs::set_permissions(seed_path(&store), fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        seed.protected_storage_bytes(),
        store
            .load_or_create_seed(&package, [9; 32], None)
            .unwrap()
            .protected_storage_bytes()
    );
    fs::remove_file(package_path(&store)).unwrap();
    fs::remove_file(store.marker_path()).unwrap();
    assert_eq!(
        store.load_or_create().unwrap_err().kind(),
        ProtectedLocalKeyStoreErrorKind::MissingAuthority
    );
    assert!(!package_path(&store).exists());
}

#[cfg(target_os = "macos")]
#[tokio::test]
#[ignore = "uses two unique test-only macOS Keychain items and temporary markers"]
async fn isolated_seed_keychain_reopen() {
    struct Cleanup(ProtectedLocalKeyStore);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            if let Backend::Keychain(backend) = &self.0.backend {
                backend.delete();
            }
            if let Backend::Keychain(backend) = self.0.seed_backend() {
                backend.delete();
            }
        }
    }
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("db.sqlite"))
        .await
        .unwrap();
    let account = database_account(&db.path().canonicalize().unwrap());
    let cleanup = Cleanup(ProtectedLocalKeyStore {
        backend: Backend::Keychain(KeychainBackend {
            service: format!("fi.zendit.Aven.tests.seed.{account}"),
            account: account.clone(),
        }),
        account,
        directory: root.path().join("markers"),
    });
    let first = cleanup.0.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    let reopened = cleanup.0.prepare_seed_claim(&db, [9; 32]).await.unwrap();
    assert_eq!(
        first.protected_storage_bytes(),
        reopened.protected_storage_bytes()
    );
    if let Backend::Keychain(backend) = cleanup.0.seed_backend() {
        backend.delete();
    }
    assert!(cleanup.0.prepare_seed_claim(&db, [9; 32]).await.is_err());
}
