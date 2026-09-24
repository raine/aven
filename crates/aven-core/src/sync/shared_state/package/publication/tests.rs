use super::super::EncryptedLocalSharedStatePackage;
use super::super::test_support::*;
use super::*;
use crate::operations::TaskUpdate;

async fn specimen() -> (
    tempfile::TempDir,
    crate::db::Database,
    NeverDispatchedLocalSharedCapture,
    EncryptedLocalSharedStatePackage,
    Package,
) {
    let (dir, database, task) = source_with_history().await;
    add_selected_images(dir.path(), &database, &task).await;
    let mut conn = database.acquire_writer().await.unwrap();
    sqlx::query("UPDATE changes SET server_seq = 7001 WHERE change_id = (SELECT change_id FROM changes ORDER BY local_seq LIMIT 1)")
        .execute(&mut *conn).await.unwrap();
    drop(conn);
    let capture = database
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let local = database
        .package_local_shared_state_never_dispatched(
            dir.path(),
            package_context(),
            &package_key(),
            [0x64; 32],
        )
        .await
        .unwrap();
    let package = local.upload_package();
    (dir, database, capture, local, package)
}

#[test]
fn exact_prefix_fixture_and_round_trip() {
    let bytes = catalog::prefix_encode(&[(1, "first".into()), (2, "second".into())]).unwrap();
    assert_eq!(
        hex::encode(&bytes),
        concat!(
            "415642430001020000000000000002",
            "0000000000000015000000000000000100000000000000056669727374",
            "0000000000000016000000000000000200000000000000067365636f6e64"
        )
    );
    assert_eq!(
        catalog::prefix_decode(&bytes, 2).unwrap(),
        vec![(1, "first".into()), (2, "second".into())]
    );
    for end in 0..bytes.len() {
        assert!(catalog::prefix_decode(&bytes[..end], 2).is_err());
    }
    let mut trailing = bytes.clone();
    trailing.push(0);
    assert!(catalog::prefix_decode(&trailing, 2).is_err());
    for rows in [
        vec![(2, "second".into()), (1, "first".into())],
        vec![(1, "first".into()), (1, "second".into())],
        vec![(1, "first".into()), (2, "first".into())],
        vec![(1, "first".into())],
    ] {
        assert_eq!(
            catalog::prefix_decode(&catalog::prefix_encode(&rows).unwrap(), 2),
            Err(Error::Invalid)
        );
    }
}

#[test]
fn descriptor_fixture_is_exact_and_bounded() {
    let d = Descriptor {
        vault: [1; 32],
        stream: [2; 32],
        generation: [3; 32],
        bootstrap: [4; 32],
        membership: [5; 32],
        prefix: 0,
        catalogs: std::array::from_fn(|i| Declaration {
            count: 0,
            length: 15,
            hash: [i as u8 + 6; 32],
            slices: vec![[i as u8 + 12; 32]],
        }),
        manifest: Artifact {
            total: 1,
            aggregate: [9; 32],
            chunks: vec![catalog::Chunk {
                length: 223,
                hash: [10; 32],
                nonce: [11; 24],
            }],
        },
    };
    let bytes = d.encode().unwrap();
    let expected = include_str!("descriptor.hex").trim();
    assert_eq!(hex::encode(&bytes), expected);
    assert!(Descriptor::decode(&bytes).unwrap() == d);
    for end in 0..bytes.len() {
        assert!(Descriptor::decode(&bytes[..end]).is_err());
    }
    let mut huge = d.clone();
    huge.prefix = u64::MAX;
    assert!(Descriptor::decode(&huge.encode().unwrap()).is_err());
    huge.prefix = i64::MAX as u64;
    assert!(Descriptor::decode(&huge.encode().unwrap()).is_err());
    huge.prefix = RECORD_LIMIT + 1;
    assert!(matches!(
        Descriptor::decode(&huge.encode().unwrap()),
        Err(Error::ResourceLimit)
    ));
    // A slice list that disagrees with the declared length cannot be framed.
    let mut extra = d.clone();
    extra.catalogs[0].slices.push([0; 32]);
    assert_eq!(
        Descriptor::decode(&extra.encode().unwrap()).err(),
        Some(Error::Invalid)
    );
    assert_eq!(add(u64::MAX, 1), Err(Error::Invalid));
    assert_eq!(count(u64::MAX), 17_592_186_044_416);
    let mut r = Reader(&u64::MAX.to_be_bytes());
    assert_eq!(r.bytes(STATE_LIMIT), Err(Error::ResourceLimit));
}

