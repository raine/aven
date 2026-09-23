use super::super::{peer, publication};
use super::*;
use hpke::PskBundle;

struct Fixture {
    seed: SeedAuthority,
    membership: Membership,
    key: LocalSharedStatePackageKey,
}
fn fixture() -> Fixture {
    let seed = publication::tests::seed();
    let descriptor = publication::tests::descriptor(seed.genesis());
    let binding = PublicationBinding::from_descriptor(seed.genesis(), &descriptor).unwrap();
    let (core, state, attachments) = publication::components(seed.genesis(), &binding);
    let record = publication::tests::signed(&seed, &core, &state, &attachments);
    let membership = Membership::from_publication(seed.genesis(), &descriptor, &record).unwrap();
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/genesis.json")).unwrap();
    let key = LocalSharedStatePackageKey::new(
        hex::decode(fixture["generation_secret"].as_str().unwrap())
            .unwrap()
            .try_into()
            .unwrap(),
    );
    Fixture {
        seed,
        membership,
        key,
    }
}
fn copy_invitation(inv: &Invitation) -> Invitation {
    Invitation::from_protected_storage(&inv.protected_storage_bytes()).unwrap()
}
fn seal_fixed(
    inv: &Invitation,
    recipient: &Hash,
    info: &[u8],
    aad: &[u8],
    plain: &[u8],
    seed: u8,
) -> (Vec<u8>, Vec<u8>) {
    let handle = inv.handle();
    let mode = OpModeS::Psk(PskBundle::new(inv.psk.expose(), &handle).unwrap());
    let public = <Kem as hpke::Kem>::PublicKey::from_bytes(recipient).unwrap();
    let (enc, ciphertext) = hpke::single_shot_seal_with_rng::<Aead, Kdf, Kem>(
        &mode,
        &public,
        info,
        plain,
        aad,
        &mut ChaCha20Rng::from_seed([seed; 32]),
    )
    .unwrap();
    (enc.to_bytes().to_vec(), ciphertext)
}
fn joiner(inv: &Invitation, seed: u8) -> Joiner {
    let (private, _) = <Kem as hpke::Kem>::derive_keypair(&[seed + 2; 32]);
    let mut peer = peer::PeerAuthority {
        device: [seed; 32],
        signing: Secret::new([seed + 1; 32]),
        recipient: Secret::new(private.to_bytes().into()),
        bearer: Secret::new([seed + 3; 32]),
        invitation: copy_invitation(inv),
        request: vec![],
    };
    let info = cce("aven-e2ee/v1/pairing/request", &[&inv.vault, &inv.handle()]);
    let aad = cce(
        "aven-e2ee/v1/pairing/request-aad",
        &[&inv.vault, &inv.handle(), &inv.inviter],
    );
    let (enc, ciphertext) = seal_fixed(
        inv,
        &inv.inviter,
        &info,
        &aad,
        &peer.recipient().unwrap().plaintext(),
        seed + 4,
    );
    let mut request = vec![1];
    for field in [&inv.handle()[..], &enc, &ciphertext] {
        bytes(&mut request, field);
    }
    peer.request = request;
    Joiner(peer)
}
fn reseal(
    m: &Membership,
    d: &Declaration,
    peer: &Joiner,
    signing: &Secret,
    plain: &[u8],
    seed: u8,
) -> Vec<u8> {
    let recipient = peer.0.recipient().unwrap();
    let (state, core) = admission::state_core(m, d, peer.request(), &recipient);
    let info = cce(
        "aven-e2ee/v1/pairing/grant",
        &[&peer.0.vault(), &d.handle, &hash(peer.request())],
    );
    let (enc, ciphertext) = seal_fixed(
        &peer.0.invitation,
        &recipient.hpke,
        &info,
        &core,
        plain,
        seed,
    );
    let mut grant = vec![2];
    bytes(&mut grant, &enc);
    bytes(&mut grant, &ciphertext);
    let mut attachments = b"AVGA\0\x04\x01\x01\x02".to_vec();
    bytes(&mut attachments, &recipient.device);
    bytes(&mut attachments, &recipient.hpke);
    bytes(&mut attachments, &grant);
    admission::signed(signing, &core, &state, &attachments)
}
fn fixed_admission(
    m: &Membership,
    d: &Declaration,
    peer: &Joiner,
    signing: &Secret,
    key: &LocalSharedStatePackageKey,
) -> Vec<u8> {
    let plain = admission::grant_plaintext(m, d, peer.request(), &peer.0.recipient().unwrap(), key);
    reseal(m, d, peer, signing, &plain, 90)
}
fn first(f: &Fixture) -> (Invitation, Declaration, Joiner, Vec<u8>, Membership) {
    let (inv, d) = Device::seed(&f.seed)
        .invitation_with_psk(&f.membership, 2_000_000_000, Secret::new([31; 32]))
        .unwrap();
    let peer = joiner(&inv, 32);
    let raw = fixed_admission(&f.membership, &d, &peer, &f.seed.signing, &f.key);
    let next = f
        .membership
        .append(d.record(), peer.request(), &raw)
        .unwrap();
    (inv, d, peer, raw, next)
}

