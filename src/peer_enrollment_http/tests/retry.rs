//! Replacement invitations for an unfinished join.
use super::*;
use crate::protected_local_keys::peer::MAX_JOIN_ATTEMPTS;

fn copy(invitation: &Invitation) -> Invitation {
    Invitation::from_protected_storage(&invitation.protected_storage_bytes()).unwrap()
}

struct Peer {
    db: Database,
    store: ProtectedLocalKeyStore,
    keys: std::path::PathBuf,
}

async fn peer(root: &Path) -> Peer {
    let db = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let keys = root.join("peer-keys");
    let store = isolated_store(&db, &keys).await;
    Peer { db, store, keys }
}

/// Every protected file of the joining store with its exact bytes.
fn protected_files(keys: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(keys)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| !path.to_string_lossy().ends_with(".lock"))
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&path).unwrap(),
            )
        })
        .collect()
}

/// Asserts that every earlier protected file is still present, unchanged.
fn retained(
    before: &std::collections::BTreeMap<String, Vec<u8>>,
    after: &std::collections::BTreeMap<String, Vec<u8>>,
) {
    for (name, bytes) in before {
        assert_eq!(after.get(name), Some(bytes), "{name}");
    }
}

#[tokio::test]
async fn expired_join_finishes_with_a_replacement_from_the_same_keys() {
    let root = tempfile::tempdir().unwrap();
    let clock = test_clock();
    let (db, store, _server, origin, task) =
        adopted_with_clock(root.path(), None, clock.clone()).await;
    let client = Client::new(&origin).unwrap();
    let expires = soon();
    let original = client.invite(&store, &db, expires).await.unwrap();
    let p = peer(root.path()).await;
    client
        .request(&p.store, &p.db, Some(copy(&original)))
        .await
        .unwrap();
    let first = p.store.prepare_peer(&p.db, &origin, None).await.unwrap();
    let pin = p.db.enrollment_pin().await.unwrap();
    advance_clock(&clock, expires);
    // The inviter can no longer admit the expired request.
    assert!(!client.admit(&store, &db).await.unwrap_or(false));
    assert!(!client.complete(&p.store, &p.db).await.unwrap());
    drop(store.tail_inputs(&db, &origin).await.unwrap());
    let replacement = client.invite(&store, &db, expiry()).await.unwrap();
    let before = protected_files(&p.keys);

    // An unknown invitation needs an explicit replacement.
    let error = client
        .request(&p.store, &p.db, Some(copy(&replacement)))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "error enrollment-invitation-conflict");
    client
        .replace(&p.store, &p.db, copy(&replacement))
        .await
        .unwrap();
    let second = p.store.prepare_peer(&p.db, &origin, None).await.unwrap();
    assert_eq!(second.device(), first.device());
    assert_eq!(second.bearer().expose(), first.bearer().expose());
    assert_ne!(second.request(), first.request());
    assert_eq!(p.db.enrollment_pin().await.unwrap(), pin);
    retained(&before, &protected_files(&p.keys));

    // Known invitations select their exact retained attempts.
    client
        .replace(&p.store, &p.db, copy(&replacement))
        .await
        .unwrap();
    for (invitation, expected) in [(&original, &first), (&replacement, &second)] {
        let selected = p
            .store
            .prepare_peer(&p.db, &origin, Some(copy(invitation)))
            .await
            .unwrap();
        assert_eq!(
            selected.protected_storage_bytes().as_slice(),
            expected.protected_storage_bytes().as_slice()
        );
    }

    assert!(client.admit(&store, &db).await.unwrap());
    assert!(client.complete(&p.store, &p.db).await.unwrap());
    client.install(&p.store, &p.db).await.unwrap();
    assert!(matches!(
        p.store.enrollment_readiness(&p.db).await.unwrap(),
        EnrollmentReadiness::Enrolled { .. }
    ));
    drop(p.store.tail_inputs(&p.db, &origin).await.unwrap());
    retained(&before, &protected_files(&p.keys));
    assert!(
        InstallationGuard::acquire(p.db.path())
            .unwrap()
            .ensure_unbound()
            .is_err()
    );

    // Once a response is pinned, no further attempt is added.
    let late = client.invite(&store, &db, expiry()).await.unwrap();
    let after = protected_files(&p.keys);
    let error = client.replace(&p.store, &p.db, late).await.unwrap_err();
    assert_eq!(error.to_string(), "error enrollment-retry-unavailable");
    assert_eq!(protected_files(&p.keys), after);
    task.abort();
}

