use super::*;

fn rotate_fixed(
    device: Device<'_>,
    m: &Membership,
    keys: &VerifiedKeys,
    id: u8,
    cutoff: u64,
) -> (Vec<u8>, Membership, VerifiedKeys) {
    let raw = device
        .rotation_with(
            m,
            keys,
            [id; 32],
            &LocalSharedStatePackageKey::new([id + 1; 32]),
            cutoff,
            &mut ChaCha20Rng::from_seed([id + 2; 32]),
        )
        .unwrap();
    let keys = device.receive_rotation(m, &raw, keys).unwrap();
    let next = m.append(&[], &[], &raw).unwrap();
    keys.validate(&next).unwrap();
    (raw, next, keys)
}
fn add_fixed(
    device: Device<'_>,
    m: &Membership,
    keys: &VerifiedKeys,
    seed: u8,
) -> (Declaration, Joiner, Vec<u8>, Membership) {
    let (inv, d) = device
        .invitation_with_psk(m, 100, Secret::new([seed; 32]))
        .unwrap();
    let peer = joiner(&inv, seed + 1);
    let plain =
        admission::grant_plaintext(m, &d, peer.request(), &peer.0.recipient().unwrap(), keys);
    let raw = reseal(m, &d, &peer, device.signing, &plain, seed + 5);
    let next = m.append(d.record(), peer.request(), &raw).unwrap();
    (d, peer, raw, next)
}
fn resign(
    seed: &Secret,
    raw: &[u8],
    change: impl FnOnce(&mut Vec<u8>, &mut Vec<u8>, &mut Vec<u8>),
) -> Vec<u8> {
    let (core, state, packages, _) = encoding::components(raw).unwrap();
    let (mut core, mut state, mut packages) = (core.to_vec(), state.to_vec(), packages.to_vec());
    change(&mut core, &mut state, &mut packages);
    let end = core.len();
    core[end - 32..].copy_from_slice(&hash(&cce("aven-e2ee/v1/membership/state", &[&state])));
    admission::signed(seed, &core, &state, &packages)
}

