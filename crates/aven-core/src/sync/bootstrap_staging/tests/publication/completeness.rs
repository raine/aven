use super::*;

#[tokio::test]
async fn publication_rejects_every_missing_obligation_and_corrupt_stored_bytes() {
    let f = Fixture::new().await;
    let p = f.publication();
    let s = f.declare().await;
    assert!(f.publish(&p, s.epoch).await.is_err());
    f.unpublished().await;
    f.upload(s.epoch).await;
    for (component, records) in f.components() {
        for (index, original) in records.into_iter().enumerate() {
            let mut conn = f.server.acquire_writer().await.unwrap();
            sqlx::query(
                "DELETE FROM server_bootstrap_chunks WHERE component = ? AND chunk_index = ?",
            )
            .bind(component.key())
            .bind(index as i64)
            .execute(&mut *conn)
            .await
            .unwrap();
            drop(conn);
            assert!(
                f.publish(&p, s.epoch).await.is_err(),
                "missing {component:?}/{index}"
            );
            f.unpublished().await;
            let mut conn = f.server.acquire_writer().await.unwrap();
            let mut bad = original.to_vec();
            bad[0] ^= 1;
            sqlx::query("INSERT INTO server_bootstrap_chunks(bootstrap, component, chunk_index, bytes) VALUES (?, ?, ?, ?)")
                .bind(f.id.as_slice()).bind(component.key()).bind(index as i64).bind(bad).execute(&mut *conn).await.unwrap();
            drop(conn);
            assert!(
                f.publish(&p, s.epoch).await.is_err(),
                "corrupt {component:?}/{index}"
            );
            f.unpublished().await;
            let mut conn = f.server.acquire_writer().await.unwrap();
            sqlx::query("UPDATE server_bootstrap_chunks SET bytes = ? WHERE component = ? AND chunk_index = ?")
                .bind(original).bind(component.key()).bind(index as i64).execute(&mut *conn).await.unwrap();
        }
    }
    f.publish(&p, s.epoch).await.unwrap();
}

#[tokio::test]
async fn every_publication_write_failure_and_quota_refusal_rolls_back_all_effects() {
    let f = Fixture::new().await;
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
    assert!(
        f.server
            .publish_bootstrap(
                &f.auth(),
                f.publish_request(&p, s.epoch),
                PublicationPolicy {
                    workspace_quota_bytes: 0
                }
            )
            .await
            .is_err()
    );
    f.unpublished().await;
    for (table, action) in [
        ("server_bootstrap_publication", "INSERT"),
        ("server_bootstrap_prefix", "INSERT"),
        ("server_e2ee_image_parents", "INSERT"),
        ("server_e2ee_images", "INSERT"),
        ("server_e2ee_image_chunks", "INSERT"),
        ("server_bootstrap_chunks", "DELETE"),
        ("server_e2ee_image_references", "INSERT"),
        ("server_e2ee_images", "UPDATE"),
        ("server_e2ee_allocator", "INSERT"),
        ("server_e2ee_membership_head", "INSERT"),
        ("server_seed_claim", "UPDATE"),
        ("server_bootstrap_candidates", "UPDATE"),
    ] {
        let f = Fixture::new().await;
        let p = f.publication();
        let s = f.declare().await;
        f.upload(s.epoch).await;
        let mut conn = f.server.acquire_writer().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TRIGGER fail_publication BEFORE {action} ON {table} BEGIN SELECT RAISE(ABORT, 'injected'); END")))
            .persistent(false).execute(&mut *conn).await.unwrap_or_else(|error| panic!("create {table}/{action}: {error}"));
        drop(conn);
        let error = f.publish(&p, s.epoch).await.unwrap_err();
        assert!(
            error.to_string().contains("injected"),
            "{table}/{action}: {error}"
        );
        f.unpublished().await;
        assert_eq!(f.status().await.expires_at, s.expires_at);
        let mut conn = f.server.acquire_writer().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_bootstrap_chunks")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count as u64, f.budget().chunks);
    }
    f.publish(&p, s.epoch).await.unwrap();
}