#[tokio::test]
async fn earlier_admission_with_a_lost_reply_completes_once_after_a_replacement() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let original = client.invite(&store, &db, expiry()).await.unwrap();
    let p = peer(root.path()).await;
    client
        .request(&p.store, &p.db, Some(original))
        .await
        .unwrap();
    let first = p.store.prepare_peer(&p.db, &origin, None).await.unwrap();
    // The inviter commits the admission; this device has not seen it yet.
    assert!(client.admit(&store, &db).await.unwrap());
    let replacement = client.invite(&store, &db, expiry()).await.unwrap();
    client
        .replace(&p.store, &p.db, copy(&replacement))
        .await
        .unwrap();
    let second = p.store.prepare_peer(&p.db, &origin, None).await.unwrap();
    assert_ne!(second.request(), first.request());
    // Membership uniqueness refuses admitting the same keys again.
    let admitted = members(&store, &db, &origin).await;
    assert!(client.admit(&store, &db).await.is_err());
    assert_eq!(members(&store, &db, &origin).await, admitted);
    // Fault injection: the latest attempt's mailbox fails, which must not
    // stop the earlier attempt from completing.
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", server.path().display()))
        .await
        .unwrap();
    sqlx::query("DELETE FROM server_membership_invitations WHERE handle=?")
        .bind(replacement.handle().as_slice())
        .execute(&pool)
        .await
        .unwrap();
    assert!(client.complete(&p.store, &p.db).await.unwrap());
    // The pinned response selects the admitted attempt, not the latest.
    assert_eq!(
        p.store
            .prepare_peer(&p.db, &origin, None)
            .await
            .unwrap()
            .protected_storage_bytes()
            .as_slice(),
        first.protected_storage_bytes().as_slice()
    );
    client.install(&p.store, &p.db).await.unwrap();
    drop(p.store.tail_inputs(&p.db, &origin).await.unwrap());
    let devices = members(&store, &db, &origin).await;
    assert_eq!(devices.iter().filter(|d| **d == first.device()).count(), 1);
    assert_eq!(devices.len(), 2);
    task.abort();
}

async fn members(store: &ProtectedLocalKeyStore, db: &Database, origin: &str) -> Vec<[u8; 32]> {
    let inputs = store.active_inputs(db, origin).await.unwrap();
    inputs.membership.devices().collect()
}

/// An invitation with the given vault and inviter key and a chosen PSK.
fn invitation(vault: &[u8], inviter: &[u8], psk: u8) -> Invitation {
    Invitation::from_protected_storage(&[vault, inviter, &[psk; 32]].concat()).unwrap()
}

