use super::*;

#[tokio::test]
async fn incomplete_reordered_and_corrupt_catalogs_use_quarantine_and_recover_exact_bytes() {
    let f = Fixture::build(true).await;
    let s = f.declare().await;
    let slices = f.package.catalogs[1].chunks(1_048_576).collect::<Vec<_>>();
    assert_eq!(slices.len(), 2);
    f.server
        .put_bootstrap_chunk(
            &f.auth(),
            f.request(s.epoch, Component::DataCatalog, 0, &f.package.catalogs[0]),
        )
        .await
        .unwrap();
    f.server
        .put_bootstrap_chunk(
            &f.auth(),
            f.request(s.epoch, Component::State, 0, &f.package.state[0]),
        )
        .await
        .unwrap();
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::PrefixCatalog, 1, slices[1])
            )
            .await
            .unwrap(),
        PutOutcome::Quarantined
    );
    assert_eq!(
        presence(&f.status().await, Component::PrefixCatalog),
        [Presence::Missing, Presence::Quarantined]
    );
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::PrefixCatalog, 1, slices[1])
            )
            .await
            .unwrap(),
        PutOutcome::Quarantined
    );
    let mut bad = slices[1].to_vec();
    bad[0] ^= 1;
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::PrefixCatalog, 1, &bad)
            )
            .await
            .is_err()
    );
    let mut corrupt_image_catalog = f.package.catalogs[2].clone();
    corrupt_image_catalog[0] ^= 1;
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::ImageCatalog, 0, &corrupt_image_catalog)
            )
            .await
            .unwrap(),
        PutOutcome::CatalogRejected
    );
    let fenced = f.status().await;
    assert_eq!(
        presence(&fenced, Component::PrefixCatalog),
        [Presence::Missing, Presence::Missing]
    );
    assert_eq!(presence(&fenced, Component::State)[0], Presence::Verified);
    let s = f
        .server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::PrefixCatalog, 1, slices[1])
            )
            .await
            .unwrap(),
        PutOutcome::Quarantined
    );
    // Network arrival order is irrelevant when indexes and exact bytes agree.
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::PrefixCatalog, 0, slices[0])
            )
            .await
            .unwrap(),
        PutOutcome::Verified
    );
    assert_eq!(
        presence(&f.status().await, Component::PrefixCatalog),
        [Presence::Verified, Presence::Verified]
    );
    let mut bad = f.package.catalogs[2].clone();
    bad[0] ^= 1;
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::ImageCatalog, 0, &bad)
            )
            .await
            .unwrap(),
        PutOutcome::CatalogRejected
    );
    let rejected = f.status().await;
    assert_eq!(
        rejected.catalog_failure,
        Some(CatalogFailure {
            component: Component::ImageCatalog,
            reason: CatalogFailureReason::Invalid
        })
    );
    assert!(rejected.epoch > s.epoch);
    assert_eq!(presence(&rejected, Component::State)[0], Presence::Verified);
    assert_eq!(
        presence(&rejected, Component::ImageCatalog),
        [Presence::Missing]
    );
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::ImageCatalog, 0, &f.package.catalogs[2])
            )
            .await
            .is_err()
    );
    let resumed = f
        .server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    f.upload(resumed.epoch).await;
    assert_eq!(f.status().await.catalog_failure, None);

    // Reclaimed bytes can return, but a slice relabeled with another index cannot.
    f.server
        .reclaim_bootstrap_staging(&f.auth(), f.id, f.commitment(), resumed.epoch, Reclaim::All)
        .await
        .unwrap();
    let resumed = f
        .server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(resumed.epoch, Component::PrefixCatalog, 0, slices[1])
            )
            .await
            .is_err()
    );
    let mut wrong_order = slices[0].to_vec();
    wrong_order.reverse();
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(resumed.epoch, Component::PrefixCatalog, 0, &wrong_order)
            )
            .await
            .unwrap(),
        PutOutcome::Quarantined
    );
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(resumed.epoch, Component::PrefixCatalog, 1, slices[1])
            )
            .await
            .unwrap(),
        PutOutcome::CatalogRejected
    );
}

