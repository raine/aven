use super::*;

#[tokio::test]
async fn current_authority_precedes_exact_outcome_and_never_uses_historical_credentials() {
    let f = Fixture::new().await;
    let p = f.publication();
    f.declare().await;
    f.upload().await;
    let wrong = Secret::new([0; 32]);
    let auth = Authentication {
        bearer: &wrong,
        ..f.auth()
    };
    assert!(
        f.server
            .publish_bootstrap(&auth, f.publish_request(&p), Default::default())
            .await
            .is_err()
    );
    f.unpublished().await;
    f.publish(&p).await.unwrap();
    assert!(
        f.server
            .publish_bootstrap(&auth, f.publish_request(&p), Default::default())
            .await
            .is_err()
    );
    assert!(
        f.server
            .bootstrap_staging_status(&auth, f.id)
            .await
            .is_err()
    );
    assert!(
        f.server
            .cancel_bootstrap_staging(&auth, f.id)
            .await
            .is_err()
    );
    assert!(
        f.server
            .admit_seed_claim(
                &f.seed.genesis().claim_bytes(),
                ClaimAuthentication::SeedBearer(f.seed.bearer())
            )
            .await
            .is_err()
    );
    assert!(
        f.server
            .declare_bootstrap_staging(&f.auth(), &f.package.descriptor, f.budget())
            .await
            .is_err()
    );
    assert!(
        f.server
            .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
            .await
            .is_err()
    );
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(Component::Manifest, 0, &f.package.manifest[0])
            )
            .await
            .is_err()
    );
    assert!(
        f.server
            .cancel_bootstrap_staging(&f.auth(), [99; 32])
            .await
            .is_err()
    );
    for (sequence, commitment) in [(2, p.commitment()), (1, [0; 32])] {
        let mut conn = f.server.acquire_writer().await.unwrap();
        sqlx::query("UPDATE server_e2ee_membership_head SET sequence = ?, commitment = ?")
            .bind(sequence)
            .bind(commitment.as_slice())
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);
        assert!(f.publish(&p).await.is_err());
        assert!(
            f.server
                .bootstrap_staging_status(&f.auth(), f.id)
                .await
                .is_err()
        );
        assert!(
            f.server
                .cancel_bootstrap_staging(&f.auth(), f.id)
                .await
                .is_err()
        );
    }
    let mut conn = f.server.acquire_reader().await.unwrap();
    let bytes: i64 = sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_chunks")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(bytes, 2);
}

#[tokio::test]
async fn cancellation_races_serialize_across_independent_pools() {
    let f = Fixture::new().await;
    let p = f.publication();
    f.declare().await;
    f.upload().await;
    let second = Database::open(&f.dir.path().join("server.sqlite"))
        .await
        .unwrap();
    let auth = f.auth();
    let (publish, cancellation) =
        tokio::join!(f.publish(&p), second.cancel_bootstrap_staging(&auth, f.id));
    match cancellation.unwrap() {
        Status::Published(outcome) => assert_eq!(publish.unwrap(), outcome),
        Status::Canceled => {
            assert!(publish.is_err());
            f.unpublished().await;
            assert!(f.publish(&p).await.is_err());
            assert!(
                f.server
                    .declare_bootstrap_staging(&auth, &f.package.descriptor, f.budget())
                    .await
                    .is_err()
            );
        }
        _ => panic!("nonterminal cancellation"),
    }
    let f = Fixture::new().await;
    let p = f.publication();
    f.server
        .cancel_bootstrap_staging(&f.auth(), f.id)
        .await
        .unwrap();
    assert!(f.publish(&p).await.is_err());
    f.unpublished().await;
}

/// Cancellation is terminal for the bootstrap ID, whether or not it was
/// declared. Nothing else rejects a delayed declaration of that ID, and the
/// seed's resume path declares whenever status does not report cancellation.
#[tokio::test]
async fn delayed_declaration_never_reopens_or_publishes_a_canceled_candidate() {
    for declared in [false, true] {
        let f = Fixture::new().await;
        let p = f.publication();
        if declared {
            f.declare().await;
            f.upload().await;
        }
        f.server
            .cancel_bootstrap_staging(&f.auth(), f.id)
            .await
            .unwrap();
        assert!(
            f.server
                .declare_bootstrap_staging(&f.auth(), &f.package.descriptor, f.budget())
                .await
                .is_err()
        );
        for (component, records) in f.components() {
            assert!(
                f.server
                    .put_bootstrap_chunk(&f.auth(), f.request(component, 0, records[0]))
                    .await
                    .is_err()
            );
        }
        assert!(f.publish(&p).await.is_err());
        f.unpublished().await;
    }
}