#[test]
fn other_descriptor_versions_are_refused_as_unsupported() {
    let v1 = hex::decode(include_str!("descriptor-v1.hex").trim()).unwrap();
    assert_eq!(Descriptor::decode(&v1).err(), Some(Error::Unsupported));
    let current = hex::decode(include_str!("descriptor.hex").trim()).unwrap();
    for version in [0_u16, 1, 3, u16::MAX] {
        let mut other = current.clone();
        other[4..6].copy_from_slice(&version.to_be_bytes());
        assert_eq!(Descriptor::decode(&other).err(), Some(Error::Unsupported));
    }
}

#[test]
fn maximum_descriptor_is_exactly_the_named_bound() {
    let declaration = |class: u8| Declaration {
        count: RECORD_LIMIT,
        length: CATALOG_LIMIT,
        hash: [class; 32],
        slices: vec![[class; 32]; 16],
    };
    let d = Descriptor {
        vault: [1; 32],
        stream: [2; 32],
        generation: [3; 32],
        bootstrap: [4; 32],
        membership: [5; 32],
        prefix: RECORD_LIMIT,
        catalogs: [declaration(1), declaration(2), declaration(3)],
        manifest: Artifact {
            total: CHUNK,
            aggregate: [9; 32],
            chunks: vec![catalog::Chunk {
                length: CHUNK + 222,
                hash: [10; 32],
                nonce: [11; 24],
            }],
        },
    };
    let bytes = d.encode().unwrap();
    assert_eq!(bytes.len(), MAX_DESCRIPTOR_BYTES);
    assert_eq!(MAX_DESCRIPTOR_BYTES, 1978);
    assert!(Descriptor::decode(&bytes).unwrap() == d);
    let mut padded = bytes;
    padded.push(0);
    assert_eq!(
        Descriptor::decode(&padded).err(),
        Some(Error::ResourceLimit)
    );
}

#[test]
fn catalog_slices_verify_only_in_their_own_slot() {
    let rows = (1..=40_000)
        .map(|rank| (rank, format!("operation-{rank:08}")))
        .collect::<Vec<_>>();
    let bytes = catalog::prefix_encode(&rows).unwrap();
    let declaration = Declaration::new(&bytes, 2).unwrap();
    let slices = bytes.chunks(CHUNK as usize).collect::<Vec<_>>();
    assert_eq!(slices.len(), 2);
    for (index, slice) in slices.iter().enumerate() {
        declaration.verify_slice(index, slice).unwrap();
        assert!(declaration.verify_slice(1 - index, slice).is_err());
        assert!(declaration.verify_slice(index, &slice[1..]).is_err());
        let mut flipped = slice.to_vec();
        flipped[0] ^= 1;
        assert!(declaration.verify_slice(index, &flipped).is_err());
    }
    assert!(declaration.verify_slice(2, slices[1]).is_err());
    declaration.verify(&bytes, 2).unwrap();
    // Matching slices do not repair a wrong aggregate or record count.
    for lie in [
        Declaration {
            hash: [0; 32],
            ..declaration.clone()
        },
        Declaration {
            count: 1,
            ..declaration.clone()
        },
    ] {
        lie.verify_slice(0, slices[0]).unwrap();
        assert!(lie.verify(&bytes, 2).is_err());
    }
}

