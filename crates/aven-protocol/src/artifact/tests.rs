use super::*;
use crate::{codec::hash, record};
use catalog::Chunk;

fn fixture() -> (Artifact, Vec<Vec<u8>>, LocalSharedStatePackageContext) {
    let context = LocalSharedStatePackageContext {
        vault_id: [1; 32],
        generation_id: [2; 32],
    };
    let header = record::chunk_header(context, [3; 32], [4; 32], 1, 0, 0, 1, 3, [5; 24]).unwrap();
    let mut bytes = Vec::new();
    crate::codec::bytes(&mut bytes, &header);
    crate::codec::bytes(&mut bytes, &[9; 19]);
    let artifact = Artifact {
        total: 3,
        aggregate: hash(&bytes),
        chunks: vec![Chunk {
            length: bytes.len() as u64,
            hash: hash(&bytes),
            nonce: [5; 24],
        }],
    };
    (artifact, vec![bytes], context)
}

#[test]
fn exact_record_and_descriptor_bytes() {
    let (artifact, records, context) = fixture();
    assert_eq!(records[0].len(), 225);
    assert_eq!(
        hex::encode(hash(&records[0])),
        "1d586b706f9ac88d003fe9be8035942aaf3b111c2902afb25b9a978f7ffeb6a6"
    );
    artifact.shape(codec::IMAGE_LIMIT, true).unwrap();
    artifact
        .verify(&records, context, [3; 32], [4; 32], 1, 0)
        .unwrap();
    let mut encoded = Vec::new();
    artifact.write(&mut encoded).unwrap();
    let mut reader = codec::Reader(&encoded);
    assert_eq!(
        Artifact::read(&mut reader, codec::IMAGE_LIMIT, true).unwrap(),
        artifact
    );
    reader.end().unwrap();
    for n in 0..encoded.len() {
        assert!(
            Artifact::read(&mut codec::Reader(&encoded[..n]), codec::IMAGE_LIMIT, true).is_err()
        );
    }
}

#[test]
fn valid_individual_records_do_not_prove_aggregate_completeness() {
    let (mut artifact, records, context) = fixture();
    artifact.aggregate[0] ^= 1;
    artifact
        .verify_chunk(&records[0], 0, context, [3; 32], [4; 32], 1, 0)
        .unwrap();
    assert_eq!(
        artifact.verify(&records, context, [3; 32], [4; 32], 1, 0),
        Err(Error::Invalid)
    );
    assert!(
        artifact
            .verify(&[], context, [3; 32], [4; 32], 1, 0)
            .is_err()
    );
}

#[test]
fn record_verification_binds_framing_context_index_and_nonce() {
    let (artifact, records, context) = fixture();
    let verify = |a: &Artifact, bytes: &[u8], index, context, stream, id, family, class| {
        a.verify_chunk(bytes, index, context, stream, id, family, class)
    };
    for n in 0..records[0].len() {
        assert!(
            verify(
                &artifact,
                &records[0][..n],
                0,
                context,
                [3; 32],
                [4; 32],
                1,
                0
            )
            .is_err()
        );
    }
    assert!(verify(&artifact, &records[0], 1, context, [3; 32], [4; 32], 1, 0).is_err());
    for (context, stream, id, family, class) in [
        (
            LocalSharedStatePackageContext {
                vault_id: [6; 32],
                ..context
            },
            [3; 32],
            [4; 32],
            1,
            0,
        ),
        (
            LocalSharedStatePackageContext {
                generation_id: [6; 32],
                ..context
            },
            [3; 32],
            [4; 32],
            1,
            0,
        ),
        (context, [6; 32], [4; 32], 1, 0),
        (context, [3; 32], [6; 32], 1, 0),
        (context, [3; 32], [4; 32], 2, 0),
        (context, [3; 32], [4; 32], 1, 1),
    ] {
        assert!(
            verify(
                &artifact,
                &records[0],
                0,
                context,
                stream,
                id,
                family,
                class
            )
            .is_err()
        );
    }
    let mut changed = artifact.clone();
    changed.chunks[0].nonce[0] ^= 1;
    assert!(verify(&changed, &records[0], 0, context, [3; 32], [4; 32], 1, 0).is_err());
    // Recommit malformed framing so the hash check cannot mask the parser check.
    let mut bytes = records[0].clone();
    bytes[3] ^= 1;
    changed = artifact;
    changed.chunks[0].hash = hash(&bytes);
    assert!(verify(&changed, &bytes, 0, context, [3; 32], [4; 32], 1, 0).is_err());
}

#[test]
fn valid_catalog_slices_do_not_prove_catalog_aggregate() {
    let bytes = codec::stream(2, &[vec![7; codec::CHUNK as usize]]).unwrap();
    let mut declaration = Declaration::new(&bytes, 2).unwrap();
    declaration.hash[0] ^= 1;
    for (index, slice) in bytes.chunks(codec::CHUNK as usize).enumerate() {
        declaration.verify_slice(index, slice).unwrap();
    }
    assert_eq!(declaration.verify(&bytes, 2), Err(Error::Invalid));
}
