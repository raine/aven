use super::*;
use crate::db::Database;

fn fixture(name: &str) -> Vec<u8> {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/genesis.json")).unwrap();
    hex::decode(json[name].as_str().unwrap()).unwrap()
}

fn array(name: &str) -> [u8; 32] {
    fixture(name).try_into().unwrap()
}

fn context() -> LocalSharedStatePackageContext {
    LocalSharedStatePackageContext {
        vault_id: array("vault"),
        generation_id: array("generation"),
    }
}

fn key() -> LocalSharedStatePackageKey {
    LocalSharedStatePackageKey::new(array("generation_secret"))
}

fn authority() -> SeedAuthority {
    let mut bytes = Vec::new();
    for name in ["signing_seed", "hpke_private", "token", "record"] {
        bytes.extend(fixture(name));
    }
    SeedAuthority::from_protected_storage(&bytes, context(), &key()).unwrap()
}

fn operator_secret() -> Secret {
    Secret::new([0x91; 32])
}

/// Issues the fixture genesis's setup ID to `db` for [`operator_secret`].
async fn operator(db: &Database) -> Secret {
    let secret = operator_secret();
    db.issue_e2ee_server_setup(&secret, array("setup"), u64::MAX)
        .await
        .unwrap();
    secret
}

fn resign(core: &[u8], state: &[u8], attachments: &[u8]) -> Vec<u8> {
    let mut core = core.to_vec();
    core[197..229].copy_from_slice(&hash(&cce("aven-e2ee/v1/membership/state", &[state])));
    let signature = SigningKey::from_bytes(&array("signing_seed"))
        .sign(&cce("aven-e2ee/v1/membership/sign", &[&core, attachments]));
    let mut record = vec![1];
    for part in [&core[..], state, attachments, &signature.to_bytes()] {
        bytes(&mut record, part);
    }
    record
}

