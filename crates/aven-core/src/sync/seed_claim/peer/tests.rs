use super::*;
use crate::{
    db::Database,
    sync::{bootstrap_format, bootstrap_staging as staging},
};

pub(crate) struct Fixture {
    pub(crate) dir: tempfile::TempDir,
    pub(crate) db: Database,
    pub(crate) seed: SeedAuthority,
    pub(crate) key: LocalSharedStatePackageKey,
    pub(crate) package: bootstrap_format::Package,
    pub(crate) publication: Publication,
}
impl Fixture {
    pub(crate) async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let source = Database::open(&dir.path().join("source.db")).await.unwrap();
        let context = LocalSharedStatePackageContext {
            vault_id: [1; 32],
            generation_id: [2; 32],
        };
        let key = LocalSharedStatePackageKey::new([3; 32]);
        let seed = SeedAuthority::generate(context, &key, [4; 32]).unwrap();
        source
            .capture_local_shared_state_never_dispatched(dir.path())
            .await
            .unwrap();
        let package = source
            .package_local_shared_state_never_dispatched(
                dir.path(),
                context,
                &key,
                seed.genesis().commitment(),
            )
            .await
            .unwrap()
            .upload_package();
        let p = seed.prepare_bootstrap_publication(&package, &key).unwrap();
        let db = Database::open(&dir.path().join("server.db")).await.unwrap();
        let setup_secret = Secret::new([5; 32]);
        let setup = SetupAuthority::from_verifier(
            [4; 32],
            SetupAuthority::verifier([4; 32], &setup_secret),
        );
        db.admit_seed_claim(
            &seed.genesis().claim_bytes(),
            Some(&setup),
            ClaimAuthentication::SetupSecret(&setup_secret),
        )
        .await
        .unwrap();
        let auth = staging::Authentication {
            vault_id: context.vault_id,
            genesis_commitment: seed.genesis().commitment(),
            bearer: seed.bearer(),
        };
        let mut components = Vec::new();
        for (i, c) in [
            staging::Component::DataCatalog,
            staging::Component::PrefixCatalog,
            staging::Component::ImageCatalog,
        ]
        .into_iter()
        .enumerate()
        {
            components.push((c, package.catalogs[i].chunks(1048576).collect::<Vec<_>>()));
        }
        components.push((
            staging::Component::Manifest,
            package.manifest.iter().map(Vec::as_slice).collect(),
        ));
        components.push((
            staging::Component::State,
            package.state.iter().map(Vec::as_slice).collect(),
        ));
        let budget = staging::Budget {
            bytes: components
                .iter()
                .flat_map(|(_, v)| v)
                .map(|v| v.len() as u64)
                .sum(),
            chunks: components.iter().map(|(_, v)| v.len() as u64).sum(),
        };
        let status = db
            .declare_bootstrap_staging(&auth, &package.descriptor, budget)
            .await
            .unwrap();
        for (component, records) in components {
            for (index, bytes) in records.iter().enumerate() {
                db.put_bootstrap_chunk(
                    &auth,
                    staging::PutChunk {
                        bootstrap_id: p.binding().bootstrap_id,
                        descriptor_commitment: p.binding().descriptor_commitment,
                        epoch: status.epoch,
                        component,
                        index: index as u64,
                        bytes,
                    },
                )
                .await
                .unwrap();
            }
        }
        db.publish_bootstrap(
            &auth,
            staging::PublishBootstrap {
                bootstrap_id: p.binding().bootstrap_id,
                descriptor_commitment: p.binding().descriptor_commitment,
                epoch: status.epoch,
                record: p.record(),
            },
            Default::default(),
        )
        .await
        .unwrap();
        Self {
            dir,
            db,
            seed,
            key,
            package,
            publication: p,
        }
    }
    fn auth(&self) -> Authentication<'_> {
        Authentication {
            vault: self.seed.genesis().context.vault_id,
            genesis: self.seed.genesis().commitment(),
            device: self.seed.genesis().device,
            credential_version: 1,
            head: self.publication.commitment(),
            bearer: self.seed.bearer(),
        }
    }
    fn prepare(&self) -> (Invitation, Declaration, PeerAuthority, Admission) {
        let (inv, d) = self
            .seed
            .prepare_peer_invitation(
                &self.publication,
                u64::try_from(persistence::test_now()).unwrap() + 3600,
            )
            .unwrap();
        let peer = PeerAuthority::generate(
            Invitation::from_protected_storage(&inv.protected_storage_bytes()).unwrap(),
        )
        .unwrap();
        let a = self
            .seed
            .prepare_peer_admission(&self.publication, &d, &inv, peer.request(), &self.key)
            .unwrap();
        (inv, d, peer, a)
    }
    async fn post(&self, d: &Declaration, peer: &PeerAuthority) {
        self.db
            .register_peer_invitation(&self.auth(), d.record())
            .await
            .unwrap();
        self.db
            .post_peer_request(peer.vault(), peer.handle(), peer.request())
            .await
            .unwrap();
    }
    fn evidence(&self, d: &Declaration, peer: &PeerAuthority, a: &Admission) -> Evidence {
        Evidence {
            genesis: self.seed.genesis().record().to_vec(),
            publication: self.publication.record().to_vec(),
            declaration: d.record().to_vec(),
            request: peer.request().to_vec(),
            admission: a.record().to_vec(),
        }
    }
}