#[tokio::test]
async fn real_capture_round_trip_images_privacy_and_unchanged_local_retry() {
    let (dir, db, capture, local, package) = specimen().await;
    let complete = validate_keyless(&package).unwrap();
    assert_eq!(complete.image_count, 2);
    let images = Images::decode(&package.catalogs[2]).unwrap();
    assert_eq!(
        images
            .objects
            .iter()
            .map(|o| o.selection)
            .collect::<std::collections::BTreeSet<_>>(),
        [1, 2].into()
    );
    let (decoded, mappings) = decrypt_domain(&package, &package_key()).unwrap();
    assert_eq!(
        domain::encode(&decoded.snapshot.tables, &mappings).unwrap(),
        domain::encode(&capture.capture.snapshot.tables, &mappings).unwrap()
    );
    assert!(
        mappings
            .iter()
            .any(|m| m.classification == "unavailable" && m.object.is_none())
    );
    assert!(
        decoded
            .snapshot
            .tables
            .shared_history_provenance
            .iter()
            .any(|p| p.source_server_seq == Some(7001))
    );
    assert!(
        decoded
            .snapshot
            .tables
            .shared_history_provenance
            .iter()
            .any(|p| p.source_pending_rank.is_some())
    );
    for bytes in std::iter::once(&package.descriptor).chain(package.catalogs.iter()) {
        for secret in [
            "captured title",
            "history retained",
            "source_pending_rank",
            "source_server_seq",
            "capture.png",
        ] {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|part| part == secret.as_bytes())
            );
        }
        for mapping in &mappings {
            assert!(
                !bytes
                    .windows(mapping.sha256.len())
                    .any(|part| part == mapping.sha256.as_bytes())
            );
            let raw = hex::decode(&mapping.sha256).unwrap();
            assert!(!bytes.windows(raw.len()).any(|part| part == raw));
        }
    }
    let task = &capture.capture.snapshot.tables.tasks[0];
    let mut conn = db.acquire_writer().await.unwrap();
    let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    drop(conn);
    db.update_task(
        &workspace,
        &task.id,
        TaskUpdate {
            title: Some("after capture".into()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    validate_against_capture(&package, &capture, &package_key(), [0x64; 32]).unwrap();
    let retry = db
        .package_local_shared_state_never_dispatched(
            dir.path(),
            package_context(),
            &package_key(),
            [0x64; 32],
        )
        .await
        .unwrap();
    assert_eq!(retry, local);
    let resumed = db
        .resume_local_shared_state_never_dispatched()
        .await
        .unwrap()
        .unwrap();
    validate_against_capture(&package, &resumed, &package_key(), [0x64; 32]).unwrap();
    let rebuilt = retry.upload_package();
    assert_eq!(package.descriptor, rebuilt.descriptor);
    assert!(package.images == rebuilt.images);
    assert!(validate_against_capture(&package, &capture, &package_key(), [0; 32]).is_err());
    assert!(
        validate_against_capture(
            &package,
            &capture,
            &LocalSharedStatePackageKey::new([9; 32]),
            [0x64; 32]
        )
        .is_err()
    );
}

#[tokio::test]
async fn keyless_rejects_incomplete_mutated_reordered_and_recommitted_catalogs() {
    let (_, _, capture, _, package) = specimen().await;
    for index in 0..3 {
        let mut bad = package.clone();
        bad.catalogs[index].pop();
        assert!(validate_keyless(&bad).is_err());
        let mut bad = package.clone();
        bad.catalogs[index][0] ^= 1;
        assert!(validate_keyless(&bad).is_err());
    }
    let mut bad = package.clone();
    bad.catalogs.swap(0, 1);
    assert!(validate_keyless(&bad).is_err());
    let mut bad = package.clone();
    bad.state.clear();
    assert!(validate_keyless(&bad).is_err());
    let mut bad = package.clone();
    bad.manifest[0].push(0);
    assert!(validate_keyless(&bad).is_err());
    let mut bad = package.clone();
    bad.images.pop();
    assert!(validate_keyless(&bad).is_err());
    let mut bad = package.clone();
    bad.images.push(bad.images[0].clone());
    assert!(validate_keyless(&bad).is_err());
    let mut bad = package.clone();
    bad.images.reverse();
    assert!(validate_keyless(&bad).is_err());
    let mut bad = package.clone();
    bad.images[0].records[0][210] ^= 1;
    assert!(validate_keyless(&bad).is_err());
    let mut bad = package.clone();
    let mut images = Images::decode(&bad.catalogs[2]).unwrap();
    images.objects.push(images.objects[0].clone());
    bad.catalogs[2] = images.encode().unwrap();
    super::recommit_catalog(&mut bad, 2);
    assert!(validate_keyless(&bad).is_err());
    let mut bad = package.clone();
    let mut rows = read_stream(&bad.catalogs[1], 2)
        .unwrap()
        .iter()
        .map(|r| r.to_vec())
        .collect::<Vec<_>>();
    rows.reverse();
    bad.catalogs[1] = stream(2, &rows).unwrap();
    super::recommit_catalog(&mut bad, 1);
    assert!(validate_keyless(&bad).is_err());
    // Structurally valid lies cannot be detected by a keyless server, but the
    // encrypted manifest and the source comparison must reject them.
    let mut bad = package.clone();
    let mut rows = catalog::prefix_decode(
        &bad.catalogs[1],
        Descriptor::decode(&bad.descriptor).unwrap().prefix,
    )
    .unwrap();
    rows[0].1 = "substituted".into();
    bad.catalogs[1] = catalog::prefix_encode(&rows).unwrap();
    super::recommit_catalog(&mut bad, 1);
    validate_keyless(&bad).unwrap();
    assert!(validate_against_capture(&bad, &capture, &package_key(), [0x64; 32]).is_err());
}

#[tokio::test]
async fn domain_rejects_missing_unknown_duplicate_and_noncanonical_fields() {
    let (_, _, capture, _, _) = specimen().await;
    let (encoded, _) = domain::encode(&capture.capture.snapshot.tables, &[]).unwrap();
    let mut r = Reader(&encoded[6..]);
    assert_eq!(u16::from_be_bytes(r.array().unwrap()), 1);
    let n = r.u64().unwrap();
    assert!(n > 0);
    let first = r.bytes(STATE_LIMIT).unwrap();
    let offset = encoded.len() - r.0.len() - first.len() - 8;
    for payload in [
        format!("{} ", std::str::from_utf8(first).unwrap()).into_bytes(),
        first
            .iter()
            .copied()
            .take(first.len() - 1)
            .chain(b",\"unknown\":0}".iter().copied())
            .collect(),
        first
            .iter()
            .copied()
            .take(first.len() - 1)
            .chain(b",\"name\":\"duplicate\"}".iter().copied())
            .collect(),
        b"{}".to_vec(),
    ] {
        let mut bad = encoded[..offset].to_vec();
        bytes(&mut bad, &payload).unwrap();
        bad.extend_from_slice(r.0);
        assert!(domain::decode(&bad).is_err());
    }
    for end in [0, 5, encoded.len() - 1] {
        assert!(domain::decode(&encoded[..end]).is_err());
    }
    let mut trailing = encoded;
    trailing.push(0);
    assert!(domain::decode(&trailing).is_err());
}

#[test]
fn artifact_limits_and_exact_chunk_lengths() {
    let mut artifact = Artifact {
        total: 0,
        aggregate: [0; 32],
        chunks: vec![catalog::Chunk {
            length: 222,
            hash: [0; 32],
            nonce: [0; 24],
        }],
    };
    artifact.shape(STATE_LIMIT, false).unwrap();
    assert_eq!(artifact.shape(IMAGE_LIMIT, true), Err(Error::Invalid));
    artifact.total = u64::MAX;
    assert_eq!(
        artifact.shape(STATE_LIMIT, false),
        Err(Error::ResourceLimit)
    );
    artifact.total = CHUNK + 1;
    artifact.chunks[0].length = CHUNK + 222;
    assert!(artifact.shape(STATE_LIMIT, false).is_err());
    artifact.chunks.push(catalog::Chunk {
        length: 223,
        hash: [0; 32],
        nonce: [0; 24],
    });
    artifact.shape(STATE_LIMIT, false).unwrap();
    artifact.chunks[1].length = 224;
    assert!(artifact.shape(STATE_LIMIT, false).is_err());
}

#[tokio::test]
async fn unavailable_reference_has_no_object_or_byte_obligation() {
    let (dir, db, task) = source_with_history().await;
    let mut conn = db.acquire_writer().await.unwrap();
    let workspace = crate::workspaces::ensure_default_workspace(&mut conn)
        .await
        .unwrap();
    drop(conn);
    let attachment = db
        .add_task_attachment(
            &workspace,
            dir.path(),
            crate::attachments::lifecycle::LifecyclePolicy::default(),
            &task.parse().unwrap(),
            crate::operations::AttachmentAddInput {
                filename: Some("private.png".into()),
                alt_text: None,
                declared_media_type: Some("image/png".into()),
                bytes: png_bytes(3, 2),
                optimization_policy: crate::attachments::ImageOptimizationPolicy::Preserve,
                dedupe_existing: false,
            },
        )
        .await
        .unwrap()
        .outcome
        .attachment;
    db.delete_task_attachment(&workspace, &attachment.attachment_id)
        .await
        .unwrap();
    let mut conn = db.acquire_writer().await.unwrap();
    sqlx::query("UPDATE blob_inventory SET available = 0")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    std::fs::remove_file(
        crate::attachments::storage::object_path(dir.path(), &attachment.sha256).unwrap(),
    )
    .unwrap();
    let capture = db
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let local = db
        .package_local_shared_state_never_dispatched(
            dir.path(),
            package_context(),
            &package_key(),
            [0x64; 32],
        )
        .await
        .unwrap();
    let package = local.upload_package();
    assert_eq!(validate_keyless(&package).unwrap().image_count, 0);
    let catalog = Images::decode(&package.catalogs[2]).unwrap();
    assert!(catalog.objects.is_empty());
    assert_eq!(catalog.references.len(), 1);
    assert!(catalog.references[0].deleted);
    assert_eq!(catalog.references[0].object, None);
    validate_against_capture(&package, &capture, &package_key(), [0x64; 32]).unwrap();
    let mut bad = package;
    let mut catalog = catalog;
    catalog.references[0].object = Some([88; 32]);
    bad.catalogs[2] = catalog.encode().unwrap();
    super::recommit_catalog(&mut bad, 2);
    assert!(validate_keyless(&bad).is_err());
}

#[tokio::test]
async fn empty_capture_and_records_spanning_transport_chunks() {
    let dir = tempfile::tempdir().unwrap();
    let db = crate::db::Database::open(&dir.path().join("empty.sqlite"))
        .await
        .unwrap();
    db.capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let local = db
        .package_local_shared_state_never_dispatched(
            dir.path(),
            package_context(),
            &package_key(),
            [0x64; 32],
        )
        .await
        .unwrap();
    let package = local.upload_package();
    assert_eq!(validate_keyless(&package).unwrap().prefix_count, 0);

    // Materialized imported text can exceed an operation's payload limit and a
    // transport chunk. Domain records have only the independent resource cap.
    let (dir, db, task) = source_with_history().await;
    let mut conn = db.acquire_writer().await.unwrap();
    sqlx::query("UPDATE tasks SET description = ? WHERE id = ?")
        .bind("x".repeat(CHUNK as usize + 7))
        .bind(&task)
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    let capture = db
        .capture_local_shared_state_never_dispatched()
        .await
        .unwrap();
    let local = db
        .package_local_shared_state_never_dispatched(
            dir.path(),
            package_context(),
            &package_key(),
            [0x64; 32],
        )
        .await
        .unwrap();
    let package = local.upload_package();
    assert!(package.state.len() > 1);
    validate_against_capture(&package, &capture, &package_key(), [0x64; 32]).unwrap();
    let mut reversed = package.clone();
    reversed.state.reverse();
    assert!(validate_keyless(&reversed).is_err());
    let mut duplicate = package;
    duplicate.state[1] = duplicate.state[0].clone();
    assert!(validate_keyless(&duplicate).is_err());
}

#[tokio::test]
async fn parent_protection_is_derived_from_captured_history_not_final_boolean() {
    let (_, _, capture, _, package) = specimen().await;
    let (mut decoded, mappings) = decrypt_domain(&package, &package_key()).unwrap();
    let base = Images::decode(&package.catalogs[2]).unwrap();
    assert!(!base.parents[0].protected);
    let task = &capture.capture.snapshot.tables.tasks[0];
    let t = &mut decoded.snapshot.tables;
    let prior = t
        .changes
        .iter()
        .map(|r| r.server_seq.unwrap())
        .max()
        .unwrap();
    t.changes.push(crate::data_safety::export_types::ChangeRow {
        change_id: "stale-delete".into(),
        client_id: "client".into(),
        local_seq: 100,
        entity_type: "task".into(),
        entity_id: task.id.to_string(),
        field: Some("deleted".into()),
        op_type: "set_field".into(),
        payload: serde_json::json!({"workspace_id":task.workspace_id,"value":"1"}).to_string(),
        base_version: Some("wrong-base".into()),
        created_at: String::new(),
        server_seq: Some(prior + 1),
    });
    let project = |tables: &crate::data_safety::export_types::ExportTables| {
        projection::images(
            tables,
            &mappings,
            Images {
                objects: base.objects.clone(),
                parents: Vec::new(),
                references: Vec::new(),
            },
        )
        .unwrap()
    };
    assert!(project(t).parents[0].protected);
    let mut force = t.changes.last().unwrap().clone();
    force.change_id = "force".into();
    force.op_type = "resolve_field".into();
    force.server_seq = Some(prior + 2);
    t.changes.push(force);
    assert!(project(t).parents[0].protected);
    t.changes.clear();
    assert!(project(t).parents[0].protected);
}

#[test]
fn deterministic_domain_and_unavailable_catalog_fixtures() {
    let bytes = hex::decode(include_str!("empty-domain.hex").trim()).unwrap();
    let (tables, mappings, stats) = domain::decode(&bytes).unwrap();
    assert_eq!(domain::encode(&tables, &mappings).unwrap(), (bytes, stats));
    let bytes = hex::decode(include_str!("unavailable-catalog.hex").trim()).unwrap();
    let catalog = Images::decode(&bytes).unwrap();
    assert_eq!(catalog.encode().unwrap(), bytes);
    assert!(catalog.references[0].object.is_none());
    assert!(!catalog.references[0].deleted);
    assert!(catalog.parents[0].protected);
    for end in 0..bytes.len() {
        assert!(Images::decode(&bytes[..end]).is_err());
    }
    let mut duplicate = catalog.clone();
    duplicate.references.push(duplicate.references[0].clone());
    assert!(Images::decode(&duplicate.encode().unwrap()).is_err());
    let mut missing_parent = catalog;
    missing_parent.parents.clear();
    assert!(Images::decode(&missing_parent.encode().unwrap()).is_err());
}

#[tokio::test]
async fn keyless_checks_header_context_even_with_recommitted_records() {
    let (_, _, _, _, mut package) = specimen().await;
    package.state[0][52] ^= 1;
    let mut state = decode_state_catalog(&package.catalogs[0]).unwrap();
    state.chunks[0].hash = crypto::sha256(&package.state[0]);
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    for record in &package.state {
        digest.update(record);
    }
    state.aggregate = digest.finalize().into();
    package.catalogs[0] = state_catalog(&state).unwrap();
    super::recommit_catalog(&mut package, 0);
    assert_eq!(validate_keyless(&package), Err(Error::Invalid));
}

#[test]
fn declared_resource_limits_refuse_before_artifact_allocation() {
    let mut count_bomb = b"AVBC\0\x01\x02".to_vec();
    u64_bytes(&mut count_bomb, RECORD_LIMIT + 1);
    assert_eq!(read_stream(&count_bomb, 2), Err(Error::ResourceLimit));
    let mut domain_bomb = b"AVBD\0\x01\0\x01".to_vec();
    u64_bytes(&mut domain_bomb, RECORD_LIMIT + 1);
    assert!(matches!(
        domain::decode(&domain_bomb),
        Err(Error::ResourceLimit)
    ));
    assert_eq!(
        catalog::prefix_encode(&[(1, "x".repeat(ID_LIMIT as usize + 1))]),
        Err(Error::ResourceLimit)
    );
    let artifact = Artifact {
        total: IMAGE_LIMIT,
        aggregate: [0; 32],
        chunks: vec![
            catalog::Chunk {
                length: CHUNK + 222,
                hash: [0; 32],
                nonce: [0; 24]
            };
            25
        ],
    };
    let images = Images {
        objects: (1..=11)
            .map(|n| Image {
                id: [n; 32],
                selection: 2,
                artifact: artifact.clone(),
            })
            .collect(),
        parents: Vec::new(),
        references: Vec::new(),
    };
    assert_eq!(
        Images::decode(&images.encode().unwrap()),
        Err(Error::ResourceLimit)
    );
    let mut declaration = Declaration {
        count: 0,
        length: CATALOG_LIMIT + 1,
        hash: [0; 32],
        slices: Vec::new(),
    };
    let mut bytes = Vec::new();
    declaration.write(&mut bytes);
    assert_eq!(
        Declaration::read(&mut Reader(&bytes)),
        Err(Error::ResourceLimit)
    );
    // A declaration missing its slice hash cannot be read.
    declaration.length = 15;
    bytes.clear();
    declaration.write(&mut bytes);
    assert_eq!(Declaration::read(&mut Reader(&bytes)), Err(Error::Invalid));
}

#[tokio::test]
async fn metadata_join_authentication_does_not_relax_complete_seed_validation() {
    let (_, _, _, _, mut package) = specimen().await;
    let metadata = download::Metadata {
        descriptor: package.descriptor.clone(),
        catalogs: package.catalogs.clone(),
        state: package.state.clone(),
        manifest: package.manifest.clone(),
    };
    let joined = download::decrypt(&metadata, &package_key()).unwrap();
    assert!(!joined.index.objects.is_empty());
    assert!(!joined.capture.snapshot.tables.task_attachments.is_empty());
    package.images.clear();
    assert!(validate_keyless(&package).is_err());
    assert!(decrypt_domain(&package, &package_key()).is_err());
    assert!(attachment_index(&package, &package_key()).is_err());
    let mut tampered = metadata;
    tampered.manifest[0][210] ^= 1;
    assert!(download::decrypt(&tampered, &package_key()).is_err());
}