#[tokio::test]
async fn refused_replacements_keep_the_join_and_its_data_unchanged() {
    let root = tempfile::tempdir().unwrap();
    let counts = Arc::new(ExchangeCounts::default());
    let (db, store, _server, origin, task) = adopted_with(root.path(), Some(counts.clone())).await;
    let client = Client::new(&origin).unwrap();
    let original = client.invite(&store, &db, expiry()).await.unwrap();
    let bytes = original.protected_storage_bytes();
    let (vault, inviter) = (&bytes[..32], &bytes[32..64]);
    let p = peer(root.path()).await;
    client
        .request(&p.store, &p.db, Some(original))
        .await
        .unwrap();
    let pin = p.db.enrollment_pin().await.unwrap();
    let before = protected_files(&p.keys);

    let error = p
        .store
        .replace_peer(
            &p.db,
            "https://other.invalid",
            invitation(vault, inviter, 1),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "error enrollment-context");
    for other in [
        invitation(&[9; 32], inviter, 1),
        invitation(vault, &[9; 32], 1),
    ] {
        let error = p
            .store
            .replace_peer(&p.db, &origin, other)
            .await
            .err()
            .unwrap();
        assert_eq!(error.to_string(), "error enrollment-retry-context");
    }
    assert_eq!(protected_files(&p.keys), before);

    // Replacements are retained through the limit; another is refused.
    for psk in 1..MAX_JOIN_ATTEMPTS as u8 {
        p.store
            .replace_peer(&p.db, &origin, invitation(vault, inviter, psk))
            .await
            .unwrap();
    }
    assert_eq!(
        p.store.peer_attempts(&p.db, &origin).await.unwrap().len(),
        MAX_JOIN_ATTEMPTS
    );
    let full = protected_files(&p.keys);
    retained(&before, &full);
    let error = p
        .store
        .replace_peer(&p.db, &origin, invitation(vault, inviter, u8::MAX))
        .await
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "error enrollment-retry-limit");
    assert_eq!(protected_files(&p.keys), full);
    // A known invitation still selects its attempt at the limit.
    p.store
        .replace_peer(&p.db, &origin, invitation(vault, inviter, 2))
        .await
        .unwrap();

    // Local data added while joining refuses another attempt and later
    // installation, and stays in place.
    let workspace = p.db.list_workspaces().await.unwrap().remove(0);
    p.db.create_task(
        &workspace,
        aven_core::operations::TaskDraft {
            title: "LOCAL WHILE JOINING".into(),
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
    .unwrap();
    let error = p
        .store
        .replace_peer(&p.db, &origin, invitation(vault, inviter, u8::MAX - 1))
        .await
        .err()
        .unwrap();
    assert!(
        error.to_string().starts_with("error shared-state-install"),
        "{error}"
    );
    assert_eq!(protected_files(&p.keys), full);
    assert_eq!(p.db.enrollment_pin().await.unwrap(), pin);

    // The original attempt still completes past unregistered replacements,
    // and the final empty-target check still refuses installation.
    assert!(client.admit(&store, &db).await.unwrap());
    let mailbox_reads = || counts.mailboxes.lock().unwrap().values().sum::<usize>();
    let reads_before = mailbox_reads();
    assert!(client.complete(&p.store, &p.db).await.unwrap());
    assert_eq!(mailbox_reads() - reads_before, MAX_JOIN_ATTEMPTS);
    let error = client.install(&p.store, &p.db).await.unwrap_err();
    assert!(
        error.to_string().starts_with("error shared-state-install"),
        "{error}"
    );
    retained(&full, &protected_files(&p.keys));
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", p.db.path().display()))
        .await
        .unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM tasks")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "LOCAL WHILE JOINING");
    task.abort();
}

#[tokio::test]
async fn process_exit_while_replacing_and_completing_a_replacement() {
    let root = tempfile::tempdir().unwrap();
    let clock = test_clock();
    let (db, store, server, origin, task) =
        adopted_with_clock(root.path(), None, clock.clone()).await;
    let client = Client::new(&origin).unwrap();
    let expires = soon();
    let original = client.invite(&store, &db, expires).await.unwrap();
    let p = peer(root.path()).await;
    client
        .request(&p.store, &p.db, Some(original))
        .await
        .unwrap();
    advance_clock(&clock, expires);
    drop(store.tail_inputs(&db, &origin).await.unwrap());
    let replacement = client.invite(&store, &db, expiry()).await.unwrap();
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(root.path().join("replacement.test")).unwrap();
        file.write_all(&replacement.protected_storage_bytes())
            .unwrap();
    }
    let posted = || async {
        server
            .membership_mailbox(replacement.vault(), replacement.handle())
            .await
            .unwrap()
            .request
    };
    let attempt = |files: &std::collections::BTreeMap<String, Vec<u8>>| {
        files
            .iter()
            .find(|(name, _)| name.ends_with(".peer-attempt-1"))
            .map(|(_, bytes)| bytes.clone())
    };

    // Exit after the protected write, before its SQLite commitment.
    crash_worker(root.path(), &origin, "replace", "peer-attempt-1").await;
    let written = attempt(&protected_files(&p.keys)).unwrap();
    assert!(
        p.db.enrollment_artifact("peer-attempt-1")
            .await
            .unwrap()
            .is_none()
    );
    assert!(posted().await.is_none());
    // Committed without dispatch, as an exit before posting would leave it.
    let second = p
        .store
        .replace_peer(&p.db, &origin, copy(&replacement))
        .await
        .unwrap();
    assert!(
        p.db.enrollment_artifact("peer-attempt-1")
            .await
            .unwrap()
            .is_some()
    );
    assert!(posted().await.is_none());
    client
        .replace(&p.store, &p.db, copy(&replacement))
        .await
        .unwrap();
    let files = protected_files(&p.keys);
    assert_eq!(attempt(&files).unwrap(), written);
    assert!(!files.keys().any(|name| name.ends_with(".peer-attempt-2")));
    assert_eq!(posted().await.as_deref(), Some(second.request()));

    assert!(client.admit(&store, &db).await.unwrap());
    for kind in ["peer-response", "peer-verified", "peer-ready"] {
        crash_worker(root.path(), &origin, "peer", kind).await;
    }
    assert!(client.complete(&p.store, &p.db).await.unwrap());
    client.install(&p.store, &p.db).await.unwrap();
    drop(p.store.tail_inputs(&p.db, &origin).await.unwrap());
    task.abort();
}