#[test]
fn exact_removal_rotation_empty_interval_and_fresh_join_vectors() {
    let f = fixture();
    let (_, _, peer, _, m) = first(&f);
    let keys = m.verify_initial_key(&f.key).unwrap();
    let revoke = peer
        .authority()
        .prepare_revoke(&m, &[f.seed.genesis.device])
        .unwrap();
    let pending = m.append(&[], &[], &revoke).unwrap();
    assert!(pending.rotation_pending());
    assert!(Device::seed(&f.seed).prepare_revoke(&pending, &[]).is_err());
    assert!(
        peer.authority()
            .prepare_revoke(&pending, &[peer.device()])
            .is_err()
    );
    assert!(
        pending
            .unique(
                &Recipient {
                    device: f.seed.genesis.device,
                    sign: [70; 32],
                    hpke: [71; 32],
                    verifier: [72; 32],
                    pop: [0; 64]
                },
                &[73; 32]
            )
            .is_err()
    );
    for which in 0..3 {
        let mut r = peer.0.recipient().unwrap();
        r.device = [210; 32];
        r.sign = [211; 32];
        r.hpke = [212; 32];
        match which {
            0 => r.device = f.seed.genesis.device,
            1 => r.sign = f.seed.genesis.signing_public,
            _ => r.hpke = f.seed.genesis.hpke_public,
        }
        assert!(pending.unique(&r, &[73; 32]).is_err());
    }
    let (rotation, rotated, keys2) = rotate_fixed(peer.authority(), &pending, &keys, 100, 10);
    assert!(!rotated.rotation_pending());
    assert!(rotated.generation_allows(f.seed.genesis.context.generation_id, 10));
    assert!(!rotated.generation_allows(f.seed.genesis.context.generation_id, 11));
    assert!(!rotated.generation_allows([100; 32], 10));
    assert!(rotated.generation_allows([100; 32], 11));
    assert!(!rotated.generation_allows([99; 32], 11));
    assert!(rotated.verify_initial_key(&f.key).is_err());
    let freeze2 = peer.authority().prepare_revoke(&rotated, &[]).unwrap();
    let pending2 = rotated.append(&[], &[], &freeze2).unwrap();
    let (rotation2, twice, keys3) = rotate_fixed(peer.authority(), &pending2, &keys2, 110, 10);
    assert!(!twice.generation_allows([100; 32], 10));
    assert!(!twice.generation_allows([100; 32], 11));
    assert!(twice.generation_allows([110; 32], 11));
    assert_eq!(
        keys3
            .key(f.seed.genesis.context.generation_id)
            .unwrap()
            .protected_storage_bytes(),
        f.key.protected_storage_bytes()
    );
    let (d, joiner, admission, joined) = add_fixed(peer.authority(), &twice, &keys3, 60);
    let verified = joiner
        .verify_enrollment(&twice, d.record(), &admission)
        .unwrap();
    verified.keys().validate(&joined).unwrap();
    assert_eq!(
        verified.key().protected_storage_bytes(),
        f.key.protected_storage_bytes()
    );
    assert_eq!(joined.publication().record(), m.publication().record());
    let vector = serde_json::json!({
        "revoke": hex::encode(&revoke), "revoke_sha256": hex::encode(hash(&revoke)),
        "rotation": hex::encode(&rotation), "rotation_sha256": hex::encode(hash(&rotation)),
        "freeze_again": hex::encode(&freeze2), "empty_rotation": hex::encode(&rotation2),
        "empty_rotation_sha256": hex::encode(hash(&rotation2)),
        "fresh_declaration": hex::encode(d.record()), "fresh_request": hex::encode(joiner.request()),
        "fresh_admission": hex::encode(&admission), "fresh_sha256": hex::encode(hash(&admission))
    });
    let expected: serde_json::Value =
        serde_json::from_str(include_str!("../../fixtures/rotation.json")).unwrap();
    assert_eq!(vector, expected);
}

#[test]
fn every_survivor_opens_own_package_and_pending_admission_preserves_freeze() {
    let f = fixture();
    let (_, _, peer, _, m) = first(&f);
    let keys = m.verify_initial_key(&f.key).unwrap();
    let freeze = peer.authority().prepare_revoke(&m, &[]).unwrap();
    let pending = m.append(&[], &[], &freeze).unwrap();
    let (d, third, admission, pending) = add_fixed(peer.authority(), &pending, &keys, 65);
    assert!(pending.pending);
    let verified = third
        .verify_enrollment(
            &m.append(&[], &[], &freeze).unwrap(),
            d.record(),
            &admission,
        )
        .unwrap();
    let raw = Device::seed(&f.seed)
        .prepare_rotation(&pending, &keys, 0)
        .unwrap();
    let other = Device::seed(&f.seed)
        .prepare_rotation(&pending, &keys, 0)
        .unwrap();
    let branch = pending.append(&[], &[], &raw).unwrap();
    let other_branch = pending.append(&[], &[], &other).unwrap();
    assert_ne!(
        branch.current_generation().id,
        other_branch.current_generation().id
    );
    assert_ne!(
        branch.current_generation().commitment,
        other_branch.current_generation().commitment
    );
    assert!(!branch.extends(&other_branch));
    for device in [Device::seed(&f.seed), peer.authority(), third.authority()] {
        let k = device
            .receive_rotation(&pending, &raw, verified.keys())
            .unwrap();
        k.validate(&pending.append(&[], &[], &raw).unwrap())
            .unwrap();
    }
    assert!(
        Device::seed(&f.seed)
            .prepare_rotation(&m, &keys, 0)
            .is_err()
    );
    let self_leave = peer
        .authority()
        .prepare_revoke(&m, &[peer.device()])
        .unwrap();
    let left = m.append(&[], &[], &self_leave).unwrap();
    assert_eq!(left.device_count(), 1);
    assert!(peer.authority().prepare_rotation(&left, &keys, 0).is_err());
    Device::seed(&f.seed)
        .prepare_rotation(&left, &keys, 0)
        .unwrap();
}