#[test]
fn exact_new_formats_from_sequence_two_and_peer_invited_third() {
    let f = fixture();
    let (_, d, peer, raw, next) = first(&f);
    assert_eq!(raw.len(), 1731);
    assert_eq!(next.sequence(), 2);
    assert_eq!(next.device_count(), 2);
    let verified = peer
        .verify_enrollment(&f.membership, d.record(), &raw)
        .unwrap();
    assert_eq!(
        verified.key().protected_storage_bytes(),
        f.key.protected_storage_bytes()
    );
    let (inv, third_d) = peer
        .authority()
        .invitation_with_psk(&next, 2_000_000_001, Secret::new([41; 32]))
        .unwrap();
    assert_ne!(inv.inviter, f.seed.genesis.hpke_public);
    let third = joiner(&inv, 42);
    let third_raw = fixed_admission(&next, &third_d, &third, &peer.0.signing, &f.key);
    let final_state = next
        .append(third_d.record(), third.request(), &third_raw)
        .unwrap();
    assert_eq!(third_raw.len(), 1895);
    assert_eq!(final_state.device_count(), 3);
    assert!(final_state.extends(verified.membership()));
    third
        .verify_enrollment(&next, third_d.record(), &third_raw)
        .unwrap();
    assert!(peer.verify_enrollment(&next, d.record(), &raw).is_err());
    let again = peer
        .verify_enrollment(&f.membership, d.record(), &raw)
        .unwrap();
    assert!(final_state.extends(again.membership()));
    assert_eq!(final_state.sequence(), 3);
    assert!(
        final_state
            .append(d.record(), peer.request(), &raw)
            .is_err()
    );
    let vector = serde_json::json!({
        "declaration":hex::encode(d.record()), "request":hex::encode(peer.request()),
        "admission":hex::encode(&raw), "admission_sha256":hex::encode(hash(&raw)),
        "third_declaration":hex::encode(third_d.record()), "third_request":hex::encode(third.request()),
        "third_admission":hex::encode(&third_raw), "third_sha256":hex::encode(hash(&third_raw))
    });
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/membership.json")).unwrap();
    assert_eq!(vector, expected);
}

#[test]
fn independent_random_clients_and_both_existing_inviters() {
    let f = fixture();
    let (inv, d) = Device::seed(&f.seed)
        .prepare_invitation(&f.membership, 20)
        .unwrap();
    let peer = Joiner::generate(copy_invitation(&inv)).unwrap();
    assert!(
        peer.authority()
            .prepare_invitation(&f.membership, 20)
            .is_err()
    );
    let raw = Device::seed(&f.seed)
        .prepare_admission(&f.membership, &d, &inv, peer.request(), &f.key)
        .unwrap();
    let next = peer
        .verify_enrollment(&f.membership, d.record(), &raw)
        .unwrap();
    for inviter in [Device::seed(&f.seed), peer.authority()] {
        let (inv, d) = inviter.prepare_invitation(next.membership(), 30).unwrap();
        let third = Joiner::generate(copy_invitation(&inv)).unwrap();
        let raw = inviter
            .prepare_admission(next.membership(), &d, &inv, third.request(), next.key())
            .unwrap();
        let verified = third
            .verify_enrollment(next.membership(), d.record(), &raw)
            .unwrap();
        assert_eq!(verified.membership().device_count(), 3);
        assert_ne!(third.bearer().expose(), peer.bearer().expose());
        assert_ne!(third.device(), peer.device());
    }
}

