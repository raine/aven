use super::*;

fn input(key: &str, bytes: usize) -> TaskMetadataInput {
    TaskMetadataInput {
        expected_field_id: None,
        key: key.into(),
        value: "x".repeat(bytes),
    }
}

fn values(count: usize, bytes: usize) -> Vec<(String, String)> {
    (0..count)
        .map(|i| (format!("key{i}"), "x".repeat(bytes)))
        .collect()
}

#[test]
fn task_admission_checks_each_dimension_independently() {
    for (count, bytes, set, remove, accepted) in [
        // Both dimensions over: incremental reductions and no growth.
        (130, 256, vec![], vec![], true),
        (130, 256, vec![input("key0", 256)], vec![], true),
        (130, 256, vec![input("key0", 255)], vec![], true),
        (130, 256, vec![], vec!["key0"], true),
        (130, 256, vec![input("key0", 257)], vec![], false),
        (130, 256, vec![input("extra", 0)], vec![], false),
        // Reducing count does not buy permission to grow over-limit bytes.
        (130, 256, vec![input("key0", 512)], vec!["key1"], true),
        (130, 256, vec![input("key0", 513)], vec!["key1"], false),
        // Reducing bytes does not buy permission to grow over-limit count.
        (
            130,
            256,
            vec![input("key0", 0), input("extra", 0)],
            vec![],
            false,
        ),
        // A dimension below its ceiling may grow independently.
        (130, 1, vec![input("key0", 4096)], vec![], true),
        (9, 4096, vec![input("extra", 0)], vec![], true),
        // Replacement under a different key preserves the same two metrics.
        (
            130,
            256,
            vec![input("replacement", 256)],
            vec!["key0"],
            true,
        ),
        // A previously valid metric must never acquire a new overage.
        (128, 256, vec![input("extra", 0)], vec![], false),
        (128, 256, vec![input("key0", 257)], vec![], false),
        (128, 256, vec![input("key0", 256)], vec![], true),
    ] {
        let remove = remove.into_iter().map(str::to_string).collect::<Vec<_>>();
        validate_metadata_update(&set, &remove).unwrap();
        assert_eq!(
            validate_task_metadata_result_limits(values(count, bytes), &set, &remove).is_ok(),
            accepted,
            "count={count} bytes={bytes} set={set:?} remove={remove:?}"
        );
    }
}

#[test]
fn recurrence_result_limits_remain_strict() {
    assert!(validate_metadata_result_limits(values(130, 256), &[], &["key0".into()]).is_err());
}

#[test]
fn metadata_input_bounds_still_use_utf8_bytes() {
    for (value, accepted) in [
        ("x".repeat(4096), true),
        ("x".repeat(4097), false),
        ("é".repeat(2048), true),
        (format!("{}x", "é".repeat(2048)), false),
    ] {
        let mut set = input("key", 0);
        set.value = value;
        assert_eq!(validate_metadata_update(&[set], &[]).is_ok(), accepted);
    }
    let set = (0..129)
        .map(|i| input(&format!("key{i}"), 1))
        .collect::<Vec<_>>();
    assert!(validate_metadata_update(&set, &[]).is_err());
    let set = (0..9)
        .map(|i| input(&format!("key{i}"), 4096))
        .collect::<Vec<_>>();
    assert!(validate_metadata_update(&set, &[]).is_err());
    assert!(validate_metadata_update(&[input("aven.private", 1)], &[]).is_err());
    assert!(validate_metadata_update(&[input("same", 1), input("SAME", 1)], &[]).is_err());
    assert!(validate_metadata_update(&[input("same", 1)], &["SAME".into()]).is_err());
}