#[test]
fn rotation_missing_extra_reordered_or_invalid_own_packages_never_yield_keys() {
    let f = fixture();
    let (_, _, peer, _, m) = first(&f);
    let keys = m.verify_initial_key(&f.key).unwrap();
    let freeze = Device::seed(&f.seed).prepare_revoke(&m, &[]).unwrap();
    let pending = m.append(&[], &[], &freeze).unwrap();
    let (raw, _, _) = rotate_fixed(Device::seed(&f.seed), &pending, &keys, 100, 0);
    for kind in 0..7 {
        let bad = resign(&f.seed.signing, &raw, |_, _, p| match kind {
            0 => {
                p[7] = 1;
                p.truncate(8 + 306);
            }
            1 => {
                p[7] = 3;
                p.extend_from_within(8..314);
            }
            2 => {
                let first = p[8..314].to_vec();
                let second = p[314..620].to_vec();
                p[8..314].copy_from_slice(&second);
                p[314..620].copy_from_slice(&first);
            }
            3 => {
                let first = p[8..314].to_vec();
                p[314..620].copy_from_slice(&first);
            }
            4 => p[9] = 1,
            5 => p[14] ^= 1,
            _ => p[50] ^= 1,
        });
        assert!(pending.append(&[], &[], &bad).is_err(), "shape {kind}");
        assert!(
            peer.authority()
                .receive_rotation(&pending, &bad, &keys)
                .is_err()
        );
    }
    // A valid signature cannot make malformed encrypted plaintext usable.
    for device in [Device::seed(&f.seed), peer.authority()] {
        let (core, state, packages, _) = encoding::components(&raw).unwrap();
        let rows = encoding::read_packages(packages, 0, 2, 176).unwrap();
        let mut changed = encoding::packages(2);
        for p in rows {
            let mut cipher = p.cipher.to_vec();
            if p.device == device.device {
                cipher[0] ^= 1;
            }
            encoding::package(&mut changed, 0, &p.device, &p.public, p.enc, &cipher);
        }
        let bad = admission::signed(&f.seed.signing, core, state, &changed);
        assert!(pending.append(&[], &[], &bad).is_ok());
        assert!(device.receive_rotation(&pending, &bad, &keys).is_err());
    }
}

#[test]
fn rotation_semantics_cutoff_context_and_predecessor_signature_are_strict() {
    let f = fixture();
    let (_, _, peer, _, m) = first(&f);
    let keys = m.verify_initial_key(&f.key).unwrap();
    let freeze = Device::seed(&f.seed).prepare_revoke(&m, &[]).unwrap();
    let pending = m.append(&[], &[], &freeze).unwrap();
    let (raw, next, keys2) = rotate_fixed(Device::seed(&f.seed), &pending, &keys, 100, 10);
    for i in 0..raw.len() {
        let mut bad = raw.clone();
        bad[i] ^= 1;
        assert!(pending.append(&[], &[], &bad).is_err(), "mutation {i}");
        assert!(
            pending.append(&[], &[], &raw[..i]).is_err(),
            "truncation {i}"
        );
    }
    let (core, state, p, _) = encoding::components(&raw).unwrap();
    for i in 0..core.len() {
        let mut changed = core.to_vec();
        changed[i] ^= 1;
        let bad = admission::signed(&f.seed.signing, &changed, state, p);
        assert!(pending.append(&[], &[], &bad).is_err(), "core {i}");
    }
    for i in 0..state.len() {
        let bad = resign(&f.seed.signing, &raw, |_, s, _| s[i] ^= 1);
        assert!(pending.append(&[], &[], &bad).is_err(), "state {i}");
    }
    assert!(
        pending
            .append(
                &[],
                &[],
                &admission::signed(&peer.0.signing, core, state, p)
            )
            .is_err()
    );
    assert!(pending.append(&[0], &[], &raw).is_err());
    assert!(next.append(&[], &[], &raw).is_err());
    let freeze = Device::seed(&f.seed).prepare_revoke(&next, &[]).unwrap();
    let pending2 = next.append(&[], &[], &freeze).unwrap();
    assert!(
        Device::seed(&f.seed)
            .prepare_rotation(&pending2, &keys2, 9)
            .is_err()
    );
    assert!(
        Device::seed(&f.seed)
            .rotation_with(
                &pending2,
                &keys2,
                [100; 32],
                &f.key,
                10,
                &mut ChaCha20Rng::from_seed([0; 32])
            )
            .is_err()
    );
    assert!(
        Device::seed(&f.seed)
            .receive_rotation(&pending2, &raw, &keys)
            .is_err()
    );
    // A nonzero bootstrap prefix is an independent lower bound on cutoff.
    let mut prefix = pending.clone();
    prefix.publication.binding.prefix_count = 20;
    assert!(
        Device::seed(&f.seed)
            .prepare_rotation(&prefix, &keys, 19)
            .is_err()
    );
}

