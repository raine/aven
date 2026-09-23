use super::*;
use crate::db::Database;

fn fixture(name: &str) -> Vec<u8> {
    let json: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/genesis.json")).unwrap();
    hex::decode(json[name].as_str().unwrap()).unwrap()
}

fn array(name: &str) -> [u8; 32] {
    fixture(name).try_into().unwrap()
}

fn context() -> LocalSharedStatePackageContext {
    LocalSharedStatePackageContext {
        vault_id: array("vault"),
        generation_id: array("generation"),
    }
}

fn key() -> LocalSharedStatePackageKey {
    LocalSharedStatePackageKey::new(array("generation_secret"))
}

fn authority() -> SeedAuthority {
    let mut bytes = Vec::new();
    for name in ["signing_seed", "hpke_private", "token", "record"] {
        bytes.extend(fixture(name));
    }
    SeedAuthority::from_protected_storage(&bytes, context(), &key()).unwrap()
}

fn operator() -> (Secret, SetupAuthority) {
    let secret = Secret::new([0x91; 32]);
    let authority = SetupAuthority::from_verifier(
        array("setup"),
        SetupAuthority::verifier(array("setup"), &secret),
    );
    (secret, authority)
}

struct FixedEntropy([u8; 32]);
impl rand_core::TryRng for FixedEntropy {
    type Error = rand_core::Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        panic!("unexpected entropy request")
    }
    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        panic!("unexpected entropy request")
    }
    fn try_fill_bytes(&mut self, out: &mut [u8]) -> Result<(), Self::Error> {
        assert_eq!(out.len(), 32);
        out.copy_from_slice(&self.0);
        Ok(())
    }
}
impl rand_core::TryCryptoRng for FixedEntropy {}

#[test]
fn frozen_bytes_match_reviewed_crypto_fixture() {
    let (recipient, _) = <Kem as hpke::Kem>::derive_keypair(&array("hpke_ikm"));
    let seed = SeedAuthority::build(
        context(),
        &key(),
        array("setup"),
        array("claim"),
        array("device"),
        Secret::new(array("signing_seed")),
        recipient,
        Secret::new(array("token")),
        &mut FixedEntropy(array("encapsulation_entropy")),
    )
    .unwrap();
    assert_eq!(seed.genesis.record().as_slice(), fixture("record"));
    assert_eq!(seed.genesis.claim_bytes(), fixture("request"));
    assert_eq!(seed.genesis.commitment(), array("record_commitment"));
    assert_eq!(seed.genesis.state(), fixture("state"));
    assert_eq!(seed.genesis.core(&seed.genesis.state()), fixture("core"));
    assert_eq!(seed.genesis.info(), fixture("hpke_info"));
    assert_eq!(
        &*self_plaintext(&seed.genesis, key().protected_storage_bytes()),
        &fixture("self_plaintext")
    );
    assert_eq!(
        seed.protected_storage_bytes(),
        authority().protected_storage_bytes()
    );
}

#[test]
fn bounded_parser_rejects_every_truncation_and_mutation() {
    let record = fixture("record");
    for i in 0..record.len() {
        assert!(Genesis::from_record(&record[..i]).is_err());
        let mut bad = record.clone();
        bad[i] ^= 1;
        assert!(Genesis::from_record(&bad).is_err(), "byte {i}");
    }
    let mut extra = record.clone();
    extra.push(0);
    assert!(Genesis::from_record(&extra).is_err());
    let request = fixture("request");
    for i in 0..request.len() {
        assert!(codec::claim_record(&request[..i]).is_err());
    }
    let mut huge = request;
    huge[6..10].fill(255);
    assert!(codec::claim_record(&huge).is_err());
}

// Recompute the state hash and sign the modified transcript with the real seed.
// These are valid signatures over invalid semantics, not tamper-only negatives.
fn resign(core: &[u8], state: &[u8], attachments: &[u8]) -> Vec<u8> {
    let mut core = core.to_vec();
    core[197..229].copy_from_slice(&hash(&cce("aven-e2ee/v1/membership/state", &[state])));
    let signature = SigningKey::from_bytes(&array("signing_seed"))
        .sign(&cce("aven-e2ee/v1/membership/sign", &[&core, attachments]));
    let mut record = vec![1];
    for part in [&core[..], state, attachments, &signature.to_bytes()] {
        bytes(&mut record, part);
    }
    record
}