#[tokio::test]
async fn exact_profile_and_psk_trust_negative_matrix() {
    let f = Fixture::new().await;
    let (inv, d, peer, a) = f.prepare();
    assert_eq!(
        (d.record().len(), peer.request().len(), a.record().len()),
        (280, 314, 1698)
    );
    let (core, state, attachments, _) = components(a.record()).unwrap();
    assert_eq!(
        (core.len(), state.len(), attachments.len()),
        (461, 580, 576)
    );
    assert_eq!(&state[4..6], &3_u16.to_be_bytes());
    let evidence = f.evidence(&d, &peer, &a);
    let verified = peer
        .verify_enrollment(&evidence, &f.package.descriptor)
        .unwrap();
    assert_eq!(
        verified.key().protected_storage_bytes(),
        f.key.protected_storage_bytes()
    );
    assert_ne!(peer.device(), f.seed.genesis().device_id());
    assert_ne!(peer.bearer().expose(), f.seed.bearer().expose());
    let peer = PeerAuthority::from_protected_storage(&peer.protected_storage_bytes()).unwrap();
    peer.verify_enrollment(&evidence, &f.package.descriptor)
        .unwrap();
    for index in [0, 40, 120, 490, 1000, 1697] {
        let mut bad = evidence.clone();
        bad.admission[index] ^= 1;
        assert!(peer.verify_enrollment(&bad, &f.package.descriptor).is_err());
    }
    for index in 0..a.record().len() {
        assert!(
            Admission::from_record(
                f.seed.genesis(),
                &f.publication,
                &d,
                peer.request(),
                &a.record()[..index]
            )
            .is_err()
        );
    }
    let mut extra = a.record().to_vec();
    extra.push(0);
    assert!(
        Admission::from_record(f.seed.genesis(), &f.publication, &d, peer.request(), &extra)
            .is_err()
    );
    let mut wrong = inv.protected_storage_bytes();
    wrong[95] ^= 1;
    let other =
        PeerAuthority::generate(Invitation::from_protected_storage(&wrong).unwrap()).unwrap();
    assert!(
        f.seed
            .prepare_peer_admission(&f.publication, &d, &inv, other.request(), &f.key)
            .is_err()
    );
    let mut descriptor = f.package.descriptor.clone();
    descriptor[10] ^= 1;
    assert!(peer.verify_enrollment(&evidence, &descriptor).is_err());
    let mut bad = evidence.clone();
    bad.genesis[100] ^= 1;
    assert!(peer.open_provisional(&bad).is_err());
    let mut bad = evidence.clone();
    bad.request[10] ^= 1;
    assert!(peer.open_provisional(&bad).is_err());
    // Re-sign an invalid state with the genuinely authorized seed. Signature
    // validity must not replace deterministic transition validation.
    let (core, state, attachments, _) = components(a.record()).unwrap();
    let mut state = state.to_vec();
    state[176] = 1;
    let mut core = core.to_vec();
    let n = core.len();
    core[n - 32..].copy_from_slice(&hash(&cce("aven-e2ee/v1/membership/state", &[&state])));
    let sig = SigningKey::from_bytes(f.seed.signing.expose())
        .sign(&cce("aven-e2ee/v1/membership/sign", &[&core, attachments]));
    let mut raw = vec![1];
    for part in [&core[..], &state, attachments, &sig.to_bytes()] {
        bytes(&mut raw, part);
    }
    assert!(
        Admission::from_record(f.seed.genesis(), &f.publication, &d, peer.request(), &raw).is_err()
    );
    assert!(seal(&inv, &[0; 32], b"info", b"aad", b"plain").is_err());
    for end in 0..DECLARATION_BYTES {
        assert!(
            Declaration::from_record(f.seed.genesis(), &f.publication, &d.record()[..end]).is_err()
        );
    }
    for end in 0..REQUEST_BYTES {
        assert!(request_parts(&peer.request()[..end], &d.handle()).is_err());
    }
    for index in 0..ADMISSION_BYTES {
        let mut raw = a.record().to_vec();
        raw[index] ^= 1;
        assert!(
            Admission::from_record(f.seed.genesis(), &f.publication, &d, peer.request(), &raw)
                .is_err()
        );
    }
    // A genuinely authorized seed can sign a decryptable but wrong key grant.
    // The keyless server cannot detect this; peer readiness must detect it.
    let (core, state, attachments, _) = components(a.record()).unwrap();
    let recipient = peer.recipient().unwrap();
    let (enc, cipher) = grant_parts(attachments, &recipient).unwrap();
    let info = cce(
        "aven-e2ee/v1/pairing/grant",
        &[&peer.vault(), &peer.handle(), &hash(peer.request())],
    );
    let mut plain = open(&inv, &peer.recipient, &info, core, enc, cipher).unwrap();
    plain[394] ^= 1;
    let (enc, cipher) = seal(&inv, &recipient.hpke, &info, core, &plain).unwrap();
    let mut grant = vec![1];
    bytes(&mut grant, &enc);
    bytes(&mut grant, &cipher);
    let mut attachments = attachments.to_vec();
    attachments[85..].copy_from_slice(&grant);
    let sig = SigningKey::from_bytes(f.seed.signing.expose())
        .sign(&cce("aven-e2ee/v1/membership/sign", &[core, &attachments]));
    let mut raw = vec![1];
    for part in [core, state, &attachments, &sig.to_bytes()] {
        bytes(&mut raw, part);
    }
    Admission::from_record(f.seed.genesis(), &f.publication, &d, peer.request(), &raw).unwrap();
    let mut bad = evidence.clone();
    bad.admission = raw;
    assert!(peer.verify_enrollment(&bad, &f.package.descriptor).is_err());
}

