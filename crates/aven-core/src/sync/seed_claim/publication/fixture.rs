//! Fixed signed publication built from the genesis fixture.
use super::*;

pub(in crate::sync::seed_claim) fn seed() -> SeedAuthority {
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
pub(in crate::sync::seed_claim) fn descriptor(g: &Genesis) -> Vec<u8> {
    let mut d = b"AVBP\0\x02\x01".to_vec();
    for id in [
        g.context().vault_id,
        [21; 32],
        g.context().generation_id,
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
        d.extend([class; 32]);
        d.extend([class + 10; 32]);
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

pub(in crate::sync::seed_claim) fn signed(
    seed: &SeedAuthority,
    core: &[u8],
    state: &[u8],
    attachments: &[u8],
) -> Vec<u8> {
    let mut core = core.to_vec();
    core[269..301].copy_from_slice(&hash(&cce("aven-e2ee/v1/membership/state", &[state])));
    let signature = SigningKey::from_bytes(seed.protected_signing_seed().expose())
        .sign(&cce("aven-e2ee/v1/membership/sign", &[&core, attachments]));
    let mut record = vec![1];
    for part in [&core[..], state, attachments, &signature.to_bytes()] {
        bytes(&mut record, part);
    }
    record
}