#[test]
fn resigned_invalid_genesis_semantics_are_rejected() {
    let state = fixture("state");
    let core = fixture("core");
    let attachments = fixture("attachments");
    // Version/suite, bootstrap/pending/recovery flags, counts, credential version,
    // admission mismatch, generation count and initial eligibility boundary.
    for offset in [4, 6, 39, 40, 41, 42, 174, 175, 207, 279] {
        let mut bad = state.clone();
        bad[offset] ^= 1;
        let record = resign(&core, &bad, &attachments);
        assert!(Genesis::from_record(&record).is_err(), "state {offset}");
    }
    // Core version, vault, membership sequence, predecessor, signer kind/ID,
    // action, action profile, and claim binding.
    for offset in [0, 5, 44, 49, 81, 86, 118, 127, 161] {
        let mut bad = core.clone();
        bad[offset] ^= 1;
        assert!(
            Genesis::from_record(&resign(&bad, &state, &attachments)).is_err(),
            "core {offset}"
        );
    }
    // Recipient count/kind/purpose, wrong device and wrong recipient public key.
    for offset in [6, 7, 8, 13, 49] {
        let mut bad = attachments.clone();
        bad[offset] ^= 1;
        assert!(
            Genesis::from_record(&resign(&core, &state, &bad)).is_err(),
            "attachment {offset}"
        );
    }
}

#[test]
fn protected_validation_checks_secrets_and_opaque_self_coverage() {
    let seed = authority();
    let saved = seed.protected_storage_bytes();
    for offset in [0, 33, 64] {
        let mut wrong = saved.clone();
        wrong[offset] ^= 1;
        assert!(SeedAuthority::from_protected_storage(&wrong, context(), &key()).is_err());
    }
    let mut wrong_context = context();
    wrong_context.generation_id[0] ^= 1;
    assert!(SeedAuthority::from_protected_storage(&saved, wrong_context, &key()).is_err());
    assert!(
        SeedAuthority::from_protected_storage(
            &saved,
            context(),
            &LocalSharedStatePackageKey::new([99; 32])
        )
        .is_err()
    );

    let core = fixture("core");
    let mut att = fixture("attachments");
    att[121] ^= 1;
    let bad = resign(&core, &fixture("state"), &att);
    // A keyless server cannot establish HPKE plaintext validity.
    assert!(Genesis::from_record(&bad).is_ok());
    let mut saved = saved.clone();
    saved[96..].copy_from_slice(&bad);
    assert!(SeedAuthority::from_protected_storage(&saved, context(), &key()).is_err());

    // A validly encrypted and signed self package with the wrong secret also fails.
    let private = HpkePrivate::from_bytes(&fixture("hpke_private")).unwrap();
    let public = <Kem as hpke::Kem>::sk_to_pk(&private);
    let wrong_plaintext = self_plaintext(seed.genesis(), &[99; 32]);
    let (enc, ct) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
        &OpModeS::Base,
        &public,
        &seed.genesis.info(),
        &wrong_plaintext,
        &core,
        &mut FixedEntropy([7; 32]),
    )
    .unwrap();
    att[85..117].copy_from_slice(&enc.to_bytes());
    att[121..].copy_from_slice(&ct);
    let bad = resign(&core, &fixture("state"), &att);
    assert!(Genesis::from_record(&bad).is_ok());
    saved[96..].copy_from_slice(&bad);
    assert!(SeedAuthority::from_protected_storage(&saved, context(), &key()).is_err());
}