/// The protected file for `kind` in the joining store.
fn protected_path(keys: &Path, kind: &str) -> std::path::PathBuf {
    std::fs::read_dir(keys)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path.to_string_lossy().ends_with(&format!(".{kind}")))
        .unwrap()
}

#[tokio::test]
async fn missing_or_corrupt_protected_join_records_refuse_an_admitted_grant() {
    for (kind, damage) in [
        ("peer-sent", "delete"),
        ("peer-sent", "corrupt"),
        ("peer-attempt-1", "delete"),
    ] {
        let root = tempfile::tempdir().unwrap();
        let (db, store, _server, origin, task) = adopted(root.path()).await;
        let client = Client::new(&origin).unwrap();
        let original = client.invite(&store, &db, expiry()).await.unwrap();
        let bytes = original.protected_storage_bytes();
        let p = peer(root.path()).await;
        client
            .request(&p.store, &p.db, Some(original))
            .await
            .unwrap();
        assert!(
            p.store
                .replace_peer(&p.db, &origin, invitation(&bytes[..32], &bytes[32..64], 1))
                .await
                .is_ok()
        );
        assert!(client.admit(&store, &db).await.unwrap());
        let path = protected_path(&p.keys, kind);
        if damage == "delete" {
            std::fs::remove_file(&path).unwrap();
        } else {
            let mut frame = std::fs::read(&path).unwrap();
            frame[50] ^= 1;
            std::fs::write(&path, frame).unwrap();
        }
        let expected = if damage == "corrupt" {
            "error enrollment-protected-corrupt"
        } else {
            "error enrollment-protected-missing"
        };
        let error = client.complete(&p.store, &p.db).await.unwrap_err();
        assert_eq!(error.to_string(), expected, "{kind} {damage}: complete");
        let error = crate::sync::encrypted::await_join(
            &client,
            &p.store,
            &p.db,
            None,
            false,
            std::time::Instant::now() + std::time::Duration::from_secs(5),
            &|_| {},
        )
        .await
        .unwrap_err();
        assert_eq!(error.to_string(), expected, "{kind} {damage}: join");
        assert!(
            !protected_files(&p.keys)
                .keys()
                .any(|name| name.ends_with(".peer-response"))
        );
        task.abort();
    }
}

/// Waits in the join caller with a short deadline, as `run_join` does.
async fn join(client: &Client, p: &Peer, invitation: Invitation) -> anyhow::Result<()> {
    crate::sync::encrypted::await_join(
        client,
        &p.store,
        &p.db,
        Some(invitation),
        true,
        std::time::Instant::now() + std::time::Duration::from_secs(20),
        &|_| {},
    )
    .await
}