#[test]
fn complete_ordered_join_history_is_required_before_verified_keys() {
    let f = fixture();
    let (_, _, peer, _, m) = first(&f);
    let keys = m.verify_initial_key(&f.key).unwrap();
    let freeze = Device::seed(&f.seed).prepare_revoke(&m, &[]).unwrap();
    let pending = m.append(&[], &[], &freeze).unwrap();
    let (_, m, keys) = rotate_fixed(Device::seed(&f.seed), &pending, &keys, 100, 0);
    let (inv, d) = peer
        .authority()
        .invitation_with_psk(&m, 100, Secret::new([70; 32]))
        .unwrap();
    let joiner = joiner(&inv, 71);
    let plain = admission::grant_plaintext(
        &m,
        &d,
        joiner.request(),
        &joiner.0.recipient().unwrap(),
        &keys,
    );
    for kind in 0..6 {
        let mut bad = plain.clone();
        let start = GRANT_PREFIX_BYTES + 2;
        match kind {
            0 => bad[GRANT_PREFIX_BYTES + 1] = 1,
            1 => bad[GRANT_PREFIX_BYTES + 1] = 3,
            2 => {
                let a = bad[start..start + 72].to_vec();
                let b = bad[start + 72..start + 144].to_vec();
                bad[start..start + 72].copy_from_slice(&b);
                bad[start + 72..].copy_from_slice(&a);
            }
            3 => bad[start + 32] ^= 1,
            4 => bad[start + 64] ^= 1,
            _ => {
                let a = bad[start..start + 72].to_vec();
                bad[start + 72..].copy_from_slice(&a);
            }
        }
        let raw = reseal(&m, &d, &joiner, &peer.0.signing, &bad, 80);
        assert!(m.append(d.record(), joiner.request(), &raw).is_ok());
        assert!(
            joiner.verify_enrollment(&m, d.record(), &raw).is_err(),
            "coverage {kind}"
        );
    }
    assert!(
        peer.authority()
            .prepare_admission(
                &m,
                &d,
                &inv,
                joiner.request(),
                &f.membership.verify_initial_key(&f.key).unwrap()
            )
            .is_err()
    );
}

