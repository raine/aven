use super::*;

#[tokio::test]
async fn committed_publication_survives_process_exit_without_response() {
    let f = Fixture::new().await;
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
    // Synthetic authority stays in the private temporary test directory, never
    // in server SQLite or an environment variable containing key material.
    std::fs::write(
        f.dir.path().join("seed.fixture"),
        f.seed.protected_storage_bytes(),
    )
    .unwrap();
    std::fs::write(
        f.dir.path().join("descriptor.fixture"),
        &f.package.descriptor,
    )
    .unwrap();
    std::fs::write(f.dir.path().join("publication.fixture"), p.record()).unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "sync::bootstrap_staging::tests::publication::crash::publication_exit_worker",
            "--ignored",
        ])
        .env("AVEN_PUBLICATION_TEST_ROOT", f.dir.path())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(24),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let recovered = f.publish(&p, 0).await.unwrap();
    assert_eq!(recovered.publication(), &p);
    recovered
        .validate_expected(f.seed.genesis(), &f.package.descriptor)
        .unwrap();
    assert_eq!(
        f.server
            .bootstrap_staging_status(&f.auth(), f.id)
            .await
            .unwrap(),
        Status::Published(recovered)
    );
}

#[tokio::test]
#[ignore = "subprocess worker exits without destructors; invoked with an isolated test root"]
async fn publication_exit_worker() {
    let Some(root) = std::env::var_os("AVEN_PUBLICATION_TEST_ROOT") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let seed = SeedAuthority::from_protected_storage(
        &std::fs::read(root.join("seed.fixture")).unwrap(),
        LocalSharedStatePackageContext {
            vault_id: [31; 32],
            generation_id: [42; 32],
        },
        &LocalSharedStatePackageKey::new([53; 32]),
    )
    .unwrap();
    let descriptor = std::fs::read(root.join("descriptor.fixture")).unwrap();
    let record = std::fs::read(root.join("publication.fixture")).unwrap();
    let publication = Publication::from_record(seed.genesis(), &descriptor, &record).unwrap();
    let binding = publication.binding();
    let server = Database::open(&root.join("server.sqlite")).await.unwrap();
    server
        .publish_bootstrap(
            &Authentication {
                vault_id: binding.vault_id,
                genesis_commitment: binding.genesis_commitment,
                bearer: seed.bearer(),
            },
            PublishBootstrap {
                bootstrap_id: binding.bootstrap_id,
                descriptor_commitment: binding.descriptor_commitment,
                epoch: 1,
                record: &record,
            },
            Default::default(),
        )
        .await
        .unwrap();
    std::process::exit(24);
}