#[test]
fn weak_signatures_and_invalid_dh_fail_in_libraries() {
    let mut record = fixture("record");
    record[870..].fill(255);
    assert!(Genesis::from_record(&record).is_err());
    let mut state = fixture("state");
    state[75..107].fill(0);
    state[75] = 1;
    assert!(
        Genesis::from_record(&resign(&fixture("core"), &state, &fixture("attachments"))).is_err()
    );
    for pk in [[0; 32], {
        let mut x = [0; 32];
        x[0] = 1;
        x
    }] {
        let public = <Kem as hpke::Kem>::PublicKey::from_bytes(&pk).unwrap();
        assert!(
            hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
                &OpModeS::Base,
                &public,
                b"info",
                b"plaintext",
                b"aad",
                &mut FixedEntropy([3; 32])
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn setup_authorization_resume_restart_and_divergent_bindings() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("server.sqlite");
    let db = Database::open(&path).await.unwrap();
    let seed = authority();
    let request = seed.genesis.claim_bytes();
    let (secret, operator) = operator();
    for authentication in [
        ClaimAuthentication::SeedBearer(seed.bearer()),
        ClaimAuthentication::SetupSecret(&Secret::new([0; 32])),
    ] {
        assert!(
            db.admit_seed_claim(&request, Some(&operator), authentication)
                .await
                .is_err()
        );
    }
    let wrong_setup =
        SetupAuthority::from_verifier([3; 32], SetupAuthority::verifier([3; 32], &secret));
    assert!(
        db.admit_seed_claim(
            &request,
            Some(&wrong_setup),
            ClaimAuthentication::SetupSecret(&secret)
        )
        .await
        .is_err()
    );
    let result = db
        .admit_seed_claim(
            &request,
            Some(&operator),
            ClaimAuthentication::SetupSecret(&secret),
        )
        .await
        .unwrap();
    result.validate_pinned(seed.genesis()).unwrap();
    drop(db);
    // The prior response could have been lost. Reopen with no operator config.
    let db = Database::open(&path).await.unwrap();
    assert_eq!(
        result,
        db.admit_seed_claim(
            &request,
            None,
            ClaimAuthentication::SeedBearer(seed.bearer())
        )
        .await
        .unwrap()
    );
    assert_eq!(
        result,
        db.admit_seed_claim(
            &request,
            Some(&operator),
            ClaimAuthentication::SetupSecret(&secret)
        )
        .await
        .unwrap()
    );
    assert!(
        db.admit_seed_claim(
            &request,
            Some(&operator),
            ClaimAuthentication::SetupSecret(&Secret::new([0; 32]))
        )
        .await
        .is_err()
    );
    assert!(
        db.admit_seed_claim(
            &request,
            None,
            ClaimAuthentication::SeedBearer(&Secret::new([0; 32]))
        )
        .await
        .is_err()
    );

    let (core, state, att, _) = codec::components(seed.genesis.record()).unwrap();
    for field in ["claim", "verifier", "ciphertext", "setup"] {
        let mut c = core.to_vec();
        let mut s = state.to_vec();
        let mut a = att.to_vec();
        match field {
            "claim" => {
                c[161] ^= 1;
                s[175] ^= 1;
            }
            "verifier" => s[139] ^= 1,
            "ciphertext" => a[121] ^= 1,
            "setup" => c[129] ^= 1,
            _ => unreachable!(),
        }
        let divergent = Genesis::from_record(&resign(&c, &s, &a)).unwrap();
        let error = db
            .admit_seed_claim(
                &divergent.claim_bytes(),
                Some(&operator),
                ClaimAuthentication::SetupSecret(&secret),
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("conflict"), "{field}: {error}");
    }
    let other = SeedAuthority::generate(context(), &key(), array("setup")).unwrap();
    assert!(
        db.admit_seed_claim(
            &other.genesis.claim_bytes(),
            None,
            ClaimAuthentication::SeedBearer(other.bearer())
        )
        .await
        .is_err()
    );
    assert_eq!(
        result,
        db.admit_seed_claim(
            &request,
            None,
            ClaimAuthentication::SeedBearer(seed.bearer())
        )
        .await
        .unwrap()
    );
}

#[tokio::test]
async fn two_connections_compete_and_failed_insert_rolls_back() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("server.sqlite");
    let db = Database::open(&path).await.unwrap();
    let seed = authority();
    let (secret, operator) = operator();
    let request = seed.genesis.claim_bytes();
    {
        let mut conn = db.acquire_writer().await.unwrap();
        sqlx::query("CREATE TRIGGER fail_claim AFTER INSERT ON server_seed_claim BEGIN SELECT RAISE(ABORT, 'injected claim failure'); END")
            .execute(&mut *conn).await.unwrap();
    }
    assert!(
        db.admit_seed_claim(
            &request,
            Some(&operator),
            ClaimAuthentication::SetupSecret(&secret)
        )
        .await
        .is_err()
    );
    {
        let mut conn = db.acquire_writer().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM server_seed_claim")
            .fetch_one(&mut *conn)
            .await
            .unwrap();
        assert_eq!(count, 0);
        sqlx::query("DROP TRIGGER fail_claim")
            .execute(&mut *conn)
            .await
            .unwrap();
    }
    let second = Database::open(&path).await.unwrap();
    let competitor = SeedAuthority::generate(context(), &key(), array("setup")).unwrap();
    let competing_request = competitor.genesis.claim_bytes();
    let (a, b) = tokio::join!(
        db.admit_seed_claim(
            &request,
            Some(&operator),
            ClaimAuthentication::SetupSecret(&secret)
        ),
        second.admit_seed_claim(
            &competing_request,
            Some(&operator),
            ClaimAuthentication::SetupSecret(&secret)
        ),
    );
    assert_ne!(a.is_ok(), b.is_ok());
    let (winner, result) = if let Ok(result) = a {
        (&seed, result)
    } else {
        (&competitor, b.unwrap())
    };
    drop(db);
    drop(second);
    let reopened = Database::open(&path).await.unwrap();
    assert_eq!(
        result,
        reopened
            .admit_seed_claim(
                &winner.genesis.claim_bytes(),
                None,
                ClaimAuthentication::SeedBearer(winner.bearer())
            )
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn server_storage_exports_and_diagnostics_exclude_secrets() {
    let root = tempfile::tempdir().unwrap();
    let db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let seed = authority();
    let (setup, operator) = operator();
    let result = db
        .admit_seed_claim(
            &seed.genesis.claim_bytes(),
            Some(&operator),
            ClaimAuthentication::SetupSecret(&setup),
        )
        .await
        .unwrap();
    let exported =
        serde_json::to_vec(&db.export_data("2026-09-22T00:00:00Z".into()).await.unwrap()).unwrap();
    let debug = format!(
        "{seed:?} {result:?} {operator:?} {:?}",
        ClaimAuthentication::SetupSecret(&setup)
    );
    let mut conn = db.acquire_reader().await.unwrap();
    let stored: Vec<u8> = sqlx::query_scalar("SELECT genesis FROM server_seed_claim")
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(stored, seed.genesis.record());
    let mut surfaces = vec![exported, stored, debug.into_bytes()];
    for path in [
        db.path().to_path_buf(),
        std::path::PathBuf::from(format!("{}-wal", db.path().display())),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            surfaces.push(bytes);
        }
    }
    for secret in [
        fixture("signing_seed"),
        fixture("hpke_private"),
        fixture("token"),
        fixture("generation_secret"),
        setup.expose().to_vec(),
    ] {
        for bytes in &surfaces {
            assert!(!bytes.windows(32).any(|window| window == secret));
            assert!(
                !bytes
                    .windows(64)
                    .any(|window| window == hex::encode(&secret).as_bytes())
            );
        }
    }
}

#[tokio::test]
async fn committed_claim_resumes_after_process_exit_without_response() {
    let root = tempfile::tempdir().unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "sync::seed_claim::tests::claim_exit_worker",
            "--ignored",
        ])
        .env("AVEN_SEED_CLAIM_TEST_ROOT", root.path())
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(23),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let db = Database::open(&root.path().join("server.sqlite"))
        .await
        .unwrap();
    let seed = authority();
    let result = db
        .admit_seed_claim(
            &seed.genesis.claim_bytes(),
            None,
            ClaimAuthentication::SeedBearer(seed.bearer()),
        )
        .await
        .unwrap();
    result.validate_pinned(seed.genesis()).unwrap();
}

#[tokio::test]
#[ignore = "subprocess worker exits without destructors; invoked with an isolated test root"]
async fn claim_exit_worker() {
    let Some(root) = std::env::var_os("AVEN_SEED_CLAIM_TEST_ROOT") else {
        return;
    };
    let db = Database::open(&std::path::PathBuf::from(root).join("server.sqlite"))
        .await
        .unwrap();
    let (secret, operator) = operator();
    db.admit_seed_claim(
        &authority().genesis.claim_bytes(),
        Some(&operator),
        ClaimAuthentication::SetupSecret(&secret),
    )
    .await
    .unwrap();
    std::process::exit(23);
}