#[tokio::test]
async fn setup_authorization_resume_restart_and_divergent_bindings() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("server.sqlite");
    let db = Database::open(&path).await.unwrap();
    let seed = authority();
    let request = seed.genesis().claim_bytes();
    let secret = operator(&db).await;
    for authentication in [
        ClaimAuthentication::SeedBearer(seed.bearer()),
        ClaimAuthentication::SetupSecret(&Secret::new([0; 32])),
    ] {
        assert!(db.admit_seed_claim(&request, authentication).await.is_err());
    }
    // A verifier issued for another setup ID refuses the same secret.
    let other = Database::open(&root.path().join("other.sqlite"))
        .await
        .unwrap();
    other
        .issue_e2ee_server_setup(&secret, [3; 32], u64::MAX)
        .await
        .unwrap();
    assert!(
        other
            .admit_seed_claim(&request, ClaimAuthentication::SetupSecret(&secret))
            .await
            .is_err()
    );
    let result = db
        .admit_seed_claim(&request, ClaimAuthentication::SetupSecret(&secret))
        .await
        .unwrap();
    result.validate_pinned(seed.genesis()).unwrap();
    drop(db);
    // The prior response could have been lost. Reopen and resume.
    let db = Database::open(&path).await.unwrap();
    assert_eq!(
        result,
        db.admit_seed_claim(&request, ClaimAuthentication::SeedBearer(seed.bearer()))
            .await
            .unwrap()
    );
    assert_eq!(
        result,
        db.admit_seed_claim(&request, ClaimAuthentication::SetupSecret(&secret))
            .await
            .unwrap()
    );
    assert!(
        db.admit_seed_claim(
            &request,
            ClaimAuthentication::SetupSecret(&Secret::new([0; 32]))
        )
        .await
        .is_err()
    );
    assert!(
        db.admit_seed_claim(
            &request,
            ClaimAuthentication::SeedBearer(&Secret::new([0; 32]))
        )
        .await
        .is_err()
    );

    let (core, state, att, _) = codec::components(seed.genesis().record()).unwrap();
    for field in ["claim", "verifier", "ciphertext", "setup"] {
        let mut c = core.to_vec();
        let mut s = state.to_vec();
        let mut a = att.to_vec();
        match field {
            "claim" => {
                c[161] ^= 1;
                s[175] ^= 1;
            }
            "verifier" => s[139] ^= 1,
            "ciphertext" => a[121] ^= 1,
            "setup" => c[129] ^= 1,
            _ => unreachable!(),
        }
        let divergent = Genesis::from_record(&resign(&c, &s, &a)).unwrap();
        let error = db
            .admit_seed_claim(
                &divergent.claim_bytes(),
                ClaimAuthentication::SetupSecret(&secret),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("conflict"), "{field}: {error}");
    }
    let other = SeedAuthority::generate(context(), &key(), array("setup")).unwrap();
    assert!(
        db.admit_seed_claim(
            &other.genesis().claim_bytes(),
            ClaimAuthentication::SeedBearer(other.bearer())
        )
        .await
        .is_err()
    );
    assert_eq!(
        result,
        db.admit_seed_claim(&request, ClaimAuthentication::SeedBearer(seed.bearer()))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn two_connections_compete_and_failed_insert_rolls_back() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("server.sqlite");
    let db = Database::open(&path).await.unwrap();
    let seed = authority();
    let secret = operator(&db).await;
    let request = seed.genesis().claim_bytes();
    {
        let mut conn = db.acquire_writer().await.unwrap();
        sqlx::query("CREATE TRIGGER fail_claim AFTER INSERT ON server_seed_claim BEGIN SELECT RAISE(ABORT, 'injected claim failure'); END")
            .execute(&mut *conn).await.unwrap();
    }
    assert!(
        db.admit_seed_claim(&request, ClaimAuthentication::SetupSecret(&secret))
            .await
            .is_err()
    );
    {
        let mut conn = db.acquire_writer().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_seed_claim")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count, 0);
        sqlx::query("DROP TRIGGER fail_claim")
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let second = Database::open(&path).await.unwrap();
    let competitor = SeedAuthority::generate(context(), &key(), array("setup")).unwrap();
    let competing_request = competitor.genesis().claim_bytes();
    let (a, b) = tokio::join!(
        db.admit_seed_claim(&request, ClaimAuthentication::SetupSecret(&secret)),
        second.admit_seed_claim(
            &competing_request,
            ClaimAuthentication::SetupSecret(&secret)
        ),
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let (winner, result) = if let Ok(result) = a {
        (&seed, result)
    } else {
        (&competitor, b.unwrap())
    };
    drop(db);
    drop(second);
    let reopened = Database::open(&path).await.unwrap();
    assert_eq!(
        result,
        reopened
            .admit_seed_claim(
                &winner.genesis().claim_bytes(),
                ClaimAuthentication::SeedBearer(winner.bearer())
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn server_storage_exports_and_diagnostics_exclude_secrets() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let seed = authority();
    let setup = operator(&db).await;
    let result = db
        .admit_seed_claim(
            &seed.genesis().claim_bytes(),
            ClaimAuthentication::SetupSecret(&setup),
        )
        .await
        .unwrap();
    let exported =
        serde_json::to_vec(&db.export_data("2026-09-22T00:00:00Z".into()).await.unwrap()).unwrap();
    let debug = format!(
        "{seed:?} {result:?} {:?}",
        ClaimAuthentication::SetupSecret(&setup)
    );
    let mut conn = db.acquire_reader().await.unwrap();
    let stored: Vec<u8> = sqlx::query_scalar("SELECT genesis FROM server_seed_claim")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(stored, seed.genesis().record());
    let mut surfaces = vec![exported, stored, debug.into_bytes()];
    for path in [
        db.path().to_path_buf(),
        std::path::PathBuf::from(format!("{}-wal", db.path().display())),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            surfaces.push(bytes);
        }
    }
    for secret in [
        fixture("signing_seed"),
        fixture("hpke_private"),
        fixture("token"),
        fixture("generation_secret"),
        setup.expose().to_vec(),
    ] {
        for bytes in &surfaces {
            assert!(!bytes.windows(32).any(|window| window == secret));
            assert!(
                !bytes
                    .windows(64)
                    .any(|window| window == hex::encode(&secret).as_bytes())
            );
        }
    }
}

#[tokio::test]
async fn committed_claim_resumes_after_process_exit_without_response() {
    let root = tempfile::tempdir().unwrap();
    crate::test_support::worker::run("sync::seed_claim::tests::claim_exit_worker", &root.path());
    let db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let seed = authority();
    let result = db
        .admit_seed_claim(
            &seed.genesis().claim_bytes(),
            ClaimAuthentication::SeedBearer(seed.bearer()),
        )
        .await
        .unwrap();
    result.validate_pinned(seed.genesis()).unwrap();
}

#[tokio::test]
#[ignore = "subprocess worker exits without destructors; invoked with an isolated test root"]
async fn claim_exit_worker() {
    let Some(root) = crate::test_support::worker::args::<std::path::PathBuf>() else {
        return;
    };
    let db = Database::open(&root.join("server.sqlite")).await.unwrap();
    let secret = operator(&db).await;
    db.admit_seed_claim(
        &authority().genesis().claim_bytes(),
        ClaimAuthentication::SetupSecret(&secret),
    )
    .await
    .unwrap();
    crate::test_support::worker::exit();
}

#[tokio::test]
async fn issued_server_setup_expires_and_refuses_used_storage() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    assert!(!db.is_e2ee_server_storage().await.unwrap());
    let seed = authority();
    let request = seed.genesis().claim_bytes();
    let secret = operator_secret();
    let stale = Secret::new([0x92; 32]);
    let id = db
        .issue_e2ee_server_setup(&stale, array("setup"), 100)
        .await
        .unwrap();
    assert_eq!(id, array("setup"));
    assert!(db.is_e2ee_server_storage().await.unwrap());
    assert!(db.e2ee_server_setup(100).await.unwrap().is_none());
    // Reissue after expiry keeps the ID and refuses the replaced secret.
    let id = db
        .issue_e2ee_server_setup(&secret, [3; 32], 200)
        .await
        .unwrap();
    assert_eq!(id, array("setup"));
    assert!(db.e2ee_server_setup(199).await.unwrap().is_some());
    assert!(
        db.admit_seed_claim_at(&request, ClaimAuthentication::SetupSecret(&stale), 199)
            .await
            .is_err()
    );
    db.admit_seed_claim_at(&request, ClaimAuthentication::SetupSecret(&secret), 199)
        .await
        .unwrap();
    let error = db
        .issue_e2ee_server_setup(&secret, [3; 32], 300)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "error e2ee-server-already-claimed");

    let used = Database::open(&root.path().join("used.sqlite"))
        .await
        .unwrap();
    let workspace = used.list_workspaces().await.unwrap().remove(0);
    used.create_label(&workspace, "history").await.unwrap();
    let error = used
        .issue_e2ee_server_setup(&secret, [3; 32], 300)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "error e2ee-server-storage-not-empty");
}

