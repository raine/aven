//! Replacement invitations for an unfinished join. Expiry here is the
//! server's real clock passing a short declared invitation lifetime.
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
    let store = isolated_store(db.path(), &keys);
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
    let (db, store, _server, origin, task) = adopted(root.path()).await;
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
    past(expires).await;
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
    let (db, store, _server, origin, task) = adopted(root.path()).await;
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

    // Three replacements are retained; a fourth is refused.
    for psk in 1..MAX_JOIN_ATTEMPTS as u8 {
        p.store
            .replace_peer(&p.db, &origin, invitation(vault, inviter, psk))
            .await
            .unwrap();
    }
    let full = protected_files(&p.keys);
    retained(&before, &full);
    let error = p
        .store
        .replace_peer(&p.db, &origin, invitation(vault, inviter, 9))
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
        .replace_peer(&p.db, &origin, invitation(vault, inviter, 8))
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
    assert!(client.complete(&p.store, &p.db).await.unwrap());
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
    let (db, store, server, origin, task) = adopted(root.path()).await;
    let client = Client::new(&origin).unwrap();
    let expires = soon();
    let original = client.invite(&store, &db, expires).await.unwrap();
    let p = peer(root.path()).await;
    client
        .request(&p.store, &p.db, Some(original))
        .await
        .unwrap();
    past(expires).await;
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