#[tokio::test]
async fn real_admission_current_authority_and_old_outcome_are_separate() {
    let f = Fixture::new().await;
    let (_, d, peer, a) = f.prepare();
    f.post(&d, &peer).await;
    let wrong = Secret::new([9; 32]);
    let mut auth = f.auth();
    auth.bearer = &wrong;
    assert!(f.db.admit_first_peer(&auth, a.record()).await.is_err());
    let evidence = f.db.admit_first_peer(&f.auth(), a.record()).await.unwrap();
    peer.verify_enrollment(&evidence, &f.package.descriptor)
        .unwrap();
    assert_eq!(
        f.db.admit_first_peer(&f.auth(), a.record())
            .await
            .unwrap()
            .admission,
        a.record()
    );
    let mut wrong_context = f.auth();
    wrong_context.head = [99; 32];
    assert!(
        f.db.admit_first_peer(&wrong_context, a.record())
            .await
            .is_err()
    );
    let mut wrong_version = f.auth();
    wrong_version.credential_version = 2;
    assert!(
        f.db.admit_first_peer(&wrong_version, a.record())
            .await
            .is_err()
    );
    let pa = Authentication {
        vault: peer.vault(),
        genesis: f.seed.genesis().commitment(),
        device: peer.device(),
        credential_version: 1,
        head: a.commitment(),
        bearer: peer.bearer(),
    };
    assert_eq!(
        f.db.peer_enrollment_descriptor(&pa).await.unwrap(),
        f.package.descriptor
    );
    assert!(f.db.peer_enrollment_descriptor(&f.auth()).await.is_err());
    let seed_auth = staging::Authentication {
        vault_id: peer.vault(),
        genesis_commitment: f.seed.genesis().commitment(),
        bearer: f.seed.bearer(),
    };
    assert!(matches!(
        f.db.bootstrap_staging_status(&seed_auth, f.publication.binding().bootstrap_id)
            .await
            .unwrap(),
        staging::Status::Published(_)
    ));
    let mut conn = f.db.acquire_writer().await.unwrap();
    let count: i64 = sqlx::query_scalar("SELECT high_water FROM server_e2ee_allocator")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(count, 0);
    sqlx::query("UPDATE server_e2ee_membership_head SET sequence=3,commitment=?")
        .bind([8_u8; 32].as_slice())
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let error =
        f.db.admit_first_peer(&f.auth(), a.record())
            .await
            .err()
            .unwrap()
            .to_string();
    assert!(error.contains("membership-unsupported"));
    assert!(f.db.peer_enrollment_descriptor(&pa).await.is_err());
    assert!(
        f.db.bootstrap_staging_status(&seed_auth, f.publication.binding().bootstrap_id)
            .await
            .is_err()
    );
    // This is unknown-head injection, not a removal implementation or test.
    let mut conn = f.db.acquire_reader().await.unwrap();
    let saved: Vec<u8> = sqlx::query_scalar("SELECT admission FROM server_peer_invitation")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(saved, a.record());
}