/// A request delayed across expiry and resume is indistinguishable from a
/// current one. Slot verification, expiry, cancellation and the single active
/// candidate still bound what it can change.
#[tokio::test]
async fn delayed_requests_across_expiry_resume_and_cancellation_cannot_change_staging() {
    let f = Fixture::new().await;
    let p = f.publication();
    f.declare().await;
    let put = async |component, index, bytes: &[u8]| {
        f.server
            .put_bootstrap_chunk(&f.auth(), f.request(component, index, bytes))
            .await
    };
    put(Component::DataCatalog, 0, &f.package.catalogs[0])
        .await
        .unwrap();
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("UPDATE server_bootstrap_candidates SET expires_at = 1")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(
        put(Component::Manifest, 0, &f.package.manifest[0])
            .await
            .is_err()
    );
    assert!(f.publish(&p).await.is_err());
    f.server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    let stored = async || -> Vec<(Vec<u8>, i64, Vec<u8>)> {
        sqlx::query_as("SELECT component, chunk_index, bytes FROM server_bootstrap_chunks ORDER BY component, chunk_index")
            .fetch_all(&mut *f.server.acquire_reader().await.unwrap())
            .await
            .unwrap()
    };
    // A delayed exact request stores only the bytes its slot commits to.
    put(Component::Manifest, 0, &f.package.manifest[0])
        .await
        .unwrap();
    let before = stored().await;
    let mut tampered = f.package.manifest[0].clone();
    tampered[0] ^= 1;
    let mut catalog = f.package.catalogs[0].clone();
    catalog[0] ^= 1;
    for (component, bytes) in [
        (Component::Manifest, tampered.as_slice()),
        (Component::State, f.package.manifest[0].as_slice()),
        (Component::DataCatalog, catalog.as_slice()),
    ] {
        assert!(put(component, 0, bytes).await.is_err());
    }
    assert!(f.publish(&p).await.is_err());
    assert_eq!(stored().await, before);
    f.unpublished().await;
    // Cancellation fences every delayed request for the canceled candidate.
    f.server
        .cancel_bootstrap_staging(&f.auth(), f.id)
        .await
        .unwrap();
    assert!(
        put(Component::Manifest, 0, &f.package.manifest[0])
            .await
            .is_err()
    );
    assert!(f.publish(&p).await.is_err());
    assert!(
        f.server
            .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
            .await
            .is_err()
    );
    // A replacement has its own ID and commitment, so delayed requests for the
    // canceled candidate cannot land in it.
    let mut descriptor = f.package.descriptor.clone();
    descriptor[103] ^= 1;
    f.server
        .declare_bootstrap_staging(&f.auth(), &descriptor, f.budget())
        .await
        .unwrap();
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                PutChunk {
                    bootstrap_id: descriptor[103..135].try_into().unwrap(),
                    ..f.request(Component::DataCatalog, 0, &f.package.catalogs[0])
                },
            )
            .await
            .is_err()
    );
    assert!(stored().await.is_empty());
    f.unpublished().await;
}

#[tokio::test]
async fn expired_status_is_read_only_and_conflicting_outcomes_never_replace_bindings() {
    let f = Fixture::new().await;
    let p = f.publication();
    f.declare().await;
    f.upload().await;
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("UPDATE server_bootstrap_candidates SET expires_at = 1")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(f.publish(&p).await.is_err());
    assert_eq!(f.status().await.expires_at, 1);
    f.server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    let second = Database::open(&f.dir.path().join("server.sqlite"))
        .await
        .unwrap();
    let auth = f.auth();
    let (a, b) = tokio::join!(
        f.publish(&p),
        second.publish_bootstrap(&auth, f.publish_request(&p), Default::default())
    );
    assert_eq!(a.unwrap(), b.unwrap());
    for field in 0..3 {
        let mut request = f.publish_request(&p);
        let mut record = p.record().to_vec();
        match field {
            0 => request.bootstrap_id[0] ^= 1,
            1 => request.descriptor_commitment[0] ^= 1,
            _ => {
                record[10] ^= 1;
                request.record = &record;
            }
        }
        assert!(
            f.server
                .publish_bootstrap(&auth, request, Default::default())
                .await
                .is_err()
        );
    }
    assert!(f.publish(&p).await.is_ok());
}

#[tokio::test]
async fn concurrent_unsupported_authority_change_never_reactivates_genesis() {
    let f = Fixture::new().await;
    let p = f.publication();
    f.declare().await;
    f.upload().await;
    let second = Database::open(&f.dir.path().join("server.sqlite"))
        .await
        .unwrap();
    // This is storage fault/successor-fence injection, not a supported membership
    // transition or a model of the future removal protocol.
    let advance = async {
        let mut conn = second.acquire_writer().await.unwrap();
        let mut tx = crate::db::begin_immediate(&mut conn).await.unwrap();
        sqlx::query("INSERT INTO server_e2ee_membership_head(singleton, sequence, commitment) VALUES (1, 2, ?) ON CONFLICT(singleton) DO UPDATE SET sequence = excluded.sequence, commitment = excluded.commitment")
            .bind([99_u8; 32].as_slice()).execute(&mut *tx).await.unwrap();
        sqlx::query("UPDATE server_seed_claim SET genesis_only = 0")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    };
    let (publication, ()) = tokio::join!(f.publish(&p), advance);
    assert!(f.publish(&p).await.is_err());
    assert!(
        f.server
            .bootstrap_staging_status(&f.auth(), f.id)
            .await
            .is_err()
    );
    assert!(
        f.server
            .cancel_bootstrap_staging(&f.auth(), f.id)
            .await
            .is_err()
    );
    let mut conn = f.server.acquire_reader().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_bootstrap_publication")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count, i64::from(publication.is_ok()));
    let image_chunks: i64 = sqlx::query_scalar("SELECT count(*) FROM server_e2ee_image_chunks")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(image_chunks, if publication.is_ok() { 2 } else { 0 });
}
