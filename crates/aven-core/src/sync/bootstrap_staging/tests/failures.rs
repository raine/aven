use super::*;

async fn stored_chunks(f: &Fixture) -> Vec<(Vec<u8>, i64, Vec<u8>)> {
    let mut conn = f.server.acquire_reader().await.unwrap();
    sqlx::query_as(
        "SELECT component, chunk_index, bytes FROM server_bootstrap_chunks
         ORDER BY component, chunk_index",
    )
    .fetch_all(&mut *conn)
    .await
    .unwrap()
}

#[tokio::test]
async fn catalog_slices_are_checked_in_their_slot_before_storage() {
    let f = Fixture::build(true).await;
    f.declare().await;
    let slices = f.package.catalogs[1].chunks(1_048_576).collect::<Vec<_>>();
    assert_eq!(slices.len(), 2);
    // Network arrival order is irrelevant when indexes and exact bytes agree.
    for _ in 0..2 {
        f.server
            .put_bootstrap_chunk(&f.auth(), f.request(Component::PrefixCatalog, 1, slices[1]))
            .await
            .unwrap();
    }
    let partial = f.status().await;
    assert_eq!(
        presence(&partial, Component::PrefixCatalog),
        [Presence::Missing, Presence::Verified]
    );
    let stored = stored_chunks(&f).await;
    let mut flipped = slices[0].to_vec();
    flipped[0] ^= 1;
    let mut conflicting = slices[1].to_vec();
    conflicting[0] ^= 1;
    for (component, index, bytes) in [
        (Component::PrefixCatalog, 0, &flipped[..]),
        (Component::PrefixCatalog, 0, &slices[0][1..]),
        (Component::PrefixCatalog, 0, slices[1]),
        (Component::PrefixCatalog, 1, &conflicting[..]),
        (Component::PrefixCatalog, 2, slices[1]),
        (Component::ImageCatalog, 0, slices[0]),
        (Component::DataCatalog, 0, slices[0]),
    ] {
        assert!(
            f.server
                .put_bootstrap_chunk(&f.auth(), f.request(component, index, bytes))
                .await
                .is_err(),
            "{component:?}/{index}"
        );
        assert_eq!(stored_chunks(&f).await, stored);
        assert_eq!(f.status().await, partial);
    }
    f.server
        .put_bootstrap_chunk(&f.auth(), f.request(Component::PrefixCatalog, 0, slices[0]))
        .await
        .unwrap();
    assert_eq!(
        presence(&f.status().await, Component::PrefixCatalog),
        [Presence::Verified, Presence::Verified]
    );
    f.upload().await;
    f.upload().await;
}

#[tokio::test]
async fn hash_valid_malformed_catalogs_never_complete_or_admit_images() {
    // Broken dense rank: every slice hash matches, the catalog does not decode.
    let mut f = Fixture::new().await;
    f.package.catalogs[1][23..31].copy_from_slice(&2_u64.to_be_bytes());
    bootstrap_format::recommit_catalog(&mut f.package, 1);
    let s = f.declare().await;
    for _ in 0..2 {
        assert!(
            f.server
                .put_bootstrap_chunk(
                    &f.auth(),
                    f.request(Component::PrefixCatalog, 0, &f.package.catalogs[1])
                )
                .await
                .is_err()
        );
        assert_eq!(f.status().await, s);
    }

    // Reference to an absent parent in an otherwise hash-valid image catalog.
    let mut f = Fixture::new().await;
    let mut images = bootstrap_format::catalog::Images::decode(&f.package.catalogs[2]).unwrap();
    assert!(!images.references.is_empty());
    images.parents.clear();
    f.package.catalogs[2] = images.encode().unwrap();
    bootstrap_format::recommit_catalog(&mut f.package, 2);
    f.declare().await;
    f.server
        .put_bootstrap_chunk(
            &f.auth(),
            f.request(Component::DataCatalog, 0, &f.package.catalogs[0]),
        )
        .await
        .unwrap();
    let before = f.status().await;
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(Component::ImageCatalog, 0, &f.package.catalogs[2])
            )
            .await
            .is_err()
    );
    assert_eq!(f.status().await, before);
    let image = &f.package.images[0];
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(Component::Image(image.object_id), 0, &image.records[0])
            )
            .await
            .is_err()
    );
    // Even bytes written behind the PUT check never become a valid layout.
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("INSERT INTO server_bootstrap_chunks(bootstrap, component, chunk_index, bytes) VALUES (?, ?, 0, ?)")
        .bind(f.id.as_slice()).bind(Component::ImageCatalog.key()).bind(&f.package.catalogs[2])
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        f.server
            .bootstrap_staging_status(&f.auth(), f.id)
            .await
            .is_err()
    );
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(Component::Image(image.object_id), 0, &image.records[0])
            )
            .await
            .is_err()
    );
    // Cancellation remains available for an unusable candidate.
    assert_eq!(
        f.server
            .cancel_bootstrap_staging(&f.auth(), f.id)
            .await
            .unwrap(),
        Status::Canceled
    );
}

