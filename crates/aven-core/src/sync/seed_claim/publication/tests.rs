use super::*;

fn seed() -> SeedAuthority {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../fixtures/genesis.json")).unwrap();
    let field = |name: &str| hex::decode(fixture[name].as_str().unwrap()).unwrap();
    let mut stored = Vec::new();
    for name in ["signing_seed", "hpke_private", "token", "record"] {
        stored.extend(field(name));
    }
    SeedAuthority::from_protected_storage(
        &stored,
        LocalSharedStatePackageContext {
            vault_id: field("vault").try_into().unwrap(),
            generation_id: field("generation").try_into().unwrap(),
        },
        &LocalSharedStatePackageKey::new(field("generation_secret").try_into().unwrap()),
    )
    .unwrap()
}

// Public framing fixture only. It makes no ciphertext completeness claim.
fn descriptor(g: &Genesis) -> Vec<u8> {
    let mut d = b"AVBP\0\x01\x01".to_vec();
    for id in [
        g.context.vault_id,
        [21; 32],
        g.context.generation_id,
        [22; 32],
        g.commitment(),
    ] {
        d.extend(id);
    }
    d.extend(0_u64.to_be_bytes());
    for class in 1..=3 {
        d.push(class);
        d.extend(0_u64.to_be_bytes());
        d.extend(15_u64.to_be_bytes());
        d.extend(1_u64.to_be_bytes());
        d.extend([class; 32]);
    }
    d.extend(0_u64.to_be_bytes());
    d.extend([23; 32]);
    d.extend(1_u64.to_be_bytes());
    d.extend(0_u64.to_be_bytes());
    d.extend(222_u64.to_be_bytes());
    d.extend([24; 32]);
    d.extend([25; 24]);
    d
}

fn signed(seed: &SeedAuthority, core: &[u8], state: &[u8], attachments: &[u8]) -> Vec<u8> {
    let mut core = core.to_vec();
    core[269..301].copy_from_slice(&hash(&cce("aven-e2ee/v1/membership/state", &[state])));
    let signature = SigningKey::from_bytes(seed.signing.expose())
        .sign(&cce("aven-e2ee/v1/membership/sign", &[&core, attachments]));
    let mut record = vec![1];
    for part in [&core[..], state, attachments, &signature.to_bytes()] {
        bytes(&mut record, part);
    }
    record
}

#[test]
fn fixed_successor_bytes_and_strict_semantics_preserve_genesis() {
    let seed = seed();
    let g = seed.genesis();
    let d = descriptor(g);
    let b = PublicationBinding::from_descriptor(g, &d).unwrap();
    let (core, state, attachments) = components(g, &b);
    assert_eq!((core.len(), state.len(), attachments.len()), (301, 416, 7));
    let record = signed(&seed, &core, &state, &attachments);
    let publication = Publication::from_record(g, &d, &record).unwrap();
    assert_eq!(record.len(), 805);
    assert_eq!(
        hex::encode(publication.commitment()),
        "0f2e101fdd9c47478e55224d6c01eb5a358f48ebae29613c3a9875040dae17d3"
    );
    assert_eq!(publication.binding().prefix_count, 0);
    assert!(Genesis::from_record(&record).is_err());
    assert_eq!(Genesis::from_record(g.record()).unwrap(), *g);
    for index in 0..record.len() {
        assert!(Publication::from_record(g, &d, &record[..index]).is_err());
        let mut bad = record.clone();
        bad[index] ^= 1;
        assert!(
            Publication::from_record(g, &d, &bad).is_err(),
            "byte {index}"
        );
    }
    let mut extra = record.clone();
    extra.push(0);
    assert!(Publication::from_record(g, &d, &extra).is_err());

    // Real seed signatures over changed authority, flags, generation, bootstrap
    // binding or sequence still cannot authorize those unsupported semantics.
    for index in 0..state.len() {
        let mut bad = state.clone();
        bad[index] ^= 1;
        assert!(
            Publication::from_record(g, &d, &signed(&seed, &core, &bad, &attachments)).is_err(),
            "state {index}"
        );
    }
    for index in 0..269 {
        let mut bad = core.clone();
        bad[index] ^= 1;
        assert!(
            Publication::from_record(g, &d, &signed(&seed, &bad, &state, &attachments)).is_err(),
            "core {index}"
        );
    }
    for index in 0..attachments.len() {
        let mut bad = attachments.clone();
        bad[index] ^= 1;
        assert!(Publication::from_record(g, &d, &signed(&seed, &core, &state, &bad)).is_err());
    }
    let other = SeedAuthority::generate(
        g.context(),
        &LocalSharedStatePackageKey::new([99; 32]),
        g.setup_id(),
    )
    .unwrap();
    assert!(Publication::from_record(g, &d, &signed(&other, &core, &state, &attachments)).is_err());
    let mut wrong_descriptor = d.clone();
    wrong_descriptor[39] ^= 1;
    assert!(Publication::from_record(g, &wrong_descriptor, &record).is_err());
}
