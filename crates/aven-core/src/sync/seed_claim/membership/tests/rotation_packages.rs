use super::*;

#[test]
fn authenticated_wrong_rotation_plaintext_and_context_never_yield_coverage() {
    let f = fixture();
    let keys = f.membership.verify_initial_key(&f.key).unwrap();
    let freeze = Device::seed(&f.seed)
        .prepare_revoke(&f.membership, &[])
        .unwrap();
    let pending = f.membership.append(&[], &[], &freeze).unwrap();
    let raw = Device::seed(&f.seed)
        .rotation_with(
            &pending,
            &keys,
            [100; 32],
            &LocalSharedStatePackageKey::new([101; 32]),
            0,
            &mut ChaCha20Rng::from_seed([102; 32]),
        )
        .unwrap();
    let (core, state, attachments, _) = encoding::components(&raw).unwrap();
    let packages = encoding::read_packages(attachments, 0, 1, 176).unwrap();
    let p = &packages[0];
    let private = HpkePrivate::from_bytes(f.seed.recipient.expose()).unwrap();
    let enc = Enc::from_bytes(p.enc).unwrap();
    let info = cce(
        "aven-e2ee/v1/membership/rotation-key",
        &[&f.seed.genesis.context.vault_id, &[1], &p.device, &p.public],
    );
    let plain = hpke::single_shot_open::<Aead, Kdf, Kem>(
        &OpModeR::Base,
        &private,
        &enc,
        &info,
        p.cipher,
        core,
    )
    .unwrap();
    for index in 0..plain.len() {
        let mut plain = plain.clone();
        plain[index] ^= 1;
        let (enc, cipher) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
            &OpModeS::Base,
            &<Kem as hpke::Kem>::PublicKey::from_bytes(&p.public).unwrap(),
            &info,
            &plain,
            core,
            &mut ChaCha20Rng::from_seed([103; 32]),
        )
        .unwrap();
        let mut attachments = encoding::packages(1);
        encoding::package(
            &mut attachments,
            0,
            &p.device,
            &p.public,
            &enc.to_bytes(),
            &cipher,
        );
        let bad = admission::signed(&f.seed.signing, core, state, &attachments);
        pending.append(&[], &[], &bad).unwrap();
        assert!(
            Device::seed(&f.seed)
                .receive_rotation(&pending, &bad, &keys)
                .is_err(),
            "plaintext {index}"
        );
    }
    for kind in 0..5 {
        let mut info = info.clone();
        let mut aad = core.to_vec();
        let mut public = p.public;
        match kind {
            0 => {
                public = <Kem as hpke::Kem>::derive_keypair(&[90; 32])
                    .1
                    .to_bytes()
                    .into()
            }
            1 => info[4] ^= 1,
            2 => {
                let end = info.len();
                info[end - 1] ^= 1;
            }
            3 => aad[1] ^= 1,
            _ => {}
        }
        let mode = if kind == 4 {
            OpModeS::Psk(PskBundle::new(&[8; 32], &[9; 32]).unwrap())
        } else {
            OpModeS::Base
        };
        let (enc, cipher) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
            &mode,
            &<Kem as hpke::Kem>::PublicKey::from_bytes(&public).unwrap(),
            &info,
            &plain,
            &aad,
            &mut ChaCha20Rng::from_seed([104; 32]),
        )
        .unwrap();
        let mut attachments = encoding::packages(1);
        encoding::package(
            &mut attachments,
            0,
            &p.device,
            &p.public,
            &enc.to_bytes(),
            &cipher,
        );
        let bad = admission::signed(&f.seed.signing, core, state, &attachments);
        pending.append(&[], &[], &bad).unwrap();
        assert!(
            Device::seed(&f.seed)
                .receive_rotation(&pending, &bad, &keys)
                .is_err(),
            "context {kind}"
        );
    }
}