#[tokio::test]
async fn refused_newest_request_keeps_waiting_for_an_earlier_admission() {
    // An unregistered replacement: the server refuses its request and
    // mailbox, like a replacement whose invitation it no longer serves.
    let unregistered = |original: &Invitation| {
        let bytes = original.protected_storage_bytes();
        invitation(&bytes[..32], &bytes[32..64], 1)
    };

    // The earlier admission is committed, but its mailbox is busy at first.
    let root = tempfile::tempdir().unwrap();
    let counts = Arc::new(ExchangeCounts::default());
    let (db, store, _server, origin, task) = adopted_with(root.path(), Some(counts.clone())).await;
    let client = Client::new(&origin).unwrap();
    let original = client.invite(&store, &db, expiry()).await.unwrap();
    let handle = original.handle();
    let replacement = unregistered(&original);
    let p = peer(root.path()).await;
    client
        .request(&p.store, &p.db, Some(original))
        .await
        .unwrap();
    assert!(client.admit(&store, &db).await.unwrap());
    counts.busy_mailboxes.lock().unwrap().insert(handle, 3);
    join(&client, &p, replacement).await.unwrap();
    assert!(counts.mailboxes.lock().unwrap()[&handle] > 3);
    task.abort();

    // The earlier admission arrives only after the first completion check.
    let root = tempfile::tempdir().unwrap();
    let counts = Arc::new(ExchangeCounts::default());
    let (db, store, _server, origin, task) = adopted_with(root.path(), Some(counts.clone())).await;
    let client = Client::new(&origin).unwrap();
    let original = client.invite(&store, &db, expiry()).await.unwrap();
    let handle = original.handle();
    let replacement = unregistered(&original);
    let p = peer(root.path()).await;
    client
        .request(&p.store, &p.db, Some(original))
        .await
        .unwrap();
    let admit = async {
        while counts
            .mailboxes
            .lock()
            .unwrap()
            .get(&handle)
            .copied()
            .unwrap_or(0)
            < 2
        {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        // The single enrollment permit may be busy with the joiner's poll.
        loop {
            match client.admit(&store, &db).await {
                Ok(admitted) => break assert!(admitted),
                Err(error) if error.to_string() == "error enrollment-busy" => {}
                Err(error) => panic!("{error:#}"),
            }
        }
    };
    let (joined, ()) = tokio::time::timeout(std::time::Duration::from_secs(60), async {
        tokio::join!(join(&client, &p, replacement), admit)
    })
    .await
    .expect("join and admission settle");
    joined.unwrap();
    client.install(&p.store, &p.db).await.unwrap();
    task.abort();
}

#[tokio::test]
async fn sent_record_written_before_its_commitment_is_committed_before_dispatch() {
    let root = tempfile::tempdir().unwrap();
    let (db, store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let original = client.invite(&store, &db, expiry()).await.unwrap();
    {
        use std::io::Write;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(root.path().join("invitation.test")).unwrap();
        file.write_all(&original.protected_storage_bytes()).unwrap();
    }
    let posted = || async {
        server
            .membership_mailbox(original.vault(), original.handle())
            .await
            .unwrap()
            .request
    };
    // Exit after the protected peer-sent write, before its SQLite pin.
    crash_worker(root.path(), &origin, "initial", "peer-sent").await;
    let p = peer(root.path()).await;
    let written = std::fs::read(protected_path(&p.keys, "peer-sent")).unwrap();
    assert!(
        p.db.enrollment_artifact("peer-sent")
            .await
            .unwrap()
            .is_none()
    );
    assert!(posted().await.is_none());

    // Preparation commits the written record unchanged, without posting.
    let peer = p.store.prepare_peer(&p.db, &origin, None).await.unwrap();
    // The pin commits the frame's content: the original request's digest.
    assert_eq!(
        p.db.enrollment_artifact("peer-sent")
            .await
            .unwrap()
            .as_deref(),
        Some(sha2::Sha256::digest(sha2::Sha256::digest(peer.request())).as_slice())
    );
    assert_eq!(
        std::fs::read(protected_path(&p.keys, "peer-sent")).unwrap(),
        written
    );
    assert!(posted().await.is_none());
    client.request(&p.store, &p.db, None).await.unwrap();
    assert_eq!(posted().await.as_deref(), Some(peer.request()));
    task.abort();
}
