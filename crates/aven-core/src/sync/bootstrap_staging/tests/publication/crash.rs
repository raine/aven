use super::*;

#[tokio::test]
async fn committed_publication_survives_process_exit_without_response() {
    let f = Fixture::new().await;
    let p = f.publication();
    f.declare().await;
    f.upload().await;
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
    crate::test_support::worker::run(
        "sync::bootstrap_staging::tests::publication::crash::publication_exit_worker",
        &f.dir.path(),
    );
    let recovered = f.publish(&p).await.unwrap();
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
    let Some(root) = crate::test_support::worker::args::<std::path::PathBuf>() else {
        return;
    };
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
                record: &record,
            },
            Default::default(),
        )
        .await
        .unwrap();
    crate::test_support::worker::exit();
}