#[tokio::test]
async fn committed_but_structurally_invalid_catalog_and_corrupt_data_fail_closed() {
    let mut f = Fixture::new().await;
    // Bind a real catalog with a broken rank to the descriptor. Hash equality
    // alone must not turn malformed prefix coverage into verified presence.
    f.package.catalogs[1][23..31].copy_from_slice(&2_u64.to_be_bytes());
    let digest: [u8; 32] = Sha256::digest(&f.package.catalogs[1]).into();
    let hash_offset = 175 + 57 + 25;
    f.package.descriptor[hash_offset..hash_offset + 32].copy_from_slice(&digest);
    let s = f.declare().await;
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::PrefixCatalog, 0, &f.package.catalogs[1])
            )
            .await
            .unwrap(),
        PutOutcome::CatalogRejected
    );

    let f = Fixture::new().await;
    let s = f.declare().await;
    f.server
        .put_bootstrap_chunk(
            &f.auth(),
            f.request(s.epoch, Component::DataCatalog, 0, &f.package.catalogs[0]),
        )
        .await
        .unwrap();
    let mut bad = f.package.state[0].clone();
    let end = bad.len() - 1;
    bad[end] ^= 1;
    assert!(
        f.server
            .put_bootstrap_chunk(&f.auth(), f.request(s.epoch, Component::State, 0, &bad))
            .await
            .is_err()
    );
    assert_eq!(
        presence(&f.status().await, Component::State),
        [Presence::Missing]
    );
}

#[tokio::test]
async fn storage_failures_roll_back_declaration_upload_verification_and_cancellation() {
    let f = Fixture::new().await;
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("CREATE TRIGGER fail_declare BEFORE INSERT ON server_bootstrap_candidates BEGIN SELECT RAISE(ABORT, 'injected'); END")
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        f.server
            .declare_bootstrap_staging(&f.auth(), &f.package.descriptor, f.budget())
            .await
            .is_err()
    );
    assert_eq!(
        f.server
            .bootstrap_staging_status(&f.auth(), f.id)
            .await
            .unwrap(),
        Status::Missing
    );
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("DROP TRIGGER fail_declare")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_verify BEFORE UPDATE OF verified ON server_bootstrap_chunks BEGIN SELECT RAISE(ABORT, 'injected'); END")
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    let s = f.declare().await;
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::DataCatalog, 0, &f.package.catalogs[0])
            )
            .await
            .is_err()
    );
    assert_eq!(f.status().await, s);
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("DROP TRIGGER fail_verify")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("CREATE TRIGGER fail_put BEFORE INSERT ON server_bootstrap_chunks BEGIN SELECT RAISE(ABORT, 'injected'); END")
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::Manifest, 0, &f.package.manifest[0])
            )
            .await
            .is_err()
    );
    assert_eq!(f.status().await, s);
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("DROP TRIGGER fail_put")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    f.upload(s.epoch).await;
    let before = f.status().await;
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("CREATE TRIGGER fail_cancel BEFORE UPDATE ON server_bootstrap_candidates BEGIN SELECT RAISE(ABORT, 'injected'); END")
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        f.server
            .cancel_bootstrap_staging(&f.auth(), f.id)
            .await
            .is_err()
    );
    assert!(
        f.server
            .reclaim_bootstrap_staging(&f.auth(), f.id, f.commitment(), s.epoch, Reclaim::All)
            .await
            .is_err()
    );
    assert_eq!(f.status().await, before);
    f.upload(s.epoch).await;
}

#[tokio::test]
async fn declaration_request_storage_and_terminal_metadata_are_bounded() {
    let f = Fixture::new().await;
    for budget in [
        Budget {
            bytes: u64::MAX,
            chunks: 1,
        },
        Budget {
            bytes: 1,
            chunks: u64::MAX,
        },
        Budget {
            bytes: 1,
            chunks: 1,
        },
    ] {
        assert!(
            f.server
                .declare_bootstrap_staging(&f.auth(), &f.package.descriptor, budget)
                .await
                .is_err()
        );
    }
    let too_large = vec![0; 1025];
    assert!(
        f.server
            .declare_bootstrap_staging(&f.auth(), &too_large, f.budget())
            .await
            .is_err()
    );
    let mut huge_catalog = f.package.descriptor.clone();
    huge_catalog[184..192].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(
        f.server
            .declare_bootstrap_staging(&f.auth(), &huge_catalog, f.budget())
            .await
            .is_err()
    );
    let mut budget = f.budget();
    budget.bytes -= 1;
    let s = f
        .server
        .declare_bootstrap_staging(&f.auth(), &f.package.descriptor, budget)
        .await
        .unwrap();
    assert!(
        f.server
            .declare_bootstrap_staging(&f.auth(), &f.package.descriptor, f.budget())
            .await
            .is_err()
    );
    let bytes = vec![0; MAX_REQUEST_BYTES + 1];
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::Manifest, 0, &bytes)
            )
            .await
            .is_err()
    );
    f.server
        .put_bootstrap_chunk(
            &f.auth(),
            f.request(s.epoch, Component::DataCatalog, 0, &f.package.catalogs[0]),
        )
        .await
        .unwrap();
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::ImageCatalog, 0, &f.package.catalogs[2])
            )
            .await
            .unwrap(),
        PutOutcome::CatalogRejected
    );
    f.server
        .cancel_bootstrap_staging(&f.auth(), f.id)
        .await
        .unwrap();
    for i in 1..MAX_CANDIDATES {
        let mut id = [0; 32];
        id[..8].copy_from_slice(&i.to_be_bytes());
        f.server
            .cancel_bootstrap_staging(&f.auth(), id)
            .await
            .unwrap();
    }
    assert!(
        f.server
            .cancel_bootstrap_staging(&f.auth(), [255; 32])
            .await
            .is_err()
    );
    // The cap must not prevent exact terminal retries or cancellation of an
    // already allocated candidate.
    f.server
        .cancel_bootstrap_staging(&f.auth(), f.id)
        .await
        .unwrap();
}