#[test]
fn encoded_maxima_and_freeze_reserves_are_exercised_with_signed_history() {
    let f = fixture();
    let mut m = f.membership.clone();
    let mut keys = m.verify_initial_key(&f.key).unwrap();
    for i in 0..30 {
        let (_, _, _, next) = add_fixed(Device::seed(&f.seed), &m, &keys, i * 6);
        m = next;
    }
    assert_eq!(m.device_count(), 31);
    // Actual rotations reach all 32 retained generations, without fabricated rows.
    for i in 0..30 {
        let freeze = Device::seed(&f.seed).prepare_revoke(&m, &[]).unwrap();
        m = m.append(&[], &[], &freeze).unwrap();
        let (raw, next, next_keys) = rotate_fixed(Device::seed(&f.seed), &m, &keys, 190 + i, 0);
        assert!(raw.len() <= MAX_RECORD_BYTES);
        m = next;
        keys = next_keys;
    }
    let (_, _, _, full) = add_fixed(Device::seed(&f.seed), &m, &keys, 181);
    let freeze = Device::seed(&f.seed).prepare_revoke(&full, &[]).unwrap();
    let full = full.append(&[], &[], &freeze).unwrap();
    let (maximum, _, _) = rotate_fixed(Device::seed(&f.seed), &full, &keys, 220, 0);
    assert_eq!(maximum.len(), 17884);
    let freeze = Device::seed(&f.seed).prepare_revoke(&m, &[]).unwrap();
    let pending = m.append(&[], &[], &freeze).unwrap();
    let (_, m, keys) = rotate_fixed(Device::seed(&f.seed), &pending, &keys, 220, 0);
    let (d, peer, raw, next) = add_fixed(Device::seed(&f.seed), &m, &keys, 181);
    assert_eq!(encoding::state(&next).len(), 7734);
    assert_eq!(raw.len(), 11113);
    let plain =
        admission::grant_plaintext(&m, &d, peer.request(), &peer.0.recipient().unwrap(), &keys);
    assert_eq!(plain.len(), 2699);
    assert_eq!(keys.protected_storage_bytes().len(), 2344);
    VerifiedKeys::from_protected_storage(&m, &keys.protected_storage_bytes()).unwrap();
    peer.verify_enrollment(&m, d.record(), &raw).unwrap();
    assert!(Device::seed(&f.seed).prepare_revoke(&next, &[]).is_err());

    let keys = f.membership.verify_initial_key(&f.key).unwrap();
    let freeze = Device::seed(&f.seed)
        .prepare_revoke(&f.membership, &[])
        .unwrap();
    let mut near = f.membership.clone();
    near.evidence_bytes = MAX_CHAIN_BYTES - MAX_RECORD_BYTES - freeze.len();
    let pending = near.append(&[], &[], &freeze).unwrap();
    Device::seed(&f.seed)
        .prepare_rotation(&pending, &keys, 0)
        .unwrap();
    near.evidence_bytes += 1;
    assert!(near.append(&[], &[], &freeze).is_err());
    let mut near = f.membership.clone();
    near.heads = vec![near.head(); MAX_TRANSITIONS];
    assert!(Device::seed(&f.seed).prepare_revoke(&near, &[]).is_err());
}

#[test]
fn protected_rotation_material_replays_exact_candidate_and_rejects_other_generation() {
    let f = fixture();
    let (_, _, peer, _, m) = first(&f);
    let keys = m.verify_initial_key(&f.key).unwrap();
    let revoke = peer.authority().prepare_revoke(&m, &[]).unwrap();
    let pending = m.append(&[], &[], &revoke).unwrap();
    let material = RotationMaterial::generate().unwrap();
    let bytes = material.protected_storage_bytes();
    assert_eq!(bytes.len(), 102);
    let reopened = RotationMaterial::from_protected_storage(&bytes).unwrap();
    let raw = peer
        .authority()
        .prepare_rotation_with(&pending, &keys, 100, &material)
        .unwrap();
    assert_eq!(
        raw,
        peer.authority()
            .prepare_rotation_with(&pending, &keys, 100, &reopened)
            .unwrap()
    );
    let after = pending.append(&[], &[], &raw).unwrap();
    reopened.validate_generation(&after).unwrap();
    let other = RotationMaterial::generate().unwrap();
    assert!(other.validate_generation(&after).is_err());
    assert_ne!(
        raw,
        peer.authority()
            .prepare_rotation_with(&pending, &keys, 100, &other)
            .unwrap()
    );
    for length in [0, 6, 101] {
        assert!(RotationMaterial::from_protected_storage(&bytes[..length]).is_err());
    }
    let mut malformed = bytes.to_vec();
    malformed.push(0);
    assert!(RotationMaterial::from_protected_storage(&malformed).is_err());
    malformed.truncate(102);
    malformed[5] = 2;
    assert!(RotationMaterial::from_protected_storage(&malformed).is_err());
}