#[test]
fn mixed_evidence_replays_historical_enrollment_after_inviter_removal() {
    let f = fixture();
    let (_, d, peer, admission, m) = first(&f);
    let keys = m.verify_initial_key(&f.key).unwrap();
    let revoke = peer
        .authority()
        .prepare_revoke(&m, &[f.seed.genesis.device])
        .unwrap();
    let pending = m.append(&[], &[], &revoke).unwrap();
    let rotate = peer
        .authority()
        .prepare_rotation(&pending, &keys, 0)
        .unwrap();
    let mut evidence = Evidence {
        genesis: f.seed.genesis.record().to_vec(),
        publication: f.membership.publication.record().to_vec(),
        descriptor: publication::tests::descriptor(f.seed.genesis()),
        transitions: vec![
            EvidenceRecord {
                declaration: d.record().to_vec(),
                request: peer.request().to_vec(),
                record: admission.clone(),
            },
            EvidenceRecord {
                declaration: vec![],
                request: vec![],
                record: revoke,
            },
            EvidenceRecord {
                declaration: vec![],
                request: vec![],
                record: rotate,
            },
        ],
    };
    let bytes = serde_json::to_vec(&evidence).unwrap();
    let parsed = Evidence::decode(&bytes).unwrap();
    assert_eq!(parsed.verify().unwrap().sequence(), 4);
    assert_eq!(
        parsed
            .enrollment(&peer, hash(&admission))
            .unwrap()
            .checkpoint(),
        m.head()
    );
    evidence.transitions[2].declaration = d.record().to_vec();
    assert!(evidence.verify().is_err());
    // Bounded serde rejects per-record/count/aggregate excess before chain replay.
    let value = serde_json::to_value(&parsed).unwrap();
    let mut bad = value.clone();
    bad["transitions"][0]["record"] = serde_json::json!(vec![0; MAX_RECORD_BYTES + 1]);
    assert!(serde_json::from_value::<Evidence>(bad).is_err());
    let mut bad = value.clone();
    let entry = serde_json::json!({"declaration":[], "request":[], "record":[]});
    bad["transitions"] = serde_json::json!(vec![entry; MAX_TRANSITIONS + 1]);
    assert!(serde_json::from_value::<Evidence>(bad).is_err());
    let mut bad = value;
    let entry =
        serde_json::json!({"declaration":[], "request":[], "record": vec![0; MAX_RECORD_BYTES]});
    bad["transitions"] = serde_json::json!(vec![entry; MAX_CHAIN_BYTES / MAX_RECORD_BYTES + 1]);
    assert!(serde_json::from_value::<Evidence>(bad).is_err());
    assert!(Evidence::decode(&vec![b' '; MAX_EVIDENCE_JSON_BYTES + 1]).is_err());
}

#[test]
fn revoke_requires_sorted_active_targets_and_predecessor_authority() {
    let f = fixture();
    let (_, _, peer, _, m) = first(&f);
    let seed = Device::seed(&f.seed);
    assert!(
        seed.prepare_revoke(&m, &[peer.device(), peer.device()])
            .is_err()
    );
    assert!(seed.prepare_revoke(&m, &[[255; 32]]).is_err());
    let mut all = vec![peer.device(), f.seed.genesis.device];
    all.sort();
    assert!(seed.prepare_revoke(&m, &all).is_err());
    assert!(
        seed.prepare_revoke(&f.membership, &[f.seed.genesis.device])
            .is_err()
    );
    let raw = seed.prepare_revoke(&m, &[peer.device()]).unwrap();
    let next = m.append(&[], &[], &raw).unwrap();
    let auth = super::super::super::peer::Authentication {
        vault: f.seed.genesis.context.vault_id,
        genesis: f.seed.genesis.commitment(),
        device: peer.device(),
        credential_version: 1,
        head: m.head(),
        bearer: peer.bearer(),
    };
    assert!(next.authenticate(&auth, true).is_err());
    for i in 0..raw.len() {
        let mut bad = raw.clone();
        bad[i] ^= 1;
        assert!(m.append(&[], &[], &bad).is_err());
        assert!(m.append(&[], &[], &raw[..i]).is_err());
    }
    let (core, state, attachments, _) = encoding::components(&raw).unwrap();
    for i in 0..state.len() {
        let mut state = state.to_vec();
        state[i] ^= 1;
        let mut core = core.to_vec();
        let end = core.len();
        core[end - 32..].copy_from_slice(&hash(&cce("aven-e2ee/v1/membership/state", &[&state])));
        assert!(
            m.append(
                &[],
                &[],
                &admission::signed(&f.seed.signing, &core, &state, attachments)
            )
            .is_err()
        );
    }
    // A valid but different signer cannot inherit the named predecessor signer.
    assert!(
        m.append(
            &[],
            &[],
            &admission::signed(&peer.0.signing, core, state, attachments)
        )
        .is_err()
    );
}

