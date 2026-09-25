use super::*;

async fn pair(f: &Fixture) -> (Membership, Declaration, Joiner, Vec<u8>) {
    let m = initial(f);
    let a = auth(f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let (d, peer, raw) = prepare(Device::seed(&f.seed), &m, f, 3700);
    register(f, &a, &d, &peer).await;
    f.db.admit_membership_device_at(&a, d.handle(), &raw, 101)
        .await
        .unwrap();
    (
        m.append(d.record(), peer.request(), &raw).unwrap(),
        d,
        peer,
        raw,
    )
}

#[tokio::test]
async fn removed_seed_is_denied_before_stale_and_historical_survivor_receipt_remains_valid() {
    let f = Fixture::new().await;
    let (m, d, peer, receipt) = pair(&f).await;
    let seed = f.seed.genesis().device_id();
    let sa = auth(&f, m.head(), seed, f.seed.bearer());
    let (unused, candidate, _) = prepare(Device::seed(&f.seed), &m, &f, 3700);
    register(&f, &sa, &unused, &candidate).await;
    let pa = auth(&f, m.head(), peer.device(), peer.bearer());
    let revoke = peer.authority().prepare_revoke(&m, &[seed]).unwrap();
    f.db.apply_membership_management(&pa, &revoke)
        .await
        .unwrap();
    for head in [m.head(), [0; 32]] {
        let bad = auth(&f, head, seed, f.seed.bearer());
        let e = f.db.membership_evidence(&bad).await.err().unwrap();
        assert_eq!(e.to_string(), "error enrollment-unauthorized");
        assert!(!e.is::<StaleContext>());
        assert!(f.db.prepare_membership_management(&bad).await.is_err());
        assert!(
            f.db.apply_membership_management(&bad, &revoke)
                .await
                .is_err()
        );
        assert!(
            f.db.admit_membership_device_at(&bad, d.handle(), &receipt, 102)
                .await
                .is_err()
        );
        assert!(
            f.db.published_snapshot_read(
                &bad,
                m.publication().binding().descriptor_commitment,
                None,
                0
            )
            .await
            .is_err()
        );
    }
    let bootstrap_auth = crate::sync::bootstrap_staging::Authentication {
        vault_id: sa.vault,
        genesis_commitment: sa.genesis,
        bearer: f.seed.bearer(),
    };
    assert!(
        f.db.bootstrap_staging_status(&bootstrap_auth, m.publication().binding().bootstrap_id)
            .await
            .is_err()
    );
    assert!(
        f.db.publish_bootstrap(
            &bootstrap_auth,
            crate::sync::bootstrap_staging::PublishBootstrap {
                bootstrap_id: m.publication().binding().bootstrap_id,
                descriptor_commitment: m.publication().binding().descriptor_commitment,
                record: f.publication.record()
            },
            Default::default()
        )
        .await
        .is_err()
    );
    assert!(
        f.db.post_membership_request_at(pa.vault, unused.handle(), candidate.request(), 102)
            .await
            .is_err()
    );
    assert!(
        f.db.membership_mailbox(pa.vault, unused.handle())
            .await
            .is_err()
    );
    let evidence = f.db.membership_evidence(&pa).await.unwrap();
    let pending = evidence.verify().unwrap();
    assert!(pending.rotation_pending());
    evidence.enrollment(&peer, hash(&receipt)).unwrap();
    assert_eq!(
        f.db.membership_mailbox(pa.vault, d.handle())
            .await
            .unwrap()
            .admission
            .unwrap(),
        receipt
    );
    let current = auth(&f, pending.head(), peer.device(), peer.bearer());
    assert_eq!(
        f.db.admit_membership_device_at(&current, d.handle(), &receipt, 4000)
            .await
            .unwrap(),
        receipt
    );
    assert_eq!(
        f.db.apply_membership_management(&current, &revoke)
            .await
            .unwrap(),
        revoke
    );
    let prep = f.db.prepare_membership_management(&current).await.unwrap();
    let keys = pending.verify_initial_key(&f.key).unwrap();
    let wrong = peer
        .authority()
        .prepare_rotation(&pending, &keys, prep.high_water + 1)
        .unwrap();
    assert!(
        f.db.apply_membership_management(&current, &wrong)
            .await
            .unwrap_err()
            .to_string()
            .contains("cutoff")
    );
    let rotation = peer
        .authority()
        .prepare_rotation(&pending, &keys, prep.high_water)
        .unwrap();
    f.db.apply_membership_management(&current, &rotation)
        .await
        .unwrap();
    let reopened = Database::open(&f.dir.path().join("server.db"))
        .await
        .unwrap();
    let rotated = reopened
        .membership_evidence(&current)
        .await
        .unwrap()
        .verify()
        .unwrap();
    assert!(!rotated.rotation_pending());
    let current = auth(&f, rotated.head(), peer.device(), peer.bearer());
    let after = reopened
        .prepare_membership_management(&current)
        .await
        .unwrap();
    assert_eq!(prep.high_water, after.high_water);
    assert_eq!(after.evidence.transitions.len(), 3);
    assert_eq!(
        reopened
            .apply_membership_management(&current, &rotation)
            .await
            .unwrap(),
        rotation
    );
    assert!(
        peer.authority()
            .prepare_revoke(&rotated, &[peer.device()])
            .is_err()
    );
}

#[tokio::test]
async fn management_journal_head_and_invitation_failure_roll_back_together() {
    let f = Fixture::new().await;
    let (m, _, peer, _) = pair(&f).await;
    let sa = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let (unused, candidate, _) = prepare(Device::seed(&f.seed), &m, &f, 3700);
    register(&f, &sa, &unused, &candidate).await;
    let pa = auth(&f, m.head(), peer.device(), peer.bearer());
    let record = peer.authority().prepare_revoke(&m, &[sa.device]).unwrap();
    for (table, action) in [
        ("server_membership_transitions", "INSERT"),
        ("server_e2ee_membership_head", "UPDATE"),
        ("server_membership_invitations", "UPDATE"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE TRIGGER fault BEFORE {action} ON {table} BEGIN SELECT RAISE(ABORT,'fault'); END"
        )))
        .execute(&mut *f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
        assert!(
            f.db.apply_membership_management(&pa, &record)
                .await
                .is_err()
        );
        sqlx::query("DROP TRIGGER fault")
            .execute(&mut *f.db.acquire_writer().await.unwrap())
            .await
            .unwrap();
        assert_eq!(
            f.db.membership_evidence(&sa)
                .await
                .unwrap()
                .verify()
                .unwrap()
                .head(),
            m.head()
        );
        f.db.post_membership_request_at(sa.vault, unused.handle(), candidate.request(), 102)
            .await
            .unwrap();
    }
    f.db.apply_membership_management(&pa, &record)
        .await
        .unwrap();
    let pending = m.append(&[], &[], &record).unwrap();
    let pa = auth(&f, pending.head(), peer.device(), peer.bearer());
    let prep = f.db.prepare_membership_management(&pa).await.unwrap();
    let rotate = peer
        .authority()
        .prepare_rotation(
            &pending,
            &pending.verify_initial_key(&f.key).unwrap(),
            prep.high_water,
        )
        .unwrap();
    sqlx::query("CREATE TRIGGER fault BEFORE UPDATE ON server_e2ee_membership_head BEGIN SELECT RAISE(ABORT,'fault'); END").execute(&mut *f.db.acquire_writer().await.unwrap()).await.unwrap();
    assert!(
        f.db.apply_membership_management(&pa, &rotate)
            .await
            .is_err()
    );
    sqlx::query("DROP TRIGGER fault")
        .execute(&mut *f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
    assert_eq!(
        f.db.membership_evidence(&pa)
            .await
            .unwrap()
            .verify()
            .unwrap()
            .head(),
        pending.head()
    );
    assert_eq!(
        f.db.prepare_membership_management(&pa)
            .await
            .unwrap()
            .high_water,
        prep.high_water
    );
}

#[tokio::test]
async fn independent_pools_serialize_revoke_against_admission_and_rotation_candidates() {
    for _ in 0..3 {
        let f = Fixture::new().await;
        let (m, _, peer, _) = pair(&f).await;
        let sa = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
        let pa = auth(&f, m.head(), peer.device(), peer.bearer());
        let (d, candidate, admission) = prepare(Device::seed(&f.seed), &m, &f, 3700);
        register(&f, &sa, &d, &candidate).await;
        let revoke = peer.authority().prepare_revoke(&m, &[sa.device]).unwrap();
        let other = Database::open(&f.dir.path().join("server.db"))
            .await
            .unwrap();
        let (r, a) = tokio::join!(
            f.db.apply_membership_management(&pa, &revoke),
            other.admit_membership_device_at(&sa, d.handle(), &admission, 102)
        );
        assert_ne!(r.is_ok(), a.is_ok());
        let mut latest =
            f.db.membership_evidence(&pa)
                .await
                .unwrap()
                .verify()
                .unwrap();
        if a.is_ok() {
            let pa = auth(&f, latest.head(), peer.device(), peer.bearer());
            let revoke = peer
                .authority()
                .prepare_revoke(&latest, &[sa.device])
                .unwrap();
            f.db.apply_membership_management(&pa, &revoke)
                .await
                .unwrap();
            latest = latest.append(&[], &[], &revoke).unwrap();
        }
        let pa = auth(&f, latest.head(), peer.device(), peer.bearer());
        let keys = latest.verify_initial_key(&f.key).unwrap();
        let high =
            f.db.prepare_membership_management(&pa)
                .await
                .unwrap()
                .high_water;
        let one = peer
            .authority()
            .prepare_rotation(&latest, &keys, high)
            .unwrap();
        let two = peer
            .authority()
            .prepare_rotation(&latest, &keys, high)
            .unwrap();
        let (r1, r2) = tokio::join!(
            f.db.apply_membership_management(&pa, &one),
            other.apply_membership_management(&pa, &two)
        );
        assert_ne!(r1.is_ok(), r2.is_ok());
        assert_eq!(
            f.db.membership_evidence(&pa)
                .await
                .unwrap()
                .verify()
                .unwrap()
                .sequence(),
            latest.sequence() + 1
        );
    }
}

#[tokio::test]
async fn pending_admission_enters_rotation_coverage_and_fresh_join_gets_all_history() {
    let f = Fixture::new().await;
    let (m, d, peer, _) = pair(&f).await;
    let sa = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let revoke = Device::seed(&f.seed)
        .prepare_revoke(&m, &[peer.device()])
        .unwrap();
    f.db.apply_membership_management(&sa, &revoke)
        .await
        .unwrap();
    assert!(f.db.membership_mailbox(sa.vault, d.handle()).await.is_err());
    assert!(
        f.db.post_membership_request_at(sa.vault, d.handle(), peer.request(), 102)
            .await
            .is_err()
    );
    let pending = m.append(&[], &[], &revoke).unwrap();
    let sa = auth(&f, pending.head(), sa.device, f.seed.bearer());
    let (d2, second, admission) = prepare(Device::seed(&f.seed), &pending, &f, 3700);
    register(&f, &sa, &d2, &second).await;
    f.db.admit_membership_device_at(&sa, d2.handle(), &admission, 102)
        .await
        .unwrap();
    let pending = pending
        .append(d2.record(), second.request(), &admission)
        .unwrap();
    assert!(pending.rotation_pending());
    let sa = auth(&f, pending.head(), sa.device, f.seed.bearer());
    let keys = pending.verify_initial_key(&f.key).unwrap();
    let high =
        f.db.prepare_membership_management(&sa)
            .await
            .unwrap()
            .high_water;
    let rotation = Device::seed(&f.seed)
        .prepare_rotation(&pending, &keys, high)
        .unwrap();
    f.db.apply_membership_management(&sa, &rotation)
        .await
        .unwrap();
    let keys = second
        .authority()
        .receive_rotation(&pending, &rotation, &keys)
        .unwrap();
    let m = pending.append(&[], &[], &rotation).unwrap();
    let pa = auth(&f, m.head(), second.device(), second.bearer());
    let (inv, d3) = second.authority().prepare_invitation(&m, 3700).unwrap();
    let fresh = Joiner::generate(
        Invitation::from_protected_storage(&inv.protected_storage_bytes()).unwrap(),
    )
    .unwrap();
    let admission = second
        .authority()
        .prepare_admission(&m, &d3, &inv, fresh.request(), &keys)
        .unwrap();
    register(&f, &pa, &d3, &fresh).await;
    f.db.admit_membership_device_at(&pa, d3.handle(), &admission, 103)
        .await
        .unwrap();
    let e = f.db.membership_evidence(&pa).await.unwrap();
    let enrolled = e.enrollment(&fresh, hash(&admission)).unwrap();
    for g in m.generations() {
        assert_eq!(
            enrolled.keys().key(g.id).unwrap().protected_storage_bytes(),
            keys.key(g.id).unwrap().protected_storage_bytes()
        );
    }
    assert_eq!(e.verify().unwrap().generations().len(), 2);
}
