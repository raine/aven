use super::*;
use crate::sync::seed_claim::membership::test_support::Fixture;

fn auth<'a>(f: &Fixture, head: Hash, device: Hash, bearer: &'a Secret) -> Authentication<'a> {
    Authentication {
        vault: f.seed.genesis().context().vault_id,
        genesis: f.seed.genesis().commitment(),
        device,
        credential_version: 1,
        head,
        bearer,
    }
}
fn initial(f: &Fixture) -> Membership {
    Membership::from_publication(
        f.seed.genesis(),
        &f.package.descriptor,
        f.publication.record(),
    )
    .unwrap()
}
fn prepare(
    device: Device<'_>,
    m: &Membership,
    f: &Fixture,
    expiry: u64,
) -> (Declaration, Joiner, Vec<u8>) {
    let (inv, d) = device.prepare_invitation(m, expiry).unwrap();
    let peer = Joiner::generate(
        Invitation::from_protected_storage(&inv.protected_storage_bytes()).unwrap(),
    )
    .unwrap();
    let raw = device
        .prepare_admission(
            m,
            &d,
            &inv,
            peer.request(),
            &m.verify_initial_key(&f.key).unwrap(),
        )
        .unwrap();
    (d, peer, raw)
}
async fn register(f: &Fixture, a: &Authentication<'_>, d: &Declaration, peer: &Joiner) {
    f.db.register_membership_invitation_at(a, d.record(), 100)
        .await
        .unwrap();
    f.db.post_membership_request_at(a.vault, d.handle(), peer.request(), 100)
        .await
        .unwrap();
}
#[tokio::test]
async fn repeated_seed_and_peer_admissions_authenticate_current_credential_before_history() {
    let f = Fixture::new().await;
    let m = initial(&f);
    let seed_device = f.seed.genesis().device_id();
    let a = auth(&f, m.head(), seed_device, f.seed.bearer());
    let (d, peer, raw) = prepare(Device::seed(&f.seed), &m, &f, 3700);
    register(&f, &a, &d, &peer).await;
    f.db.admit_membership_device_at(&a, d.handle(), &raw, 101)
        .await
        .unwrap();
    let second =
        f.db.membership_evidence(&a)
            .await
            .unwrap()
            .verify()
            .unwrap();
    assert_eq!(second.sequence(), 2);
    let peer_auth = auth(&f, second.head(), peer.device(), peer.bearer());
    let (d3, third, raw3) = prepare(peer.authority(), &second, &f, 3700);
    register(&f, &peer_auth, &d3, &third).await;
    f.db.admit_membership_device_at(&peer_auth, d3.handle(), &raw3, 102)
        .await
        .unwrap();
    let third_membership =
        f.db.membership_evidence(&a)
            .await
            .unwrap()
            .verify()
            .unwrap();
    assert_eq!(third_membership.device_count(), 3);
    assert!(third_membership.extends(&second));
    assert!(
        f.db.admit_membership_device_at(&a, d.handle(), &raw, 4000)
            .await
            .is_err()
    );
    let latest_auth = auth(&f, third_membership.head(), seed_device, f.seed.bearer());
    assert_eq!(
        f.db.admit_membership_device_at(&latest_auth, d.handle(), &raw, 4000)
            .await
            .unwrap(),
        raw
    );
    let third_auth = auth(&f, third_membership.head(), third.device(), third.bearer());
    let (d4, _, _) = prepare(third.authority(), &third_membership, &f, 3700);
    f.db.register_membership_invitation_at(&third_auth, d4.record(), 100)
        .await
        .unwrap();
    let wrong = Secret::new([99; 32]);
    let bad = auth(&f, [0; 32], seed_device, &wrong);
    assert!(
        f.db.membership_evidence(&bad)
            .await
            .err()
            .unwrap()
            .to_string()
            .contains("unauthorized")
    );
    let unknown = auth(&f, [0; 32], seed_device, f.seed.bearer());
    assert!(f.db.membership_evidence(&unknown).await.is_err());
    let reopened = Database::open(&f.dir.path().join("server.db"))
        .await
        .unwrap();
    assert_eq!(
        reopened
            .membership_evidence(&latest_auth)
            .await
            .unwrap()
            .verify()
            .unwrap()
            .head(),
        third_membership.head()
    );
}
#[tokio::test]
async fn vault_clock_expiry_and_unfinished_invitation_survive_reopen_and_rollback() {
    let f = Fixture::new().await;
    let m = initial(&f);
    let a = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let (d, peer, raw) = prepare(Device::seed(&f.seed), &m, &f, 200);
    register(&f, &a, &d, &peer).await;
    let (other, _, _) = prepare(Device::seed(&f.seed), &m, &f, 200);
    assert!(
        f.db.register_membership_invitation_at(&a, other.record(), 100)
            .await
            .is_err()
    );
    assert!(
        f.db.admit_membership_device_at(&a, d.handle(), &raw, 200)
            .await
            .is_err()
    );
    let reopened = Database::open(&f.dir.path().join("server.db"))
        .await
        .unwrap();
    assert!(
        reopened
            .admit_membership_device_at(&a, d.handle(), &raw, 101)
            .await
            .is_err()
    );
    assert_eq!(
        reopened
            .register_membership_invitation_at(&a, other.record(), 100)
            .await
            .unwrap(),
        RegistrationStatus::Expired
    );
    assert!(
        reopened
            .register_membership_invitation_at(&a, other.record(), -1)
            .await
            .is_err()
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_membership_invitations")
        .fetch_one(&mut *f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
    assert_eq!(count, 2);
}
#[tokio::test]
async fn independent_pools_serialize_same_slot_and_exact_retry() {
    let f = Fixture::new().await;
    let m = initial(&f);
    let a = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let (d, peer, raw) = prepare(Device::seed(&f.seed), &m, &f, 3700);
    register(&f, &a, &d, &peer).await;
    let other = Database::open(&f.dir.path().join("server.db"))
        .await
        .unwrap();
    let (one, two) = tokio::join!(
        f.db.admit_membership_device_at(&a, d.handle(), &raw, 100),
        other.admit_membership_device_at(&a, d.handle(), &raw, 100)
    );
    assert_ne!(one.is_ok(), two.is_ok());
    let m2 =
        f.db.membership_evidence(&a)
            .await
            .unwrap()
            .verify()
            .unwrap();
    let seed_auth = auth(&f, m2.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let peer_auth = auth(&f, m2.head(), peer.device(), peer.bearer());
    let (ds, js, rs) = prepare(Device::seed(&f.seed), &m2, &f, 3700);
    let (dp, jp, rp) = prepare(peer.authority(), &m2, &f, 3700);
    register(&f, &seed_auth, &ds, &js).await;
    register(&f, &peer_auth, &dp, &jp).await;
    let (one, two) = tokio::join!(
        f.db.admit_membership_device_at(&seed_auth, ds.handle(), &rs, 100),
        other.admit_membership_device_at(&peer_auth, dp.handle(), &rp, 100)
    );
    assert_ne!(one.is_ok(), two.is_ok());
    let m3 =
        f.db.membership_evidence(&a)
            .await
            .unwrap()
            .verify()
            .unwrap();
    assert_eq!(m3.sequence(), 3);
    assert!(m3.extends(&m2));
    let fresh = auth(&f, m3.head(), f.seed.genesis().device_id(), f.seed.bearer());
    assert_eq!(
        other
            .admit_membership_device_at(&fresh, d.handle(), &raw, 5000)
            .await
            .unwrap(),
        raw
    );
}
#[tokio::test]
async fn admission_writes_are_atomic_and_missing_history_never_authorizes() {
    let f = Fixture::new().await;
    let m = initial(&f);
    let a = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let (d, peer, raw) = prepare(Device::seed(&f.seed), &m, &f, 3700);
    register(&f, &a, &d, &peer).await;
    for (table, action) in [
        ("server_membership_transitions", "INSERT"),
        ("server_membership_invitations", "UPDATE"),
        ("server_e2ee_membership_head", "UPDATE"),
    ] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "CREATE TRIGGER fail BEFORE {action} ON {table} BEGIN SELECT RAISE(ABORT,'fault'); END"
        )))
        .execute(&mut *f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
        assert!(
            f.db.admit_membership_device_at(&a, d.handle(), &raw, 100)
                .await
                .is_err()
        );
        sqlx::query("DROP TRIGGER fail")
            .execute(&mut *f.db.acquire_writer().await.unwrap())
            .await
            .unwrap();
        assert_eq!(
            f.db.membership_evidence(&a)
                .await
                .unwrap()
                .verify()
                .unwrap()
                .head(),
            m.head()
        );
        assert!(
            f.db.membership_mailbox(a.vault, d.handle())
                .await
                .unwrap()
                .admission
                .is_none()
        );
    }
    f.db.admit_membership_device_at(&a, d.handle(), &raw, 100)
        .await
        .unwrap();
    sqlx::query("DELETE FROM server_membership_transitions")
        .execute(&mut *f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
    assert!(f.db.membership_evidence(&a).await.is_err());
}

#[tokio::test]
async fn invitation_budget_and_bounded_evidence_refuse_without_forgetting_outcomes() {
    let f = Fixture::new().await;
    let m = initial(&f);
    let a = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    for _ in 0..MAX_INVITATIONS {
        let (_, d) = Device::seed(&f.seed).prepare_invitation(&m, 100).unwrap();
        assert_eq!(
            f.db.register_membership_invitation_at(&a, d.record(), 100)
                .await
                .unwrap(),
            RegistrationStatus::Expired
        );
    }
    let (_, d) = Device::seed(&f.seed).prepare_invitation(&m, 100).unwrap();
    assert!(
        f.db.register_membership_invitation_at(&a, d.record(), 100)
            .await
            .is_err()
    );
    let evidence = f.db.membership_evidence(&a).await.unwrap();
    let bytes = serde_json::to_vec(&evidence).unwrap();
    assert_eq!(
        Evidence::decode(&bytes).unwrap().verify().unwrap().head(),
        m.head()
    );
    let mut value = serde_json::to_value(&evidence).unwrap();
    value["genesis"] = serde_json::json!(vec![0; MAX_RECORD_BYTES + 1]);
    assert!(serde_json::from_value::<Evidence>(value).is_err());
    assert!(Evidence::decode(&vec![b' '; MAX_EVIDENCE_JSON_BYTES + 1]).is_err());
    sqlx::query("DELETE FROM server_membership_clock")
        .execute(&mut *f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
    assert!(f.db.membership_evidence(&a).await.is_err());
}

#[tokio::test]
async fn a_verified_competing_successor_allows_same_recipient_at_a_new_predecessor() {
    let f = Fixture::new().await;
    let m = initial(&f);
    let a = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let (d, peer, raw) = prepare(Device::seed(&f.seed), &m, &f, 3700);
    register(&f, &a, &d, &peer).await;
    f.db.admit_membership_device_at(&a, d.handle(), &raw, 100)
        .await
        .unwrap();
    let m =
        f.db.membership_evidence(&a)
            .await
            .unwrap()
            .verify()
            .unwrap();
    let a = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let pa = auth(&f, m.head(), peer.device(), peer.bearer());
    let (inv, d) = Device::seed(&f.seed).prepare_invitation(&m, 3700).unwrap();
    let joiner = Joiner::generate(
        Invitation::from_protected_storage(&inv.protected_storage_bytes()).unwrap(),
    )
    .unwrap();
    let lost = Device::seed(&f.seed)
        .prepare_admission(
            &m,
            &d,
            &inv,
            joiner.request(),
            &m.verify_initial_key(&f.key).unwrap(),
        )
        .unwrap();
    register(&f, &a, &d, &joiner).await;
    let (winner, other, record) = prepare(peer.authority(), &m, &f, 3700);
    register(&f, &pa, &winner, &other).await;
    let pool = Database::open(&f.dir.path().join("server.db"))
        .await
        .unwrap();
    pool.admit_membership_device_at(&pa, winner.handle(), &record, 100)
        .await
        .unwrap();
    assert!(
        f.db.admit_membership_device_at(&a, d.handle(), &lost, 100)
            .await
            .is_err()
    );
    let next =
        f.db.membership_evidence(&a)
            .await
            .unwrap()
            .verify()
            .unwrap();
    assert!(next.extends(&m));
    assert!(!next.contains_head(&hash(&lost)));
    let retry = Device::seed(&f.seed)
        .prepare_admission(
            &next,
            &d,
            &inv,
            joiner.request(),
            &m.verify_initial_key(&f.key).unwrap(),
        )
        .unwrap();
    let fresh = auth(&f, next.head(), a.device, a.bearer);
    f.db.admit_membership_device_at(&fresh, d.handle(), &retry, 100)
        .await
        .unwrap();
    assert_eq!(
        f.db.membership_evidence(&a)
            .await
            .unwrap()
            .verify()
            .unwrap()
            .device_count(),
        4
    );
    assert!(joiner.verify_enrollment(&next, d.record(), &retry).is_ok());
}

#[tokio::test]
async fn independent_scanners_cannot_rebind_an_occupied_request() {
    let f = Fixture::new().await;
    let m = initial(&f);
    let a = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let (invitation, d) = Device::seed(&f.seed).prepare_invitation(&m, 3700).unwrap();
    let one = Joiner::generate(
        Invitation::from_protected_storage(&invitation.protected_storage_bytes()).unwrap(),
    )
    .unwrap();
    let two = Joiner::generate(
        Invitation::from_protected_storage(&invitation.protected_storage_bytes()).unwrap(),
    )
    .unwrap();
    f.db.register_membership_invitation_at(&a, d.record(), 100)
        .await
        .unwrap();
    let other = Database::open(&f.dir.path().join("server.db"))
        .await
        .unwrap();
    let (first, second) = tokio::join!(
        f.db.post_membership_request_at(a.vault, d.handle(), one.request(), 100),
        other.post_membership_request_at(a.vault, d.handle(), two.request(), 100)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    let accepted = if first.is_ok() { one } else { two };
    f.db.post_membership_request_at(a.vault, d.handle(), accepted.request(), 101)
        .await
        .unwrap();
    assert_eq!(
        f.db.membership_mailbox(a.vault, d.handle())
            .await
            .unwrap()
            .request
            .as_deref(),
        Some(accepted.request())
    );
    let raw = Device::seed(&f.seed)
        .prepare_admission(
            &m,
            &d,
            &invitation,
            accepted.request(),
            &m.verify_initial_key(&f.key).unwrap(),
        )
        .unwrap();
    f.db.admit_membership_device_at(&a, d.handle(), &raw, 102)
        .await
        .unwrap();
    let current =
        f.db.membership_evidence(&a)
            .await
            .unwrap()
            .verify()
            .unwrap();
    let fresh = auth(&f, current.head(), a.device, a.bearer);
    assert_eq!(
        f.db.admit_membership_device_at(&fresh, d.handle(), &raw, 4000)
            .await
            .unwrap(),
        raw
    );
    let (_, later) = Device::seed(&f.seed)
        .prepare_invitation(&current, 3700)
        .unwrap();
    assert_eq!(
        f.db.register_membership_invitation_at(&fresh, later.record(), 100)
            .await
            .unwrap(),
        RegistrationStatus::Expired
    );
    assert!(
        f.db.admit_membership_device_at(&fresh, d.handle(), &raw, -1)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn occupied_invalid_request_and_signed_history_corruption_fail_closed() {
    let f = Fixture::new().await;
    let m = initial(&f);
    let a = auth(&f, m.head(), f.seed.genesis().device_id(), f.seed.bearer());
    let (invitation, d) = Device::seed(&f.seed).prepare_invitation(&m, 3700).unwrap();
    let peer = Joiner::generate(
        Invitation::from_protected_storage(&invitation.protected_storage_bytes()).unwrap(),
    )
    .unwrap();
    let mut invalid = peer.request().to_vec();
    *invalid.last_mut().unwrap() ^= 1;
    f.db.register_membership_invitation_at(&a, d.record(), 100)
        .await
        .unwrap();
    f.db.post_membership_request_at(a.vault, d.handle(), &invalid, 100)
        .await
        .unwrap();
    assert!(
        Device::seed(&f.seed)
            .prepare_admission(
                &m,
                &d,
                &invitation,
                &invalid,
                &m.verify_initial_key(&f.key).unwrap()
            )
            .is_err()
    );
    assert!(
        f.db.post_membership_request_at(a.vault, d.handle(), peer.request(), 100)
            .await
            .is_err()
    );
    assert_eq!(
        f.db.membership_mailbox(a.vault, d.handle())
            .await
            .unwrap()
            .request
            .unwrap(),
        invalid
    );
    let oversized = vec![0; MAX_RECORD_BYTES + 1];
    let error =
        f.db.admit_membership_device_at(&a, d.handle(), &oversized, 100)
            .await
            .err()
            .unwrap();
    assert!(!error.to_string().contains(&hex::encode(a.bearer.expose())));
    // This is unsupported-store corruption, not implemented removal.
    sqlx::query("UPDATE server_e2ee_membership_head SET commitment=zeroblob(32)")
        .execute(&mut *f.db.acquire_writer().await.unwrap())
        .await
        .unwrap();
    assert!(f.db.membership_evidence(&a).await.is_err());
    assert!(
        f.db.register_membership_invitation_at(&a, d.record(), 100)
            .await
            .is_err()
    );
}

mod rotation;
