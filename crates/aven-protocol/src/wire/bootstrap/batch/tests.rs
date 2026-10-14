use super::*;

fn header(records: Vec<Slot>) -> Header {
    Header {
        vault: [1; 32],
        genesis: [2; 32],
        bootstrap: [3; 32],
        commitment: [4; 32],
        records,
    }
}

fn slot(component: Component, index: u64, len: usize) -> Slot {
    Slot {
        component,
        index,
        len: len as u64,
    }
}

fn error(bytes: &[u8]) -> String {
    decode(bytes)
        .err()
        .expect("batch must be refused")
        .to_string()
}

#[test]
fn batch_round_trips_records_in_header_order() {
    let a = vec![7; 10];
    let b = vec![8; 3];
    let h = header(vec![
        slot(Component::State, 0, a.len()),
        slot(Component::Image([9; 32]), 2, b.len()),
    ]);
    let bytes = encode(&h, &[&a, &b]).unwrap();
    let batch = decode(&bytes).unwrap();
    assert_eq!(batch.header, h);
    assert_eq!(batch.records, vec![a.as_slice(), b.as_slice()]);
}

#[test]
fn malformed_truncated_or_trailing_batches_are_refused() {
    let record = vec![5; 16];
    let h = header(vec![slot(Component::Manifest, 0, record.len())]);
    let good = encode(&h, &[&record]).unwrap();

    assert_eq!(
        error(&good[..good.len() - 1]),
        "error bootstrap-batch-framing"
    );
    let mut trailing = good.clone();
    trailing.push(0);
    assert_eq!(error(&trailing), "error bootstrap-batch-framing");
    assert_eq!(error(&good[..5]), "error bootstrap-batch-framing");

    let mut magic = good.clone();
    magic[0] ^= 1;
    assert_eq!(error(&magic), "error bootstrap-batch-framing");
    let mut version = good.clone();
    version[5] = 2;
    assert_eq!(error(&version), "error bootstrap-batch-version");

    // A header length past the end of the body.
    let mut long = good.clone();
    long[6..10].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_eq!(error(&long), "error bootstrap-batch-framing");

    // A header that is not the strict JSON shape.
    let json = br#"{"vault":"00","records":[]}"#;
    let mut bad = MAGIC.to_vec();
    bad.extend_from_slice(&VERSION.to_be_bytes());
    bad.extend_from_slice(&(json.len() as u32).to_be_bytes());
    bad.extend_from_slice(json);
    assert_eq!(error(&bad), "error bootstrap-batch-framing");
}

/// Encodes `header` with arbitrary record bytes, bypassing encode's checks.
fn raw(header: &Header) -> Vec<u8> {
    let json = serde_json::to_vec(header).unwrap();
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&VERSION.to_be_bytes());
    out.extend_from_slice(&(json.len() as u32).to_be_bytes());
    out.extend_from_slice(&json);
    for slot in &header.records {
        out.extend(std::iter::repeat_n(
            0,
            slot.len.min(MAX_BYTES as u64) as usize,
        ));
    }
    out
}

#[test]
fn overflowing_oversized_or_crowded_batches_are_refused() {
    // Declared lengths that overflow never read past the body.
    let overflow = header(vec![
        Slot {
            component: Component::State,
            index: 0,
            len: u64::MAX,
        },
        slot(Component::State, 1, 1),
    ]);
    assert!(error(&raw(&overflow)).starts_with("error bootstrap-batch"));

    let oversized = header(vec![slot(Component::State, 0, MAX_REQUEST_BYTES + 1)]);
    assert_eq!(error(&raw(&oversized)), "error bootstrap-batch-shape");

    let payload = header(
        (0..5)
            .map(|index| slot(Component::State, index, MAX_REQUEST_BYTES))
            .collect(),
    );
    assert!(error(&raw(&payload)).starts_with("error bootstrap-batch"));

    let crowded = header(
        (0..=MAX_RECORDS as u64)
            .map(|index| slot(Component::State, index, 1))
            .collect(),
    );
    assert_eq!(error(&raw(&crowded)), "error bootstrap-batch-shape");
    assert_eq!(
        error(&raw(&header(Vec::new()))),
        "error bootstrap-batch-shape"
    );

    let duplicate = header(vec![
        slot(Component::State, 0, 1),
        slot(Component::State, 0, 1),
    ]);
    assert_eq!(error(&raw(&duplicate)), "error bootstrap-batch-shape");

    let wide = header(
        (0..MAX_RECORDS as u64)
            .map(|index| slot(Component::Image([index as u8; 32]), u64::MAX - index, 1))
            .collect(),
    );
    assert!(header_len(&wide).is_none());
    assert_eq!(error(&raw(&wide)), "error bootstrap-batch-framing");
}

#[test]
fn catalog_slices_never_share_a_batch_with_data() {
    let mixed = header(vec![
        slot(Component::DataCatalog, 0, 1),
        slot(Component::State, 0, 1),
    ]);
    assert_eq!(error(&raw(&mixed)), "error bootstrap-batch-mixed");
    assert!(encode(&mixed, &[&[0], &[0]]).is_err());
}

#[test]
fn maximal_batch_fits_its_body_limit() {
    let records: Vec<Vec<u8>> = (0..4).map(|_| vec![0; MAX_REQUEST_BYTES - 1024]).collect();
    let refs: Vec<&[u8]> = records.iter().map(Vec::as_slice).collect();
    let h = header(
        (0..4)
            .map(|index| {
                slot(
                    Component::Image([255; 32]),
                    u64::MAX - index,
                    records[0].len(),
                )
            })
            .collect(),
    );
    assert!(encode(&h, &refs).unwrap().len() <= MAX_BYTES);
}