#[test]
fn declaration_anchor_and_same_recipient_cas_repreparation() {
    let f = fixture();
    let (inv, d, peer, losing, _) = first(&f);
    let (other_inv, other_d) = Device::seed(&f.seed)
        .prepare_invitation(&f.membership, 10)
        .unwrap();
    let other = Joiner::generate(copy_invitation(&other_inv)).unwrap();
    let winning = Device::seed(&f.seed)
        .prepare_admission(&f.membership, &other_d, &other_inv, other.request(), &f.key)
        .unwrap();
    let next = f
        .membership
        .append(other_d.record(), other.request(), &winning)
        .unwrap();
    assert!(next.append(d.record(), peer.request(), &losing).is_err());
    let replacement = Device::seed(&f.seed)
        .prepare_admission(&next, &d, &inv, peer.request(), &f.key)
        .unwrap();
    peer.verify_enrollment(&next, d.record(), &replacement)
        .unwrap();
    assert_ne!(hash(&losing), hash(&replacement));
    let loser_branch = f
        .membership
        .append(d.record(), peer.request(), &losing)
        .unwrap();
    assert!(!next.extends(&loser_branch) && !loser_branch.extends(&next));
    // A peer cannot claim it owned a declaration before its own admission.
    let (_, forged_d) = other.authority().prepare_invitation(&next, 100).unwrap();
    let mut body = forged_d.record()[5..212].to_vec();
    body[71..103].copy_from_slice(&f.membership.head());
    let sig = SigningKey::from_bytes(other.0.signing.expose())
        .sign(&cce("aven-e2ee/v1/pairing/declaration", &[&body]));
    let mut raw = vec![1];
    bytes(&mut raw, &body);
    bytes(&mut raw, &sig.to_bytes());
    assert!(Declaration::from_record(&next, &raw).is_err());
}

#[test]
fn strict_framing_every_mutation_and_truncation() {
    let f = fixture();
    let (_, d, peer, raw, _) = first(&f);
    for i in 0..raw.len() {
        assert!(
            f.membership
                .append(d.record(), peer.request(), &raw[..i])
                .is_err(),
            "truncated {i}"
        );
        let mut bad = raw.clone();
        bad[i] ^= 1;
        assert!(
            f.membership
                .append(d.record(), peer.request(), &bad)
                .is_err(),
            "mutated {i}"
        );
    }
    for i in 0..d.record().len() {
        assert!(Declaration::from_record(&f.membership, &d.record()[..i]).is_err());
        let mut bad = d.record().to_vec();
        bad[i] ^= 1;
        assert!(Declaration::from_record(&f.membership, &bad).is_err());
    }
    for i in 0..peer.request().len() {
        assert!(
            f.membership
                .append(d.record(), &peer.request()[..i], &raw)
                .is_err()
        );
        let mut bad = peer.request().to_vec();
        bad[i] ^= 1;
        assert!(f.membership.append(d.record(), &bad, &raw).is_err());
    }
    let mut extra = raw.clone();
    extra.push(0);
    assert!(
        f.membership
            .append(d.record(), peer.request(), &extra)
            .is_err()
    );
    assert!(
        f.membership
            .append(d.record(), peer.request(), &vec![0; MAX_RECORD_BYTES + 1])
            .is_err()
    );
}