#[tokio::test]
async fn expiry_is_durable_and_first_peer_slot_is_not_product_policy() {
    let f = Fixture::new().await;
    let (_, d, peer, a) = f.prepare();
    f.post(&d, &peer).await;
    assert!(
        f.db.admit_first_peer_at(&f.auth(), a.record(), d.expiry as i64)
            .await
            .is_err()
    );
    assert!(
        f.db.admit_first_peer_at(&f.auth(), a.record(), d.expiry as i64 - 100)
            .await
            .is_err()
    );
    let (_, other) = f
        .seed
        .prepare_peer_invitation(&f.publication, d.expiry)
        .unwrap();
    let error =
        f.db.register_peer_invitation(&f.auth(), other.record())
            .await
            .unwrap_err()
            .to_string();
    assert!(error.contains("first-peer-subset-slot-retained"));
    let db = Database::open(f.db.path()).await.unwrap();
    assert!(
        db.admit_first_peer_at(&f.auth(), a.record(), 1)
            .await
            .is_err()
    );
    let mut conn = db.acquire_reader().await.unwrap();
    let expired: bool = sqlx::query_scalar("SELECT expired FROM server_peer_invitation")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert!(expired);
}

#[tokio::test]
async fn atomic_admission_faults_and_independent_pool_retry() {
    let f = Fixture::new().await;
    let (_, d, peer, a) = f.prepare();
    f.post(&d, &peer).await;
    for table in ["server_peer_invitation", "server_e2ee_membership_head"] {
        let column = if table == "server_peer_invitation" {
            "admission"
        } else {
            "commitment"
        };
        let mut conn = f.db.acquire_writer().await.unwrap();
        sqlx::query(sqlx::AssertSqlSafe(format!("CREATE TRIGGER fail_{table} BEFORE UPDATE OF {column} ON {table} BEGIN SELECT RAISE(ABORT,'injected'); END"))).execute(&mut *conn).await.unwrap();
        drop(conn);
        assert!(f.db.admit_first_peer(&f.auth(), a.record()).await.is_err());
        let mut conn = f.db.acquire_writer().await.unwrap();
        let saved: Option<Vec<u8>> =
            sqlx::query_scalar("SELECT admission FROM server_peer_invitation")
                .fetch_one(&mut *conn)
                .await
                .unwrap();
        assert!(saved.is_none());
        let seq: i64 = sqlx::query_scalar("SELECT sequence FROM server_e2ee_membership_head")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(seq, 1);
        sqlx::query(sqlx::AssertSqlSafe(format!("DROP TRIGGER fail_{table}")))
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let other = Database::open(&f.dir.path().join("server.db"))
        .await
        .unwrap();
    let auth = f.auth();
    let (left, right) = tokio::join!(
        f.db.admit_first_peer(&auth, a.record()),
        other.admit_first_peer(&auth, a.record())
    );
    assert_eq!(left.unwrap().admission, right.unwrap().admission);
}

#[tokio::test]
async fn expired_first_registration_is_a_refusal_fence_not_a_renewable_invitation() {
    let f = Fixture::new().await;
    let (_, d, peer, _) = f.prepare();
    assert_eq!(
        f.db.register_peer_invitation_at(&f.auth(), d.record(), d.expiry as i64 + 1)
            .await
            .unwrap(),
        RegistrationStatus::Expired
    );
    assert_eq!(
        f.db.register_peer_invitation_at(&f.auth(), d.record(), 1)
            .await
            .unwrap(),
        RegistrationStatus::Expired
    );
    assert!(
        f.db.post_peer_request(peer.vault(), peer.handle(), peer.request())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn distinct_scanners_and_signed_candidates_cannot_replace_the_winner() {
    let f = Fixture::new().await;
    let (inv, d, peer, a) = f.prepare();
    f.post(&d, &peer).await;
    let other_peer = PeerAuthority::generate(
        Invitation::from_protected_storage(&inv.protected_storage_bytes()).unwrap(),
    )
    .unwrap();
    assert!(
        f.db.post_peer_request(
            other_peer.vault(),
            other_peer.handle(),
            other_peer.request()
        )
        .await
        .is_err()
    );
    // Two cryptographically valid different candidates exercise server CAS, not
    // permission for a host to regenerate its retained candidate on retry.
    let other = f
        .seed
        .prepare_peer_admission(&f.publication, &d, &inv, peer.request(), &f.key)
        .unwrap();
    assert_ne!(a.record(), other.record());
    let db = Database::open(f.db.path()).await.unwrap();
    let auth = f.auth();
    let (left, right) = tokio::join!(
        f.db.admit_first_peer(&auth, a.record()),
        db.admit_first_peer(&auth, other.record())
    );
    assert_ne!(left.is_ok(), right.is_ok());
    let winner = left.or(right).unwrap();
    let saved =
        f.db.peer_mailbox(peer.vault(), peer.handle())
            .await
            .unwrap()
            .evidence
            .unwrap();
    assert_eq!(winner.admission, saved.admission);
}

// Fixed test entropy only. Production always uses a fresh fallible OS seed.
fn fixture_seal(
    inv: &Invitation,
    recipient: &[u8; 32],
    info: &[u8],
    aad: &[u8],
    plaintext: &[u8],
    seed: u8,
) -> (Vec<u8>, Vec<u8>) {
    let handle = inv.handle();
    let mode = OpModeS::Psk(PskBundle::new(inv.psk.expose(), &handle).unwrap());
    let pk = <Kem as hpke::Kem>::PublicKey::from_bytes(recipient).unwrap();
    let (enc, cipher) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
        &mode,
        &pk,
        info,
        plaintext,
        aad,
        &mut ChaCha20Rng::from_seed([seed; 32]),
    )
    .unwrap();
    (enc.to_bytes().to_vec(), cipher)
}
#[test]
fn fixed_first_peer_byte_regression_preserves_existing_anchors() {
    let seed = super::super::publication::tests::seed();
    let g = seed.genesis();
    let descriptor = super::super::publication::tests::descriptor(g);
    let binding = PublicationBinding::from_descriptor(g, &descriptor).unwrap();
    let (core, state, attachments) = super::super::publication::components(g, &binding);
    let publication = Publication::from_record(
        g,
        &descriptor,
        &super::super::publication::tests::signed(&seed, &core, &state, &attachments),
    )
    .unwrap();
    let inv = Invitation {
        vault: g.context.vault_id,
        inviter: g.hpke_public,
        psk: Secret::new([31; 32]),
    };
    let mut body = b"AVID\0\x01\x01".to_vec();
    for v in [
        inv.vault,
        g.commitment(),
        publication.commitment(),
        g.device,
        inv.inviter,
        inv.handle(),
    ] {
        body.extend(v);
    }
    body.extend(2_000_000_000_u64.to_be_bytes());
    let signer = SigningKey::from_bytes(seed.signing.expose());
    let signature = signer.sign(&cce("aven-e2ee/v1/pairing/declaration", &[&body]));
    let mut raw = vec![1];
    bytes(&mut raw, &body);
    bytes(&mut raw, &signature.to_bytes());
    let d = Declaration::from_record(g, &publication, &raw).unwrap();
    let (private, _) = <Kem as hpke::Kem>::derive_keypair(&[34; 32]);
    let mut peer = PeerAuthority {
        device: [32; 32],
        signing: Secret::new([33; 32]),
        recipient: Secret::new(private.to_bytes().into()),
        bearer: Secret::new([35; 32]),
        invitation: Invitation::from_protected_storage(&inv.protected_storage_bytes()).unwrap(),
        request: vec![],
    };
    let row = peer.recipient().unwrap();
    let info = cce("aven-e2ee/v1/pairing/request", &[&inv.vault, &inv.handle()]);
    let aad = cce(
        "aven-e2ee/v1/pairing/request-aad",
        &[&inv.vault, &inv.handle(), &inv.inviter],
    );
    let (enc, cipher) = fixture_seal(&inv, &inv.inviter, &info, &aad, &row.plaintext(), 36);
    let mut request = vec![1];
    bytes(&mut request, &inv.handle());
    bytes(&mut request, &enc);
    bytes(&mut request, &cipher);
    peer.request = request;
    let json: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/genesis.json")).unwrap();
    let key = LocalSharedStatePackageKey::new(
        hex::decode(json["generation_secret"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap(),
    );
    let a = seed
        .prepare_peer_admission(&publication, &d, &inv, peer.request(), &key)
        .unwrap();
    let evidence = Evidence {
        genesis: g.record().to_vec(),
        publication: publication.record().to_vec(),
        declaration: d.record().to_vec(),
        request: peer.request().to_vec(),
        admission: a.record().to_vec(),
    };
    let provisional = peer.open_provisional(&evidence).unwrap();
    let (core, state, attachments, _) = components(a.record()).unwrap();
    let info = cce(
        "aven-e2ee/v1/pairing/grant",
        &[&inv.vault, &inv.handle(), &hash(peer.request())],
    );
    let (enc, cipher) = fixture_seal(&inv, &row.hpke, &info, core, &provisional.plaintext, 37);
    let mut grant = vec![1];
    bytes(&mut grant, &enc);
    bytes(&mut grant, &cipher);
    let mut attachments = attachments.to_vec();
    attachments[85..].copy_from_slice(&grant);
    let signature = signer.sign(&cce("aven-e2ee/v1/membership/sign", &[core, &attachments]));
    let mut raw = vec![1];
    for part in [core, state, &attachments, &signature.to_bytes()] {
        bytes(&mut raw, part);
    }
    let mut fixed = evidence;
    fixed.admission = raw;
    peer.verify_enrollment(&fixed, &descriptor).unwrap();
    assert_eq!(
        hex::encode(hash(&fixed.admission)),
        "6f56adb3fee2b147f29fd2061ff8a83388bcef09ffec78fbf935c8be18053169"
    );
}
