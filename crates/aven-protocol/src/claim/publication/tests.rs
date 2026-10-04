use super::*;

pub(crate) use super::fixture::{descriptor, seed, signed};

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
        "85933477fcac2c2d271758791324a91e31d5563553714e0a810da7934934d43a"
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