#[tokio::test]
async fn server_setup_refuses_historyless_domain_data_without_modifying_it() {
    for kind in ["task", "label", "workspace", "renamed-default"] {
        let root = tempfile::tempdir().unwrap();
        let source = Database::open(&root.path().join("source.sqlite"))
            .await
            .unwrap();
        let workspace = source.list_workspaces().await.unwrap().remove(0);
        match kind {
            "task" => {
                source
                    .create_task(
                        &workspace,
                        crate::operations::TaskDraft {
                            title: "private historyless task".into(),
                            description: "private description".into(),
                            project: Some("app".into()),
                            status: "todo".into(),
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
            "label" => {
                source
                    .create_label(&workspace, "private label")
                    .await
                    .unwrap();
            }
            "workspace" => {
                source.create_workspace("private workspace").await.unwrap();
            }
            _ => {
                source
                    .rename_workspace("default", "private workspace")
                    .await
                    .unwrap();
            }
        }
        let mut export = source
            .export_data("2026-10-02T00:00:00Z".into())
            .await
            .unwrap();
        export.tables.changes.clear();
        export.tables.field_versions.clear();
        let target = Database::open(&root.path().join("target.sqlite"))
            .await
            .unwrap();
        target.validate_import_data(&export).await.unwrap();
        target.import_data(&export).await.unwrap();
        let before =
            serde_json::to_value(target.export_data("fixed".into()).await.unwrap()).unwrap();
        let error = target
            .issue_e2ee_server_setup(&operator_secret(), array("setup"), 300)
            .await
            .unwrap_err();
        assert!(error.is::<StorageNotEmpty>(), "{kind}: {error:#}");
        assert!(!target.is_e2ee_server_storage().await.unwrap());
        let after =
            serde_json::to_value(target.export_data("fixed".into()).await.unwrap()).unwrap();
        assert_eq!(before, after, "{kind}");
    }
}

#[tokio::test]
async fn marked_server_storage_refuses_local_domain_contamination() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    operator(&db).await;
    assert!(db.is_e2ee_server_storage().await.unwrap());
    let workspace = db.list_workspaces().await.unwrap().remove(0);
    db.create_label(&workspace, "private label").await.unwrap();
    let error = db.is_e2ee_server_storage().await.unwrap_err();
    assert!(error.is::<StorageNotEmpty>());
}

#[tokio::test]
async fn persisted_setup_is_read_in_the_claim_transaction() {
    let root = tempfile::tempdir().unwrap();
    let seed = authority();
    let request = seed.genesis().claim_bytes();
    let old = operator_secret();
    let new = Secret::new([0x93; 32]);

    // Reissue first: the claim sees only the replacement verifier.
    let db = Database::open(&root.path().join("reissued.sqlite"))
        .await
        .unwrap();
    db.issue_e2ee_server_setup(&old, array("setup"), 200)
        .await
        .unwrap();
    db.issue_e2ee_server_setup(&new, [3; 32], 200)
        .await
        .unwrap();
    let error = db
        .admit_seed_claim_at(&request, ClaimAuthentication::SetupSecret(&old), 100)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "error seed-claim-unauthorized");
    assert!(
        db.admit_seed_claim_at(&request, ClaimAuthentication::SetupSecret(&new), 200)
            .await
            .is_err()
    );
    db.admit_seed_claim_at(&request, ClaimAuthentication::SetupSecret(&new), 100)
        .await
        .unwrap()
        .validate_pinned(seed.genesis())
        .unwrap();

    // Claim first: the claimed storage refuses a later reissue.
    let db = Database::open(&root.path().join("claimed.sqlite"))
        .await
        .unwrap();
    db.issue_e2ee_server_setup(&old, array("setup"), 200)
        .await
        .unwrap();
    db.admit_seed_claim_at(&request, ClaimAuthentication::SetupSecret(&old), 100)
        .await
        .unwrap();
    let error = db
        .issue_e2ee_server_setup(&new, [3; 32], 300)
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "error e2ee-server-already-claimed");
}

#[tokio::test]
async fn expired_setup_is_named_only_for_its_own_secret() {
    let root = tempfile::tempdir().unwrap();
    let seed = authority();
    let request = seed.genesis().claim_bytes();
    let secret = operator_secret();
    let guess = Secret::new([0x93; 32]);
    let db = Database::open(&root.path().join("expired.sqlite"))
        .await
        .unwrap();
    db.issue_e2ee_server_setup(&secret, array("setup"), 200)
        .await
        .unwrap();

    let refusal = |error: anyhow::Error| *error.downcast_ref::<ClaimRefusal>().unwrap();
    let expired = db
        .admit_seed_claim_at(&request, ClaimAuthentication::SetupSecret(&secret), 200)
        .await
        .unwrap_err();
    assert_eq!(refusal(expired), ClaimRefusal::Expired);
    let wrong = db
        .admit_seed_claim_at(&request, ClaimAuthentication::SetupSecret(&guess), 200)
        .await
        .unwrap_err();
    assert_eq!(
        refusal(wrong),
        ClaimRefusal::Unauthorized { claimed: false }
    );
    db.admit_seed_claim_at(&request, ClaimAuthentication::SetupSecret(&secret), 100)
        .await
        .unwrap();
}
