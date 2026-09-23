use super::*;
use crate::protected_local_keys::tests::isolated_store;
use aven_core::{
    choices::TaskSource,
    operations::{TaskDraft, TaskUpdate},
};
use std::path::Path;
struct Fixture {
    root: tempfile::TempDir,
    seed: Database,
    peer: Database,
    server: Database,
    seed_store: ProtectedLocalKeyStore,
    peer_store: ProtectedLocalKeyStore,
    origin: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn serve(server: Database, address: &str) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind(address).await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = seed_bootstrap_http::router(
        server.clone(),
        Some(crate::seed_bootstrap_http::tests::setup()),
        Default::default(),
    )
    .merge(crate::peer_enrollment_http::router(server.clone()))
    .merge(router(server));
    (
        origin,
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        }),
    )
}
async fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let (seed, seed_store, authority, _) =
        crate::seed_bootstrap_http::tests::fixture(root.path()).await;
    let server = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let (origin, task) = serve(server.clone(), "127.0.0.1:0").await;
    let bootstrap = seed_bootstrap_http::Client::new(&origin).unwrap();
    bootstrap
        .claim(
            authority.genesis(),
            aven_core::sync::seed_claim::ClaimAuthentication::SetupSecret(&Secret::new([7; 32])),
        )
        .await
        .unwrap();
    bootstrap.resume(&seed_store, &seed).await.unwrap();
    let peer = Database::open(&root.path().join("peer.sqlite"))
        .await
        .unwrap();
    let peer_store = isolated_store(peer.path(), &root.path().join("peer-keys"));
    let enrollment = crate::peer_enrollment_http::Client::new(&origin).unwrap();
    let expiry = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3500;
    let invitation = enrollment.invite(&seed_store, &seed, expiry).await.unwrap();
    enrollment
        .request(&peer_store, &peer, Some(invitation))
        .await
        .unwrap();
    assert!(enrollment.admit(&seed_store, &seed).await.unwrap());
    assert!(enrollment.complete(&peer_store, &peer).await.unwrap());
    enrollment
        .install(&peer_store, &peer, &root.path().join("peer-blobs"))
        .await
        .unwrap();
    assert_ne!(
        seed.meta("client_id").await.unwrap(),
        peer.meta("client_id").await.unwrap()
    );
    Fixture {
        root,
        seed,
        peer,
        server,
        seed_store,
        peer_store,
        origin,
        task,
    }
}
async fn drain(client: &Client, store: &ProtectedLocalKeyStore, db: &Database) {
    for _ in 0..100 {
        match client.round(store, db).await {
            Ok(true) => return,
            Ok(false) => {}
            // Nonblocking installation exclusion can be briefly retained by a
            // concurrently spawned child until its close-on-exec descriptors close.
            Err(error) if error.to_string() == "error installation-busy" => {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            Err(error) => panic!("{error:#}"),
        }
    }
    panic!("round budget");
}
async fn converge(f: &Fixture) {
    let c = Client::new(&f.origin).unwrap();
    drain(&c, &f.seed_store, &f.seed).await;
    drain(&c, &f.peer_store, &f.peer).await;
    drain(&c, &f.seed_store, &f.seed).await;
}
fn draft(title: &str) -> TaskDraft {
    TaskDraft {
        title: title.into(),
        description: String::new(),
        project: Some("app".into()),
        status: "todo".into(),
        priority: "none".into(),
        source: TaskSource::Cli,
        labels: vec![],
        metadata: vec![],
        available_at: None,
        due_on: None,
        is_epic: false,
    }
}
async fn title(db: &Database, id: &str) -> String {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar("SELECT title FROM tasks WHERE id=?")
        .bind(id)
        .fetch_one(&mut *c)
        .await
        .unwrap()
}
async fn scalar(db: &Database, sql: &str) -> i64 {
    let mut c = aven_core::test_support::acquire(db).await.unwrap();
    sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
        .fetch_one(&mut *c)
        .await
        .unwrap()
}
fn files(path: &Path) -> Vec<Vec<u8>> {
    let mut bytes = std::fs::read_dir(path)
        .unwrap()
        .map(|p| std::fs::read(p.unwrap().path()).unwrap())
        .collect::<Vec<_>>();
    bytes.sort();
    bytes
}

#[tokio::test]
async fn independent_clients_create_conflict_resolve_delete_restore_and_keep_images() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let seed_id = f.seed.meta("client_id").await.unwrap();
    let peer_id = f.peer.meta("client_id").await.unwrap();
    let image_bytes = files(&f.root.path().join("objects/sha256"));
    let task = f
        .seed
        .create_task(&w, draft("private fresh task"))
        .await
        .unwrap()
        .task;
    converge(&f).await;
    assert_eq!(title(&f.peer, task.id.as_str()).await, "private fresh task");
    f.seed
        .update_task(
            &w,
            &task.id,
            TaskUpdate {
                title: Some("offline seed".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.peer
        .update_task(
            &w,
            &task.id,
            TaskUpdate {
                title: Some("offline peer".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        f.seed
            .task_conflicts(&w, &task.id, Some("title"))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.peer
            .task_conflicts(&w, &task.id, Some("title"))
            .await
            .unwrap()
            .len(),
        1
    );
    f.seed
        .resolve_conflict(&w, &task.id, "title", "chosen title")
        .await
        .unwrap();
    f.peer
        .add_note(&w, &task.id, "private note".into())
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(title(&f.peer, task.id.as_str()).await, "chosen title");
    let export = f.seed.export_data("now".into()).await.unwrap();
    let image_task = export.tables.task_attachments[0].task_id.clone();
    f.seed
        .update_task(
            &w,
            &image_task,
            TaskUpdate {
                deleted: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_images WHERE unreferenced_at IS NOT NULL"
        )
        .await,
        1
    );
    f.peer
        .update_task(
            &w,
            &image_task,
            TaskUpdate {
                deleted: Some(false),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_images WHERE unreferenced_at IS NOT NULL"
        )
        .await,
        0
    );
    assert_eq!(files(&f.root.path().join("objects/sha256")), image_bytes);
    assert_eq!(
        files(&f.root.path().join("peer-blobs/objects/sha256")),
        image_bytes
    );
    assert_eq!(scalar(&f.server, "SELECT count(*) FROM changes").await, 0);
    assert_eq!(f.seed.meta("client_id").await.unwrap(), seed_id);
    assert_eq!(f.peer.meta("client_id").await.unwrap(), peer_id);
    assert_eq!(
        scalar(
            &f.seed,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn frozen_restart_lost_ack_and_server_restart_keep_exact_identity() {
    let mut f = fixture().await;
    converge(&f).await;
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(&w, draft("restart private"))
        .await
        .unwrap();
    let (record, context, bearer) = {
        let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
        (
            f.peer
                .prepare_encrypted_tail(&inputs.authority)
                .await
                .unwrap()
                .unwrap(),
            inputs.authority.context.clone(),
            Secret::new(*inputs.bearer.expose()),
        )
    };
    let reopened = Database::open(f.peer.path()).await.unwrap();
    let store = isolated_store(reopened.path(), &f.root.path().join("peer-keys"));
    {
        let inputs = store.tail_inputs(&reopened, &f.origin).await.unwrap();
        assert_eq!(
            reopened
                .prepare_encrypted_tail(&inputs.authority)
                .await
                .unwrap()
                .unwrap(),
            record
        );
    }
    let c = Client::new(&f.origin).unwrap();
    let Reply::Appended(first) = c
        .exchange(
            &context,
            &bearer,
            Operation::Append {
                record: record.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        1
    );
    f.task.abort();
    let _ = (&mut f.task).await;
    let mut server_process = spawn_server(&f).await;
    let Reply::Appended(second) = c
        .exchange(
            &context,
            &bearer,
            Operation::Append {
                record: record.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(first == second);
    let Reply::Found(accepted) = c
        .exchange(
            &context,
            &bearer,
            Operation::Lookup {
                operation_id: first.operation_id.clone(),
                expected: Some(first),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(accepted.record, record);
    drain(&c, &store, &reopened).await;
    converge(&f).await;
    assert_eq!(
        scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
        2
    );
    server_process.kill().await.unwrap();
    server_process.wait().await.unwrap();
}

#[tokio::test]
async fn offline_metadata_notes_and_relationships_converge() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let mut d = draft("metadata owner");
    d.project = Some("new-project".into());
    d.labels = vec!["new-label".into()];
    d.metadata = vec![aven_core::metadata::TaskMetadataInput {
        expected_field_id: None,
        key: "owner".into(),
        value: "initial".into(),
    }];
    let first = f
        .seed
        .create_task_with_options(
            &w,
            d,
            aven_core::operations::TaskCreationOptions::standalone(
                aven_core::operations::TaskCreationUndo::None,
            )
            .with_create_missing_labels(),
        )
        .await
        .unwrap()
        .task;
    let mut child = draft("related child");
    child.project = Some("new-project".into());
    let second = f.seed.create_task(&w, child).await.unwrap().task;
    converge(&f).await;
    for (db, value) in [(&f.seed, "seed-owner"), (&f.peer, "peer-owner")] {
        db.update_task(
            &w,
            &first.id,
            TaskUpdate {
                set_metadata: vec![aven_core::metadata::TaskMetadataInput {
                    expected_field_id: None,
                    key: "owner".into(),
                    value: value.into(),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    f.peer
        .add_task_related_link(&w, &first.id, &second.id)
        .await
        .unwrap();
    f.seed
        .add_task_dependency(&w, &first.id, &second.id)
        .await
        .unwrap();
    f.seed
        .add_task_to_epic(&w, &second.id, &first.id)
        .await
        .unwrap();
    f.peer
        .add_note(&w, &second.id, "peer offline note".into())
        .await
        .unwrap();
    converge(&f).await;
    let conflicts = f.seed.task_conflicts(&w, &first.id, None).await.unwrap();
    assert_eq!(conflicts.len(), 1);
    let mut conn = aven_core::test_support::acquire(&f.seed).await.unwrap();
    let field: String =
        sqlx::query_scalar("SELECT field FROM conflicts WHERE resolved=0 AND task_id=?")
            .bind(&first.id)
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    drop(conn);
    f.seed
        .resolve_conflict(&w, &first.id, &field, "resolved owner")
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(
            scalar(db, "SELECT count(*) FROM task_related_links WHERE linked=1").await,
            1
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM task_dependencies").await,
            1
        );
        assert_eq!(
            scalar(
                db,
                "SELECT count(*) FROM task_metadata WHERE value='resolved owner'"
            )
            .await,
            1
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM conflicts WHERE resolved=0").await,
            0
        );
    }
}

#[tokio::test]
async fn current_auth_prefix_tamper_and_whole_page_rollback() {
    let f = fixture().await;
    converge(&f).await;
    let c = Client::new(&f.origin).unwrap();
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    f.peer
        .create_task(&w, draft("pending rollback"))
        .await
        .unwrap();
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let a = &inputs.authority;
    let frozen = f.peer.prepare_encrypted_tail(a).await.unwrap().unwrap();
    for field in 0..5 {
        let mut wrong = a.context.clone();
        match field {
            0 => wrong.vault[0] ^= 1,
            1 => wrong.stream[0] ^= 1,
            2 => wrong.head[0] ^= 1,
            3 => wrong.device[0] ^= 1,
            _ => wrong.descriptor[0] ^= 1,
        };
        assert!(
            c.exchange(
                &wrong,
                &inputs.bearer,
                Operation::Append {
                    record: frozen.clone()
                }
            )
            .await
            .is_err()
        );
    }
    assert!(
        c.exchange(
            &a.context,
            &Secret::new([0; 32]),
            Operation::Append {
                record: frozen.clone()
            }
        )
        .await
        .is_err()
    );
    assert!(
        c.exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append {
                record: frozen[..20].to_vec()
            }
        )
        .await
        .is_err()
    );
    let prefix_id = {
        let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
        sqlx::query_scalar::<_, String>("SELECT operation_id FROM server_bootstrap_prefix LIMIT 1")
            .fetch_one(&mut *conn)
            .await
            .unwrap()
    };
    // Operation ID is public framing at byte 156; server cannot authenticate AEAD.
    let mut collision = frozen.clone();
    collision[160..176].copy_from_slice(prefix_id.as_bytes());
    let error = c
        .exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append { record: collision },
        )
        .await
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("prefix-identity-collision"), "{error}");
    let Reply::Appended(mapping) = c
        .exchange(
            &a.context,
            &inputs.bearer,
            Operation::Append {
                record: frozen.clone(),
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    let before = f.peer.encrypted_tail_cursor(a).await.unwrap();
    let Reply::Page(page) = c
        .exchange(
            &a.context,
            &inputs.bearer,
            Operation::Pull {
                after: before,
                limit: 16,
                watermark: None,
            },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    let mut bad = page.clone();
    let last = bad.records[0].record.len() - 1;
    bad.records[0].record[last] ^= 1;
    assert!(f.peer.apply_encrypted_tail_page(a, &bad).await.is_err());
    assert_eq!(f.peer.encrypted_tail_cursor(a).await.unwrap(), before);
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        1
    );
    // A real transaction failure after domain/rank work must also roll back.
    {
        let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
        sqlx::query("CREATE TRIGGER fail_tail_cursor BEFORE UPDATE ON meta WHEN NEW.key='sync_cursor' BEGIN SELECT RAISE(ABORT,'injected'); END").execute(&mut *conn).await.unwrap();
    }
    assert!(f.peer.apply_encrypted_tail_page(a, &page).await.is_err());
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        1
    );
    {
        let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
        sqlx::query("DROP TRIGGER fail_tail_cursor")
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    f.peer.apply_encrypted_tail_page(a, &page).await.unwrap();
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        0
    );
    let mut wrong = mapping;
    wrong.commitment[0] ^= 1;
    assert!(f.peer.observe_encrypted_tail(a, &wrong).await.is_err());
}

#[tokio::test]
#[ignore = "subprocess encrypted tail crash worker"]
async fn process_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_TAIL_ROOT").unwrap());
    let db = Database::open(&root.join("peer.sqlite")).await.unwrap();
    let store = isolated_store(db.path(), &root.join("peer-keys"));
    Client::new(&std::env::var("AVEN_TAIL_ORIGIN").unwrap())
        .unwrap()
        .round(&store, &db)
        .await
        .unwrap();
}
#[tokio::test]
async fn process_exit_before_dispatch_after_acceptance_and_during_page_commit() {
    for stage in [
        "frozen",
        "after-append",
        "before-page-commit",
        "after-page-commit",
    ] {
        let f = fixture().await;
        converge(&f).await;
        let w = f.peer.list_workspaces().await.unwrap().remove(0);
        let local = f.peer.create_task(&w, draft(stage)).await.unwrap().task;
        let remote = f
            .seed
            .create_task(&w, draft("pull crash remote"))
            .await
            .unwrap()
            .task;
        let c = Client::new(&f.origin).unwrap();
        drain(&c, &f.seed_store, &f.seed).await;
        let before = f.peer.meta("sync_cursor").await.unwrap();
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "encrypted_tail_http::tests::process_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("AVEN_TAIL_ROOT", f.root.path())
            .env("AVEN_TAIL_ORIGIN", &f.origin)
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
        if stage != "after-page-commit" {
            assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), before);
        }
        let frozen: Option<Vec<u8>> = {
            let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
            sqlx::query_scalar("SELECT record FROM local_e2ee_outbox")
                .fetch_optional(&mut *conn)
                .await
                .unwrap()
        };
        let reopened = Database::open(f.peer.path()).await.unwrap();
        let store = isolated_store(reopened.path(), &f.root.path().join("peer-keys"));
        if let Some(bytes) = frozen {
            let inputs = store.tail_inputs(&reopened, &f.origin).await.unwrap();
            assert_eq!(
                reopened
                    .prepare_encrypted_tail(&inputs.authority)
                    .await
                    .unwrap()
                    .unwrap(),
                bytes
            );
        }
        drain(&c, &store, &reopened).await;
        drain(&c, &f.seed_store, &f.seed).await;
        assert_eq!(title(&f.seed, local.id.as_str()).await, stage);
        assert_eq!(
            title(&reopened, remote.id.as_str()).await,
            "pull crash remote"
        );
        assert_eq!(
            scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
            3
        );
        assert_eq!(
            scalar(
                &reopened,
                "SELECT count(*) FROM changes WHERE server_seq IS NULL"
            )
            .await,
            0
        );
    }
}

#[tokio::test]
async fn same_id_different_ciphertext_requires_equal_domain_not_identity() {
    for divergent in [false, true] {
        let f = fixture().await;
        converge(&f).await;
        let w = f.seed.list_workspaces().await.unwrap().remove(0);
        let task = f
            .seed
            .create_task(&w, draft("shared identity target"))
            .await
            .unwrap()
            .task;
        converge(&f).await;
        f.seed
            .update_task(
                &w,
                &task.id,
                TaskUpdate {
                    title: Some("same meaning".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        f.peer
            .update_task(
                &w,
                &task.id,
                TaskUpdate {
                    title: Some(
                        if divergent {
                            "different meaning"
                        } else {
                            "same meaning"
                        }
                        .into(),
                    ),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        // Controlled duplicate identity fixture, not recurrence generation or server acceptance.
        let (id, at): (String, String) = {
            let mut conn = aven_core::test_support::acquire(&f.seed).await.unwrap();
            sqlx::query_as("SELECT change_id,created_at FROM changes WHERE server_seq IS NULL")
                .fetch_one(&mut *conn)
                .await
                .unwrap()
        };
        {
            let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
            let old: String =
                sqlx::query_scalar("SELECT change_id FROM changes WHERE server_seq IS NULL")
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap();
            sqlx::query("UPDATE changes SET change_id=?,created_at=? WHERE change_id=?")
                .bind(&id)
                .bind(&at)
                .bind(&old)
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query("UPDATE field_versions SET version=? WHERE version=?")
                .bind(&id)
                .bind(&old)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        let peer_record = {
            let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
            f.peer
                .prepare_encrypted_tail(&inputs.authority)
                .await
                .unwrap()
                .unwrap()
        };
        let c = Client::new(&f.origin).unwrap();
        drain(&c, &f.seed_store, &f.seed).await;
        let accepted_record: Vec<u8> = {
            let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
            sqlx::query_scalar("SELECT record FROM server_e2ee_tail WHERE operation_id=?")
                .bind(&id)
                .fetch_one(&mut *conn)
                .await
                .unwrap()
        };
        assert_ne!(peer_record, accepted_record);
        let before = f.peer.meta("sync_cursor").await.unwrap();
        if divergent {
            assert!(
                c.round(&f.peer_store, &f.peer)
                    .await
                    .unwrap_err()
                    .to_string()
                    .contains("same-id-divergence")
            );
            assert_eq!(f.peer.meta("sync_cursor").await.unwrap(), before);
            assert_eq!(
                scalar(
                    &f.peer,
                    "SELECT count(*) FROM changes WHERE server_seq IS NULL"
                )
                .await,
                1
            );
            assert_eq!(
                scalar(&f.peer, "SELECT blocked FROM local_e2ee_outbox").await,
                1
            );
            assert!(c.round(&f.peer_store, &f.peer).await.is_err());
            assert_eq!(title(&f.peer, task.id.as_str()).await, "different meaning");
        } else {
            drain(&c, &f.peer_store, &f.peer).await;
            assert_eq!(
                scalar(
                    &f.peer,
                    "SELECT count(*) FROM changes WHERE server_seq IS NULL"
                )
                .await,
                0
            );
            let local_origin: String = {
                let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
                sqlx::query_scalar("SELECT client_id FROM changes WHERE change_id=?")
                    .bind(&id)
                    .fetch_one(&mut *conn)
                    .await
                    .unwrap()
            };
            assert_eq!(Some(local_origin), f.peer.meta("client_id").await.unwrap());
        }
        assert_eq!(
            scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
            3
        );
    }
}

#[tokio::test]
async fn attachment_pending_prefix_is_refused_without_partial_task_acceptance() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    let task = f
        .peer
        .create_task(&w, draft("must remain local"))
        .await
        .unwrap()
        .task;
    let bytes = files(&f.root.path().join("objects/sha256")).remove(0);
    f.peer
        .add_task_attachment(
            &w,
            &f.root.path().join("peer-blobs"),
            Default::default(),
            &task.id,
            aven_core::operations::AttachmentAddInput {
                filename: None,
                alt_text: None,
                declared_media_type: None,
                bytes,
                optimization_policy: aven_core::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        )
        .await
        .unwrap();
    let c = Client::new(&f.origin).unwrap();
    assert!(c.round(&f.peer_store, &f.peer).await.is_err());
    assert_eq!(
        scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
        1
    );
    assert_eq!(
        scalar(&f.peer, "SELECT count(*) FROM local_e2ee_outbox").await,
        0
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        2
    );
    assert_eq!(title(&f.peer, task.id.as_str()).await, "must remain local");
    let remote = f
        .seed
        .create_task(&w, draft("read while upload blocked"))
        .await
        .unwrap()
        .task;
    drain(&c, &f.seed_store, &f.seed).await;
    assert!(c.pull_only_round(&f.peer_store, &f.peer).await.unwrap());
    assert_eq!(
        title(&f.peer, remote.id.as_str()).await,
        "read while upload blocked"
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        2
    );
}

#[tokio::test]
async fn recurring_local_work_is_explicitly_refused_without_partial_push() {
    use aven_core::{
        operations::{CreateRecurrenceSeriesParams, RecurrenceSeriesDraft},
        recurrence::*,
    };
    use chrono::{TimeZone, Utc};
    let f = fixture().await;
    converge(&f).await;
    let w = f.peer.list_workspaces().await.unwrap().remove(0);
    let at = Utc.with_ymd_and_hms(2026, 9, 22, 12, 0, 0).unwrap();
    f.peer
        .create_recurrence_series(
            &w,
            CreateRecurrenceSeriesParams::new(RecurrenceSeriesDraft {
                title: "deferred recurrence".into(),
                description: "".into(),
                project: "app".into(),
                priority: "none".into(),
                initial_status: "todo".into(),
                labels: vec![],
                metadata: vec![],
                schedule: RecurrenceSchedule::new(
                    RecurrenceRule::daily(),
                    "UTC".parse().unwrap(),
                    at.date_naive(),
                    None,
                    RecurrenceDuePolicy::SameDay,
                ),
            })
            .at(at),
        )
        .await
        .unwrap();
    let pending = scalar(
        &f.peer,
        "SELECT count(*) FROM changes WHERE server_seq IS NULL",
    )
    .await;
    assert!(pending > 1);
    assert!(
        Client::new(&f.origin)
            .unwrap()
            .round(&f.peer_store, &f.peer)
            .await
            .is_err()
    );
    assert_eq!(
        scalar(
            &f.peer,
            "SELECT count(*) FROM changes WHERE server_seq IS NULL"
        )
        .await,
        pending
    );
    assert_eq!(
        scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
        1
    );
    assert_eq!(
        scalar(&f.peer, "SELECT count(*) FROM local_e2ee_outbox").await,
        0
    );
}

#[tokio::test]
async fn deletion_conflict_force_and_exact_retry_keep_conservative_parent_protection() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let task = f
        .seed
        .export_data("now".into())
        .await
        .unwrap()
        .tables
        .task_attachments[0]
        .task_id
        .clone();
    f.seed
        .update_task(
            &w,
            &task,
            TaskUpdate {
                deleted: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    f.peer
        .update_task(
            &w,
            &task,
            TaskUpdate {
                deleted: Some(true),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let c = Client::new(&f.origin).unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(&f.server, "SELECT protected FROM server_e2ee_image_parents").await,
        1
    );
    assert_eq!(
        scalar(
            &f.server,
            "SELECT count(*) FROM server_e2ee_images WHERE unreferenced_at IS NOT NULL"
        )
        .await,
        0
    );
    f.seed
        .resolve_conflict(&w, &task, "deleted", "1")
        .await
        .unwrap();
    converge(&f).await;
    assert_eq!(
        scalar(&f.server, "SELECT protected FROM server_e2ee_image_parents").await,
        1
    );
    let inputs = f.seed_store.tail_inputs(&f.seed, &f.origin).await.unwrap();
    let (record, before): (Vec<u8>, i64) = {
        let mut conn = aven_core::test_support::acquire(&f.server).await.unwrap();
        sqlx::query_as(
            "SELECT record,sequence FROM server_e2ee_tail ORDER BY sequence DESC LIMIT 1",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap()
    };
    let Reply::Appended(m) = c
        .exchange(
            &inputs.authority.context,
            &inputs.bearer,
            Operation::Append { record },
        )
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(m.sequence, before);
    assert_eq!(
        scalar(&f.server, "SELECT high_water FROM server_e2ee_allocator").await,
        before
    );
    assert_eq!(
        scalar(&f.server, "SELECT protected FROM server_e2ee_image_parents").await,
        1
    );
}

#[tokio::test]
async fn missing_installation_or_protected_coverage_refuses_before_preparation() {
    for missing_key in [false, true] {
        let f = fixture().await;
        converge(&f).await;
        let w = f.peer.list_workspaces().await.unwrap().remove(0);
        f.peer
            .create_task(&w, draft("preserve local work"))
            .await
            .unwrap();
        if missing_key {
            let record = std::fs::read_dir(f.root.path().join("peer-keys"))
                .unwrap()
                .map(|p| p.unwrap().path())
                .find(|p| p.extension().is_some_and(|s| s == "peer-verified"))
                .unwrap();
            std::fs::remove_file(record).unwrap();
        } else {
            let mut conn = aven_core::test_support::acquire(&f.peer).await.unwrap();
            sqlx::query("DELETE FROM local_peer_snapshot_install")
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        assert!(
            Client::new(&f.origin)
                .unwrap()
                .round(&f.peer_store, &f.peer)
                .await
                .is_err()
        );
        assert_eq!(
            scalar(&f.peer, "SELECT count(*) FROM local_e2ee_outbox").await,
            0
        );
        assert_eq!(
            scalar(
                &f.peer,
                "SELECT count(*) FROM changes WHERE server_seq IS NULL"
            )
            .await,
            1
        );
        assert_eq!(
            scalar(&f.server, "SELECT count(*) FROM server_e2ee_tail").await,
            1
        );
    }
}

#[tokio::test]
async fn note_and_relationship_removals_use_existing_domain_operations() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    let a = f.seed.create_task(&w, draft("parent")).await.unwrap().task;
    let b = f.seed.create_task(&w, draft("child")).await.unwrap().task;
    f.seed.create_label(&w, "tag").await.unwrap();
    f.seed
        .update_task(
            &w,
            &b.id,
            TaskUpdate {
                add_labels: vec!["tag".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let note = f
        .seed
        .add_note(&w, &b.id, "initial note".into())
        .await
        .unwrap();
    f.seed.add_task_to_epic(&w, &b.id, &a.id).await.unwrap();
    f.seed
        .add_task_related_link(&w, &a.id, &b.id)
        .await
        .unwrap();
    f.seed.add_task_dependency(&w, &a.id, &b.id).await.unwrap();
    converge(&f).await;
    f.peer
        .edit_note(&w, &b.id, &note.note_id, "edited note".into())
        .await
        .unwrap();
    converge(&f).await;
    f.peer.delete_note(&w, &b.id, &note.note_id).await.unwrap();
    f.peer
        .remove_task_from_epic(&w, &b.id, &a.id)
        .await
        .unwrap();
    f.peer
        .remove_task_related_link(&w, &a.id, &b.id)
        .await
        .unwrap();
    f.peer
        .remove_task_dependency(&w, &a.id, &b.id)
        .await
        .unwrap();
    f.peer
        .update_task(
            &w,
            &b.id,
            TaskUpdate {
                remove_labels: vec!["tag".into()],
                ..Default::default()
            },
        )
        .await
        .unwrap();
    converge(&f).await;
    for db in [&f.seed, &f.peer] {
        assert_eq!(scalar(db, "SELECT count(*) FROM task_epic_links").await, 0);
        assert_eq!(
            scalar(db, "SELECT count(*) FROM task_related_links WHERE linked=1").await,
            0
        );
        assert_eq!(
            scalar(db, "SELECT count(*) FROM task_dependencies").await,
            0
        );
        assert_eq!(scalar(db, "SELECT count(*) FROM task_labels").await, 0);
    }
}

#[tokio::test]
async fn bounded_http_pull_keeps_watermark_and_makes_byte_limited_progress() {
    let f = fixture().await;
    converge(&f).await;
    let w = f.seed.list_workspaces().await.unwrap().remove(0);
    for i in 0..20 {
        let mut d = draft(&format!("large {i}"));
        d.description = "x".repeat(65000);
        f.seed.create_task(&w, d).await.unwrap();
    }
    let c = Client::new(&f.origin).unwrap();
    drain(&c, &f.seed_store, &f.seed).await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let a = &inputs.authority;
    let mut after = f.peer.encrypted_tail_cursor(a).await.unwrap();
    let mut watermark = None;
    let mut total = 0;
    let mut pages = 0;
    loop {
        let Reply::Page(page) = c
            .exchange(
                &a.context,
                &inputs.bearer,
                Operation::Pull {
                    after,
                    limit: 16,
                    watermark,
                },
            )
            .await
            .unwrap()
        else {
            panic!()
        };
        assert!(page.records.len() <= 16);
        assert!(page.records.iter().map(|r| r.record.len()).sum::<usize>() <= tail::PAGE_BYTES);
        if pages == 0 {
            assert!(page.has_more);
            assert!(
                page.records.len() < 16,
                "byte limit must shorten the first page"
            );
        }
        if let Some(mark) = watermark {
            assert_eq!(page.watermark, mark)
        } else {
            watermark = Some(page.watermark)
        }
        f.peer.apply_encrypted_tail_page(a, &page).await.unwrap();
        total += page.records.len();
        pages += 1;
        if !page.has_more {
            break;
        }
        assert!(page.cursor > after);
        after = page.cursor;
    }
    assert_eq!(total, 20);
    assert_eq!(pages, 2);
}

async fn spawn_server(f: &Fixture) -> tokio::process::Child {
    let ready = f.root.path().join("tail-server-ready");
    let _ = std::fs::remove_file(&ready);
    let child = tokio::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "encrypted_tail_http::tests::server_worker",
            "--ignored",
            "--nocapture",
        ])
        .env("AVEN_TAIL_ROOT", f.root.path())
        .env("AVEN_TAIL_ORIGIN", &f.origin)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        while !ready.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    child
}
#[tokio::test]
#[ignore = "subprocess encrypted tail server"]
async fn server_worker() {
    let root = std::path::PathBuf::from(std::env::var_os("AVEN_TAIL_ROOT").unwrap());
    let origin = std::env::var("AVEN_TAIL_ORIGIN").unwrap();
    let db = Database::open(&root.join("server.sqlite")).await.unwrap();
    let (_, task) = serve(db, origin.strip_prefix("http://").unwrap()).await;
    std::fs::write(root.join("tail-server-ready"), b"ready").unwrap();
    task.await.unwrap();
}

#[tokio::test]
async fn stalled_http_bodies_time_out_and_release_both_admission_permits() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_response(stream: &mut tokio::net::TcpStream) -> String {
        let mut bytes = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            stream.read_to_end(&mut bytes),
        )
        .await
        .unwrap()
        .unwrap();
        let response = String::from_utf8(bytes).unwrap();
        assert!(
            response
                .to_ascii_lowercase()
                .contains("cache-control: no-store\r\n")
        );
        response
    }

    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let server = Arc::new(Server {
        db,
        gate: tokio::sync::Semaphore::new(2),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route(PATH, post(handle))
        .with_state(server.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // The syntactically valid bearer has no admitted authority. Neither body
    // supplies even a JSON byte, so admission must be bounded before authentication.
    let headers = format!(
        "POST {PATH} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAuthorization: Bearer {}\r\nContent-Length: 100\r\nConnection: close\r\n\r\n",
        "0".repeat(64)
    );
    let mut first = tokio::net::TcpStream::connect(address).await.unwrap();
    let mut second = tokio::net::TcpStream::connect(address).await.unwrap();
    first.write_all(headers.as_bytes()).await.unwrap();
    second.write_all(headers.as_bytes()).await.unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while server.gate.available_permits() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();

    let complete = headers.replace("Content-Length: 100", "Content-Length: 2") + "{}";
    let mut busy = tokio::net::TcpStream::connect(address).await.unwrap();
    busy.write_all(complete.as_bytes()).await.unwrap();
    let response = read_response(&mut busy).await;
    assert!(response.starts_with("HTTP/1.1 503"));
    assert!(response.ends_with("encrypted_tail_busy"));
    assert_eq!(server.gate.available_permits(), 0);

    // Advance the server clock rather than waiting for a client-side timeout.
    tokio::time::pause();
    tokio::time::advance(REQUEST_TIMEOUT).await;
    tokio::time::resume();
    for stream in [&mut first, &mut second] {
        let response = read_response(stream).await;
        assert!(response.starts_with("HTTP/1.1 408"));
        assert!(response.ends_with("encrypted_tail_timeout"));
        assert!(!response.contains(&"0".repeat(64)));
    }
    assert_eq!(server.gate.available_permits(), 2);
    let mut admitted = tokio::net::TcpStream::connect(address).await.unwrap();
    admitted.write_all(complete.as_bytes()).await.unwrap();
    let response = read_response(&mut admitted).await;
    assert!(response.starts_with("HTTP/1.1 409"));
    assert!(response.ends_with("encrypted_tail_refused"));
    assert_eq!(server.gate.available_permits(), 2);
    task.abort();
    let _ = task.await;
}

#[tokio::test]
async fn successful_tail_response_is_not_cacheable() {
    let f = fixture().await;
    let inputs = f.peer_store.tail_inputs(&f.peer, &f.origin).await.unwrap();
    let request = Request::builder()
        .method("POST")
        .uri(PATH)
        .header(header::CONTENT_TYPE, "application/json")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", hex::encode(inputs.bearer.expose())),
        )
        .body(axum::body::Body::from(
            serde_json::to_vec(&Envelope {
                context: inputs.authority.context.clone(),
                correlation: [0; 32],
                operation: Operation::Pull {
                    after: inputs.authority.prefix,
                    limit: 1,
                    watermark: None,
                },
            })
            .unwrap(),
        ))
        .unwrap();
    let response = handle(
        State(Arc::new(Server {
            db: f.server.clone(),
            gate: tokio::sync::Semaphore::new(2),
        })),
        request,
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
}