#[tokio::test]
async fn clear_server_database_and_diagnostics_exclude_private_material() {
    let f = Fixture::new().await;
    let s = f.declare().await;
    f.upload(s.epoch).await;
    let mut secrets = vec![
        b"PRIVATE-STAGING-TASK-TITLE".to_vec(),
        b"PRIVATE-STAGING-DESCRIPTION".to_vec(),
        b"PRIVATE-STAGING-FILENAME.png".to_vec(),
        b"PRIVATE-STAGING-ALT-TEXT".to_vec(),
        f.image_hash.as_bytes().to_vec(),
        hex::decode(&f.image_hash).unwrap(),
        f.key.protected_storage_bytes().to_vec(),
    ];
    let protected = f.seed.protected_storage_bytes();
    secrets.extend(protected[..96].chunks(32).map(<[u8]>::to_vec));
    let mut conn = f.source.acquire_reader().await.unwrap();
    let provenance = crate::db::get_meta(&mut conn, "client_id")
        .await
        .unwrap()
        .unwrap();
    secrets.push(provenance.into_bytes());
    drop(conn);
    let mut observed = format!("{:?} {:?} {:?}", f.auth(), f.status().await, f.seed).into_bytes();
    for suffix in ["", "-wal", "-shm"] {
        let path = f.dir.path().join(format!("server.sqlite{suffix}"));
        if path.exists() {
            observed.extend(std::fs::read(path).unwrap());
        }
    }
    for secret in secrets {
        assert!(!observed.windows(secret.len()).any(|bytes| bytes == secret));
    }
}

#[tokio::test]
async fn keyless_staging_does_not_establish_arbitrary_domain_validity() {
    let mut f = Fixture::new().await;
    let original = f.package.clone();
    let byte = &mut f.package.catalogs[1][39];
    *byte = if *byte == b'0' { b'1' } else { b'0' };
    let digest: [u8; 32] = Sha256::digest(&f.package.catalogs[1]).into();
    f.package.descriptor[257..289].copy_from_slice(&digest);
    let s = f.declare().await;
    f.upload(s.epoch).await;
    bootstrap_format::validate_keyless(&f.package).unwrap();
    let local = f
        .source
        .package_local_shared_state_never_dispatched(
            f.dir.path(),
            f.seed.genesis().context(),
            &f.key,
            f.seed.genesis().commitment(),
        )
        .await
        .unwrap();
    assert!(local.upload_package() == original);
    assert!(
        bootstrap_format::authenticate(
            &f.package,
            &f.key,
            f.seed.genesis().context(),
            *local.stream_id(),
            f.id,
            f.seed.genesis().commitment()
        )
        .is_err()
    );
}

#[tokio::test]
async fn ensure_revalidates_retained_bytes_before_renewing_reservations() {
    let f = Fixture::new().await;
    let s = f.declare().await;
    f.upload(s.epoch).await;
    let mut corrupt = f.package.manifest[0].clone();
    let last = corrupt.len() - 1;
    corrupt[last] ^= 1;
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("UPDATE server_bootstrap_chunks SET bytes = ? WHERE component = ?")
        .bind(corrupt)
        .bind(Component::Manifest.key())
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("UPDATE server_bootstrap_candidates SET expires_at = 1")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(
        f.server
            .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
            .await
            .is_err()
    );
    assert_eq!(f.status().await.expires_at, 1);
    f.server
        .reclaim_bootstrap_staging(&f.auth(), f.id, f.commitment(), s.epoch, Reclaim::All)
        .await
        .unwrap();
    let resumed = f
        .server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    f.upload(resumed.epoch).await;
}

#[tokio::test]
async fn artifact_aggregate_failure_rolls_back_the_final_chunk() {
    let mut f = Fixture::new().await;
    f.package.catalogs[0][32] ^= 1;
    let digest: [u8; 32] = Sha256::digest(&f.package.catalogs[0]).into();
    f.package.descriptor[200..232].copy_from_slice(&digest);
    let s = f.declare().await;
    assert_eq!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::DataCatalog, 0, &f.package.catalogs[0])
            )
            .await
            .unwrap(),
        PutOutcome::Verified
    );
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::State, 0, &f.package.state[0])
            )
            .await
            .is_err()
    );
    assert_eq!(
        presence(&f.status().await, Component::State),
        [Presence::Missing]
    );
}
