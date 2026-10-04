use super::*;

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