#[test]
fn resigned_invalid_state_actions_packages_and_old_versions_refuse() {
    let f = fixture();
    let (_, d, peer, raw, _) = first(&f);
    let (core, state, attachments, _) = admission::components(&raw, 2).unwrap();
    // All state fields, including sorted rows, untouched predecessor authority,
    // bootstrap, generation and flags, must equal the derived result.
    for i in 0..state.len() {
        let mut state = state.to_vec();
        state[i] ^= 1;
        let mut core = core.to_vec();
        core[CORE_BYTES - 32..]
            .copy_from_slice(&hash(&cce("aven-e2ee/v1/membership/state", &[&state])));
        let bad = admission::signed(&f.seed.signing, &core, &state, attachments);
        assert!(
            f.membership
                .append(d.record(), peer.request(), &bad)
                .is_err(),
            "state {i}"
        );
    }
    for i in 0..core.len() {
        let mut core = core.to_vec();
        core[i] ^= 1;
        let bad = admission::signed(&f.seed.signing, &core, state, attachments);
        assert!(
            f.membership
                .append(d.record(), peer.request(), &bad)
                .is_err(),
            "core {i}"
        );
    }
    for i in 0..85 {
        let mut attachments = attachments.to_vec();
        attachments[i] ^= 1;
        let bad = admission::signed(&f.seed.signing, core, state, &attachments);
        assert!(
            f.membership
                .append(d.record(), peer.request(), &bad)
                .is_err(),
            "attachment {i}"
        );
    }
    let mut old_grant = attachments.to_vec();
    old_grant[85] = 1;
    assert!(
        f.membership
            .append(
                d.record(),
                peer.request(),
                &admission::signed(&f.seed.signing, core, state, &old_grant)
            )
            .is_err()
    );
}

#[test]
fn grant_semantics_and_psk_trust_are_not_keyless_server_acceptance() {
    let f = fixture();
    let (inv, d, peer, raw, _) = first(&f);
    let recipient = peer.0.recipient().unwrap();
    let plain = admission::grant_plaintext(&f.membership, &d, peer.request(), &recipient, &f.key);
    // Every field and byte of an authenticated but semantically wrong grant.
    for i in 0..plain.len() {
        let mut bad_plain = plain.clone();
        bad_plain[i] ^= 1;
        let bad = reseal(&f.membership, &d, &peer, &f.seed.signing, &bad_plain, 91);
        assert!(
            f.membership
                .append(d.record(), peer.request(), &bad)
                .is_ok()
        );
        assert!(
            peer.verify_enrollment(&f.membership, d.record(), &bad)
                .is_err(),
            "grant {i}"
        );
    }
    let mut wrong_psk = copy_invitation(&inv);
    wrong_psk.psk = Secret::new([80; 32]);
    let mut wrong = joiner(&wrong_psk, 32);
    wrong.0.request = peer.request().to_vec();
    assert!(wrong.open_provisional(d.record(), &raw).is_err());
    let stranger = joiner(&inv, 52);
    assert!(stranger.open_provisional(d.record(), &raw).is_err());
    let mut wrong_request = joiner(&inv, 32);
    wrong_request.0.request[50] ^= 1;
    assert!(wrong_request.open_provisional(d.record(), &raw).is_err());
    let mut wrong_private = joiner(&inv, 32);
    wrong_private.0.recipient = Secret::new([77; 32]);
    assert!(wrong_private.open_provisional(d.record(), &raw).is_err());
    assert!(
        Device::seed(&f.seed)
            .prepare_admission(
                &f.membership,
                &d,
                &inv,
                peer.request(),
                &LocalSharedStatePackageKey::new([7; 32])
            )
            .is_err()
    );
}