#[tokio::test]
async fn storage_failures_roll_back_declaration_upload_and_cancellation() {
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
    drop(conn);
    let s = f.declare().await;
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("CREATE TRIGGER fail_put BEFORE INSERT ON server_bootstrap_chunks BEGIN SELECT RAISE(ABORT, 'injected'); END")
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(Component::Manifest, 0, &f.package.manifest[0])
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
    f.upload().await;
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
    assert_eq!(f.status().await, before);
    f.upload().await;
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
    let too_large = vec![0; bootstrap_format::MAX_DESCRIPTOR_BYTES + 1];
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
    f.server
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
            .put_bootstrap_chunk(&f.auth(), f.request(Component::Manifest, 0, &bytes))
            .await
            .is_err()
    );
    f.server
        .put_bootstrap_chunk(
            &f.auth(),
            f.request(Component::DataCatalog, 0, &f.package.catalogs[0]),
        )
        .await
        .unwrap();
    // The image recipes it would admit exceed the declared budget.
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(Component::ImageCatalog, 0, &f.package.catalogs[2])
            )
            .await
            .is_err()
    );
    assert_eq!(
        presence(&f.status().await, Component::ImageCatalog),
        [Presence::Missing]
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
    f.declare().await;
    f.upload().await;
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
    bootstrap_format::recommit_catalog(&mut f.package, 1);
    f.declare().await;
    f.upload().await;
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
    let capture = f
        .source
        .resume_local_shared_state_never_dispatched()
        .await
        .unwrap()
        .unwrap();
    let stream: [u8; 32] = hex::decode(capture.stream_id())
        .unwrap()
        .try_into()
        .unwrap();
    assert!(local.upload_package() == original);
    assert!(
        bootstrap_format::authenticate(
            &f.package,
            &f.key,
            f.seed.genesis().context(),
            stream,
            f.id,
            f.seed.genesis().commitment()
        )
        .is_err()
    );
}

#[tokio::test]
async fn ensure_revalidates_retained_bytes_before_renewing_reservations() {
    let f = Fixture::new().await;
    f.declare().await;
    f.upload().await;
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
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(Component::Manifest, 0, &f.package.manifest[0])
            )
            .await
            .is_err()
    );
    // Retained bytes are never rewritten; cancellation is the exit.
    assert_eq!(
        f.server
            .cancel_bootstrap_staging(&f.auth(), f.id)
            .await
            .unwrap(),
        Status::Canceled
    );
}

#[tokio::test]
async fn artifact_aggregate_failure_rolls_back_the_final_chunk() {
    let mut f = Fixture::new().await;
    // Corrupt the state recipe's aggregate inside a recommitted data catalog.
    f.package.catalogs[0][32] ^= 1;
    bootstrap_format::recommit_catalog(&mut f.package, 0);
    f.declare().await;
    f.server
        .put_bootstrap_chunk(
            &f.auth(),
            f.request(Component::DataCatalog, 0, &f.package.catalogs[0]),
        )
        .await
        .unwrap();
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(Component::State, 0, &f.package.state[0])
            )
            .await
            .is_err()
    );
    assert_eq!(
        presence(&f.status().await, Component::State),
        [Presence::Missing]
    );
}