#[tokio::test]
async fn empty_prefix_initializes_zero_allocator_and_signing_requires_authenticated_package() {
    let dir = tempfile::tempdir().unwrap();
    let source = Database::open(&dir.path().join("source.sqlite"))
        .await
        .unwrap();
    let context = LocalSharedStatePackageContext {
        vault_id: [1; 32],
        generation_id: [2; 32],
    };
    let key = LocalSharedStatePackageKey::new([3; 32]);
    let seed = SeedAuthority::generate(context, &key, [4; 32]).unwrap();
    source
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let local = source
        .package_local_shared_state_never_dispatched(
            dir.path(),
            context,
            &key,
            seed.genesis().commitment(),
        )
        .await
        .unwrap();
    let id = hex::decode(local.candidate_id())
        .unwrap()
        .try_into()
        .unwrap();
    let package = local.upload_package();
    assert_eq!(
        bootstrap_format::validate_keyless(&package)
            .unwrap()
            .prefix_count,
        0
    );
    assert!(
        seed.prepare_bootstrap_publication(&package, &LocalSharedStatePackageKey::new([5; 32]))
            .is_err()
    );
    for component in 0..3 {
        let mut bad = package.clone();
        match component {
            0 => bad.descriptor[39] ^= 1,
            1 => {
                let last = bad.manifest[0].len() - 1;
                bad.manifest[0][last] ^= 1;
            }
            _ => bad.catalogs[1][0] ^= 1,
        }
        assert!(seed.prepare_bootstrap_publication(&bad, &key).is_err());
    }
    let server = Database::open(&dir.path().join("server.sqlite"))
        .await
        .unwrap();
    let secret = Secret::new([6; 32]);
    let setup = SetupAuthority::from_verifier([4; 32], SetupAuthority::verifier([4; 32], &secret));
    server
        .admit_seed_claim(
            &seed.genesis().claim_bytes(),
            Some(&setup),
            ClaimAuthentication::SetupSecret(&secret),
        )
        .await
        .unwrap();
    let f = Fixture {
        dir,
        source,
        server,
        seed,
        key,
        package,
        id,
        image_hash: String::new(),
    };
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
    f.publish(&p, s.epoch).await.unwrap();
    let mut conn = f.server.acquire_reader().await.unwrap();
    let (n, high): (i64, i64) =
        sqlx::query_as("SELECT prefix_count, high_water FROM server_e2ee_allocator")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert_eq!((n, high, high.checked_add(1).unwrap()), (0, 0, 1));
    let images: i64 = sqlx::query_scalar("SELECT count(*) FROM server_e2ee_images")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(images, 0);
}

#[tokio::test]
async fn published_database_wal_and_diagnostics_contain_no_domain_plaintext_or_secrets() {
    let f = Fixture::new().await;
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
    let outcome = f.publish(&p, s.epoch).await.unwrap();
    let status = f
        .server
        .bootstrap_staging_status(&f.auth(), f.id)
        .await
        .unwrap();
    let mut observed =
        format!("{:?} {:?} {:?} {:?}", f.auth(), outcome, status, f.seed).into_bytes();
    for suffix in ["", "-wal", "-shm"] {
        let path = f.dir.path().join(format!("server.sqlite{suffix}"));
        if path.exists() {
            observed.extend(std::fs::read(path).unwrap());
        }
    }
    let mut secrets = vec![
        b"PRIVATE-STAGING-TASK-TITLE".to_vec(),
        b"PRIVATE-STAGING-DESCRIPTION".to_vec(),
        b"PRIVATE-STAGING-FILENAME.png".to_vec(),
        b"PRIVATE-STAGING-ALT-TEXT".to_vec(),
        f.image_hash.as_bytes().to_vec(),
        hex::decode(&f.image_hash).unwrap(),
        f.key.protected_storage_bytes().to_vec(),
    ];
    secrets.extend(
        f.seed.protected_storage_bytes()[..96]
            .chunks(32)
            .map(<[u8]>::to_vec),
    );
    for secret in secrets {
        assert!(!observed.windows(secret.len()).any(|bytes| bytes == secret));
    }
}