#[test]
fn reserved_transition_slot_cannot_be_consumed_by_pending_management() {
    let f = fixture();
    let mut m = f.membership.clone();
    let keys = m.verify_initial_key(&f.key).unwrap();
    let (inv, d) = Device::seed(&f.seed).prepare_invitation(&m, 100).unwrap();
    for _ in 0..MAX_TRANSITIONS - 1 {
        let raw = Device::seed(&f.seed).prepare_revoke(&m, &[]).unwrap();
        m = m.append(&[], &[], &raw).unwrap();
    }
    assert!(Device::seed(&f.seed).prepare_revoke(&m, &[]).is_err());
    let error = Device::seed(&f.seed)
        .prepare_invitation(&m, 100)
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "error membership-change-limit");
    let peer = Joiner::generate(copy_invitation(&inv)).unwrap();
    assert!(
        Device::seed(&f.seed)
            .prepare_admission(&m, &d, &inv, peer.request(), &keys)
            .is_err()
    );
    let raw = Device::seed(&f.seed)
        .prepare_rotation(&m, &keys, 0)
        .unwrap();
    let next = m.append(&[], &[], &raw).unwrap();
    assert_eq!(next.sequence(), MAX_TRANSITIONS as u64 + 1);
    assert!(!next.rotation_pending());
    assert!(Device::seed(&f.seed).prepare_revoke(&next, &[]).is_err());
}

#[test]
fn signed_addition_cannot_reuse_retired_identity_or_keys() {
    let f = fixture();
    let (_, _, peer, _, m) = first(&f);
    let revoke = peer
        .authority()
        .prepare_revoke(&m, &[f.seed.genesis.device])
        .unwrap();
    let m = m.append(&[], &[], &revoke).unwrap();
    let (inv, d) = peer.authority().prepare_invitation(&m, 100).unwrap();
    let joiner = joiner(&inv, 80);
    for kind in 0..3 {
        let mut recipient = joiner.0.recipient().unwrap();
        let signing = match kind {
            0 => {
                recipient.device = f.seed.genesis.device;
                &joiner.0.signing
            }
            1 => {
                recipient.sign = f.seed.genesis.signing_public;
                &f.seed.signing
            }
            _ => {
                recipient.hpke = f.seed.genesis.hpke_public;
                &joiner.0.signing
            }
        };
        recipient.pop = SigningKey::from_bytes(signing.expose())
            .sign(&recipient.pop_message(&inv.vault, &inv.handle(), &inv.inviter))
            .to_bytes();
        let (enc, cipher) = seal_fixed(
            &inv,
            &inv.inviter,
            &cce("aven-e2ee/v1/pairing/request", &[&inv.vault, &inv.handle()]),
            &cce(
                "aven-e2ee/v1/pairing/request-aad",
                &[&inv.vault, &inv.handle(), &inv.inviter],
            ),
            &recipient.plaintext(),
            90,
        );
        let mut request = vec![1];
        for field in [&inv.handle()[..], &enc, &cipher] {
            bytes(&mut request, field);
        }
        recipient
            .verify(&inv.vault, &inv.handle(), &inv.inviter)
            .unwrap();
        let (state, core) = admission::state_core(&m, &d, &request, &recipient);
        let mut attachments = encoding::packages(1);
        encoding::package(
            &mut attachments,
            1,
            &recipient.device,
            &recipient.hpke,
            &[0; 32],
            &[0; 483],
        );
        let raw = admission::signed(&peer.0.signing, &core, &state, &attachments);
        assert!(
            m.append(d.record(), &request, &raw).is_err(),
            "retired {kind}"
        );
    }
}