#[test]
fn duplicate_identity_keys_handles_and_resource_refusal_preserve_predecessor() {
    let f = fixture();
    let (_, _, peer, _, mut m) = first(&f);
    let before = m.head();
    let recipient = peer.0.recipient().unwrap();
    assert!(m.unique(&recipient, &[1; 32]).is_err());
    for field in 0..3 {
        let mut recipient = recipient.clone();
        recipient.device = [200; 32];
        recipient.sign = [201; 32];
        recipient.hpke = [202; 32];
        match field {
            0 => recipient.device = f.seed.genesis.device,
            1 => recipient.sign = f.seed.genesis.signing_public,
            _ => recipient.hpke = f.seed.genesis.hpke_public,
        }
        assert!(m.unique(&recipient, &[1; 32]).is_err());
    }
    let (inv, d) = Device::seed(&f.seed).prepare_invitation(&m, 0).unwrap();
    let next = Joiner::generate(copy_invitation(&inv)).unwrap();
    let raw = Device::seed(&f.seed)
        .prepare_admission(&m, &d, &inv, next.request(), &f.key)
        .unwrap();
    m.evidence_bytes = MAX_CHAIN_BYTES;
    assert!(m.append(d.record(), next.request(), &raw).is_err());
    assert_eq!(m.head(), before);
    // Actual signed additions reach the cap; no fixture-created member rows.
    let mut m = f.membership.clone();
    for count in 2..=MAX_DEVICES {
        let (inv, d) = Device::seed(&f.seed).prepare_invitation(&m, 0).unwrap();
        let peer = Joiner::generate(copy_invitation(&inv)).unwrap();
        let raw = Device::seed(&f.seed)
            .prepare_admission(&m, &d, &inv, peer.request(), &f.key)
            .unwrap();
        assert_eq!(raw.len(), 1403 + 164 * count);
        m = m.append(d.record(), peer.request(), &raw).unwrap();
    }
    assert_eq!(m.device_count(), MAX_DEVICES);
    assert!(Device::seed(&f.seed).prepare_invitation(&m, 0).is_err());
}

#[test]
fn key_ownership_and_low_order_recipient_fail_before_grant() {
    let f = fixture();
    let (inv, d, _, _, next) = first(&f);
    for kind in 0..3 {
        let mut peer = joiner(&inv, 32);
        match kind {
            0 => peer.0.signing = Secret::new([100; 32]),
            1 => peer.0.recipient = Secret::new([101; 32]),
            _ => peer.0.bearer = Secret::new([102; 32]),
        }
        assert!(peer.authority().prepare_invitation(&next, 100).is_err());
    }
    let peer = joiner(&inv, 32);
    let mut recipient = peer.0.recipient().unwrap();
    recipient.hpke = [0; 32];
    recipient.pop = SigningKey::from_bytes(peer.0.signing.expose())
        .sign(&recipient.pop_message(&inv.vault, &inv.handle(), &inv.inviter))
        .to_bytes();
    let info = cce("aven-e2ee/v1/pairing/request", &[&inv.vault, &inv.handle()]);
    let aad = cce(
        "aven-e2ee/v1/pairing/request-aad",
        &[&inv.vault, &inv.handle(), &inv.inviter],
    );
    let (enc, cipher) = seal_fixed(&inv, &inv.inviter, &info, &aad, &recipient.plaintext(), 80);
    let mut request = vec![1];
    for value in [&inv.handle()[..], &enc, &cipher] {
        bytes(&mut request, value);
    }
    assert!(
        Device::seed(&f.seed)
            .prepare_admission(&f.membership, &d, &inv, &request, &f.key)
            .is_err()
    );
}

#[test]
fn old_tags_cannot_be_resigned_into_the_new_profile() {
    let f = fixture();
    let (_, d, peer, raw, _) = first(&f);
    let (core, state, attachments, _) = admission::components(&raw, 2).unwrap();
    for kind in 0..4 {
        let mut core = core.to_vec();
        let mut state = state.to_vec();
        let mut attachments = attachments.to_vec();
        match kind {
            0 => {
                assert_eq!(core[128], 2);
                core[128] = 1;
            } // AVAD version.
            1 => state[5] = 3,
            2 => attachments[5] = 3,
            _ => attachments[85] = 1,
        }
        core[CORE_BYTES - 32..]
            .copy_from_slice(&hash(&cce("aven-e2ee/v1/membership/state", &[&state])));
        let bad = admission::signed(&f.seed.signing, &core, &state, &attachments);
        assert!(
            f.membership
                .append(d.record(), peer.request(), &bad)
                .is_err()
        );
    }
}
