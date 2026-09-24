use super::*;

#[tokio::test]
async fn current_authority_precedes_exact_outcome_and_never_uses_historical_credentials() {
    let f = Fixture::new().await;
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
    let wrong = Secret::new([0; 32]);
    let auth = Authentication {
        bearer: &wrong,
        ..f.auth()
    };
    assert!(
        f.server
            .publish_bootstrap(&auth, f.publish_request(&p, s.epoch), Default::default())
            .await
            .is_err()
    );
    f.unpublished().await;
    f.publish(&p, s.epoch).await.unwrap();
    assert!(
        f.server
            .publish_bootstrap(&auth, f.publish_request(&p, s.epoch), Default::default())
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
                None,
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
            .reclaim_bootstrap_staging(&f.auth(), f.id, f.commitment(), s.epoch, Reclaim::All)
            .await
            .is_err()
    );
    assert!(
        f.server
            .put_bootstrap_chunk(
                &f.auth(),
                f.request(s.epoch, Component::Manifest, 0, &f.package.manifest[0])
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
        assert!(f.publish(&p, s.epoch).await.is_err());
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
        assert!(
            f.server
                .reclaim_bootstrap_staging(&f.auth(), f.id, f.commitment(), s.epoch, Reclaim::All)
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
async fn cancellation_and_epoch_races_serialize_across_independent_pools() {
    for cancel in [false, true] {
        let f = Fixture::new().await;
        let p = f.publication();
        let s = f.declare().await;
        f.upload(s.epoch).await;
        let second = Database::open(&f.dir.path().join("server.sqlite"))
            .await
            .unwrap();
        let auth = f.auth();
        if cancel {
            let (publish, cancellation) = tokio::join!(
                f.publish(&p, s.epoch),
                second.cancel_bootstrap_staging(&auth, f.id)
            );
            match cancellation.unwrap() {
                Status::Published(outcome) => assert_eq!(publish.unwrap(), outcome),
                Status::Canceled => {
                    assert!(publish.is_err());
                    f.unpublished().await;
                    assert!(f.publish(&p, s.epoch).await.is_err());
                    assert!(
                        f.server
                            .declare_bootstrap_staging(&auth, &f.package.descriptor, f.budget())
                            .await
                            .is_err()
                    );
                }
                _ => panic!("nonterminal cancellation"),
            }
        } else {
            let (publish, reclaim) = tokio::join!(
                f.publish(&p, s.epoch),
                second.reclaim_bootstrap_staging(
                    &auth,
                    f.id,
                    f.commitment(),
                    s.epoch,
                    Reclaim::Fence
                )
            );
            match publish {
                Ok(outcome) => {
                    assert!(reclaim.is_err());
                    assert_eq!(f.publish(&p, s.epoch).await.unwrap(), outcome);
                }
                Err(_) => {
                    reclaim.unwrap();
                    f.unpublished().await;
                    assert!(f.publish(&p, s.epoch).await.is_err());
                    let resumed = f
                        .server
                        .ensure_bootstrap_staging(&auth, f.id, f.commitment())
                        .await
                        .unwrap();
                    assert!(resumed.epoch > s.epoch);
                    f.publish(&p, resumed.epoch).await.unwrap();
                }
            }
        }
    }
    let f = Fixture::new().await;
    let p = f.publication();
    f.server
        .cancel_bootstrap_staging(&f.auth(), f.id)
        .await
        .unwrap();
    assert!(f.publish(&p, 1).await.is_err());
    f.unpublished().await;
}

#[tokio::test]
async fn expired_status_is_read_only_and_conflicting_outcomes_never_replace_bindings() {
    let f = Fixture::new().await;
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
    let mut conn = f.server.acquire_writer().await.unwrap();
    sqlx::query("UPDATE server_bootstrap_candidates SET expires_at = 1")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    assert!(f.publish(&p, s.epoch).await.is_err());
    assert_eq!(f.status().await.expires_at, 1);
    assert_eq!(f.status().await.epoch, s.epoch);
    let resumed = f
        .server
        .ensure_bootstrap_staging(&f.auth(), f.id, f.commitment())
        .await
        .unwrap();
    assert!(f.publish(&p, s.epoch).await.is_err());
    let second = Database::open(&f.dir.path().join("server.sqlite"))
        .await
        .unwrap();
    let auth = f.auth();
    let (a, b) = tokio::join!(
        f.publish(&p, resumed.epoch),
        second.publish_bootstrap(
            &auth,
            f.publish_request(&p, resumed.epoch),
            Default::default()
        )
    );
    assert_eq!(a.unwrap(), b.unwrap());
    for field in 0..3 {
        let mut request = f.publish_request(&p, resumed.epoch);
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
    assert!(f.publish(&p, 0).await.is_ok());
}

#[tokio::test]
async fn concurrent_unsupported_authority_change_never_reactivates_genesis() {
    let f = Fixture::new().await;
    let p = f.publication();
    let s = f.declare().await;
    f.upload(s.epoch).await;
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
    let (publication, ()) = tokio::join!(f.publish(&p, s.epoch), advance);
    assert!(f.publish(&p, s.epoch).await.is_err());
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
