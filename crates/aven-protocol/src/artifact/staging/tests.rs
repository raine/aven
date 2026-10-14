use super::*;
use crate::artifact::{
    catalog::{Chunk, Declaration, Image},
    state_catalog,
};
use crate::{
    codec::{bytes, hash},
    record,
};

fn context() -> LocalSharedStatePackageContext {
    LocalSharedStatePackageContext {
        vault_id: [1; 32],
        generation_id: [2; 32],
    }
}

// Artificial opaque bodies exercise keyless header and aggregate acceptance,
// not decryptability or server admission.
fn record(id: [u8; 32], family: u8, class: u8) -> (Artifact, Vec<Vec<u8>>) {
    let header =
        record::chunk_header(context(), [3; 32], id, family, class, 0, 1, 3, [5; 24]).unwrap();
    let mut record = Vec::new();
    bytes(&mut record, &header);
    bytes(&mut record, &[9; 19]);
    let artifact = Artifact {
        total: 3,
        aggregate: hash(&record),
        chunks: vec![Chunk {
            length: record.len() as u64,
            hash: hash(&record),
            nonce: [5; 24],
        }],
    };
    (artifact, vec![record])
}

fn declaration(
    state: &Artifact,
    manifest: &Artifact,
    image: &Artifact,
) -> (DeclarationView, [Vec<u8>; 3]) {
    let catalogs = [
        state_catalog(state).unwrap(),
        catalog::prefix_encode(&[]).unwrap(),
        Images {
            objects: vec![Image {
                id: [6; 32],
                selection: 2,
                artifact: image.clone(),
            }],
            parents: vec![],
            references: vec![],
        }
        .encode()
        .unwrap(),
    ];
    let descriptor = Descriptor {
        vault: [1; 32],
        stream: [3; 32],
        generation: [2; 32],
        bootstrap: [4; 32],
        membership: [7; 32],
        prefix: 0,
        catalogs: std::array::from_fn(|i| Declaration::new(&catalogs[i], i as u8 + 1).unwrap()),
        manifest: manifest.clone(),
    };
    (
        DeclarationView::decode(&descriptor.encode().unwrap()).unwrap(),
        catalogs,
    )
}

#[test]
fn staging_dispatch_preserves_fixed_record_bytes_and_physical_recipe_inputs() {
    let (state, state_records) = record([4; 32], 2, 1);
    let (manifest, manifest_records) = record([4; 32], 2, 2);
    let (image, image_records) = record([6; 32], 1, 0);
    let (d, catalogs) = declaration(&state, &manifest, &image);
    let state_catalog = d.catalog(0, &catalogs[0]).unwrap();
    let image_catalog = d.catalog(2, &catalogs[2]).unwrap();
    assert!(
        d.artifacts(Some(&d.catalog(1, &catalogs[1]).unwrap()))
            .is_empty()
    );
    for (view, records, component, identity, expected_hash) in [
        (
            d.artifacts(Some(&state_catalog)).remove(0),
            state_records,
            Component::State,
            ([4; 32], 2, 1),
            "f4556088451d8b2f01164411c6194074b9c116646907714aa1dbb393caa7e97d",
        ),
        (
            d.artifacts(None).remove(0),
            manifest_records,
            Component::Manifest,
            ([4; 32], 2, 2),
            "6a993808a375305c94894da1ba4b5f7e9f34f3d5170638004110c37fa9a1f8a5",
        ),
        (
            d.artifacts(Some(&image_catalog)).remove(0),
            image_records,
            Component::Image([6; 32]),
            ([6; 32], 1, 0),
            "fc0ecc580eed53213e956f32175433bde28abb0ad8d3f9f94c55341068156bb9",
        ),
    ] {
        assert_eq!(view.component, component);
        assert_eq!(view.identity().unwrap(), identity);
        assert_eq!(view.context(), context());
        assert_eq!(view.stream(), [3; 32]);
        assert_eq!(view.lengths(), vec![225]);
        assert_eq!(hex::encode(hash(&records[0])), expected_hash);
        view.verify_chunk(0, &records[0]).unwrap();
        view.verify(&records).unwrap();
        let (id, family, class) = view.identity().unwrap();
        view.artifact()
            .verify(&records, view.context(), view.stream(), id, family, class)
            .unwrap();
    }
}

#[test]
fn descriptor_recommitted_substitutions_cannot_change_component_header_semantics() {
    let (image, _) = record([6; 32], 1, 0);
    for (component, expected_class) in [(Component::State, 1), (Component::Manifest, 2)] {
        for (id, family, class) in [
            ([4; 32], 2, 3 - expected_class),
            ([4; 32], 1, 0),
            ([8; 32], 2, expected_class),
        ] {
            let (artifact, records) = record(id, family, class);
            let (d, catalogs) = declaration(&artifact, &artifact, &image);
            let state_catalog = d.catalog(0, &catalogs[0]).unwrap();
            let view = d
                .artifacts(if component == Component::State {
                    Some(&state_catalog)
                } else {
                    None
                })
                .remove(0);
            // Hashes and lengths match the substituted bytes. The shared
            // component identity must still reject their header.
            assert_eq!(view.verify_chunk(0, &records[0]), Err(Error::Invalid));
            assert_eq!(view.verify(&records), Err(Error::Invalid));
        }
    }
}

#[test]
fn retained_completion_requires_aggregate_even_when_every_slot_is_valid() {
    let (image, _) = record([6; 32], 1, 0);
    for (component, class) in [(Component::State, 1), (Component::Manifest, 2)] {
        let (mut artifact, records) = record([4; 32], 2, class);
        artifact.aggregate[0] ^= 1;
        let (d, catalogs) = declaration(&artifact, &artifact, &image);
        let state_catalog = d.catalog(0, &catalogs[0]).unwrap();
        let view = d
            .artifacts(if component == Component::State {
                Some(&state_catalog)
            } else {
                None
            })
            .remove(0);
        view.verify_chunk(0, &records[0]).unwrap();
        assert_eq!(view.verify(&records), Err(Error::Invalid));
        assert_eq!(view.verify(&[]), Err(Error::Invalid));
    }
}

#[test]
fn catalog_selectors_are_not_encrypted_artifact_identities() {
    let (artifact, records) = record([4; 32], 2, 2);
    let (d, _) = declaration(&artifact, &artifact, &artifact);
    let mut view = d.artifacts(None).remove(0);
    for component in [
        Component::DataCatalog,
        Component::PrefixCatalog,
        Component::ImageCatalog,
    ] {
        view.component = component;
        assert_eq!(view.identity(), Err(Error::Invalid));
        assert_eq!(view.verify_chunk(0, &records[0]), Err(Error::Invalid));
        assert_eq!(view.verify(&records), Err(Error::Invalid));
    }
}
