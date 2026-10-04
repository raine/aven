use super::*;

#[test]
fn control_messages_keep_fixed_json_bytes() {
    assert_eq!(
        serde_json::to_vec(&bootstrap::Operation::ClaimSetup {
            bytes: vec![0, 1, 2, 255]
        })
        .unwrap(),
        br#"{"ClaimSetup":{"bytes":"AAEC/w=="}}"#
    );
    assert_eq!(
        serde_json::to_vec(&bootstrap::Reply::Published(vec![0, 1, 2, 255])).unwrap(),
        br#"{"Published":"AAEC/w=="}"#
    );
    let operation: enrollment::Operation = enrollment::Operation::Post {
        vault: [0; 32],
        handle: [1; 32],
        request: vec![0, 1, 2, 255],
    };
    let expected = format!(
        "{{\"Post\":{{\"vault\":[{}],\"handle\":[{}],\"request\":\"AAEC/w==\"}}}}",
        ["0"; 32].join(","),
        ["1"; 32].join(",")
    );
    assert_eq!(serde_json::to_vec(&operation).unwrap(), expected.as_bytes());
    assert_eq!(
        serde_json::to_vec(&tail::BatchOperation::Append {
            records: vec![tail::BatchRecord(vec![0, 1, 2, 255])]
        })
        .unwrap(),
        br#"{"Append":{"records":["AAEC/w=="]}}"#
    );
    assert_eq!(
        serde_json::to_vec(&images::Reply::Chunk(vec![0, 1, 2, 255])).unwrap(),
        br#"{"Chunk":"AAEC/w=="}"#
    );
    assert!(
        serde_json::from_slice::<bootstrap::Operation>(
            br#"{"ClaimSetup":{"bytes":"AAEC/w==","extra":0}}"#
        )
        .is_err()
    );
}

#[test]
fn bootstrap_batch_keeps_fixed_header_and_binary_prefix() {
    let hex = "00".repeat(32);
    let json = format!(
        "{{\"vault\":\"{hex}\",\"genesis\":\"{hex}\",\"bootstrap\":\"{hex}\",\"commitment\":\"{hex}\",\"records\":[{{\"component\":\"03\",\"index\":0,\"len\":4}}]}}"
    );
    let mut expected = b"AVBU\x00\x01".to_vec();
    expected.extend_from_slice(&(json.len() as u32).to_be_bytes());
    expected.extend_from_slice(json.as_bytes());
    expected.extend_from_slice(&[0, 1, 2, 255]);
    let header = bootstrap::batch::Header {
        vault: [0; 32],
        genesis: [0; 32],
        bootstrap: [0; 32],
        commitment: [0; 32],
        records: vec![bootstrap::batch::Slot {
            component: bootstrap::Component::Manifest,
            index: 0,
            len: 4,
        }],
    };
    assert_eq!(
        bootstrap::batch::encode(&header, &[&[0, 1, 2, 255]]).unwrap(),
        expected
    );
}

#[test]
fn compact_mapping_and_collection_bounds_remain_exact() {
    let mapping = tail::batch::CompactMapping {
        operation_id: "op".into(),
        sequence: 42,
        commitment: [0; 32],
    };
    assert_eq!(serde_json::to_vec(&mapping).unwrap(), br#"{"operation_id":"op","sequence":42,"commitment":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}"#);
    let crowded =
        serde_json::json!({"Resolve": {"operation_ids": vec!["a"; tail::BATCH_COUNT + 1]}});
    assert!(serde_json::from_value::<tail::BatchOperation>(crowded).is_err());
    let huge = serde_json::json!({"Append": {"records": ["A".repeat(crate::base64_bytes::encoded_len(tail::RECORD_LIMIT) + 4)]}});
    assert!(serde_json::from_value::<tail::BatchOperation>(huge).is_err());
}
