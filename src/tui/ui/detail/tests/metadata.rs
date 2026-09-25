use super::*;

#[test]
fn custom_metadata_heading_uses_note_heading_and_keycap_styles() {
    let mut item = detail_test_epic_item();
    item.metadata = vec![aven_core::metadata::TaskMetadataValue {
        field_id: crate::ids::MetadataFieldId::new(),
        key: "owner".to_string(),
        value: "Alex".to_string(),
    }];
    let children = detail_epic_children(&item, None);
    let body = build_detail_body_document(&item, &children, 80, &BTreeSet::new(), None, &[]);
    let metadata = body
        .lines
        .iter()
        .find(|line| line.to_string().starts_with("CUSTOM METADATA"))
        .unwrap();
    let notes = body
        .lines
        .iter()
        .find(|line| line.to_string().starts_with("NOTES"))
        .unwrap();
    assert_eq!(metadata.to_string(), "CUSTOM METADATA (e m edit)");
    assert_eq!(metadata.spans[0].style, notes.spans[0].style);
    for key in ["e", "m"] {
        assert_eq!(
            metadata
                .spans
                .iter()
                .find(|span| span.content == key)
                .unwrap()
                .style,
            notes.spans[2].style
        );
    }
}

#[test]
fn detail_metadata_includes_operational_fields() {
    let item = detail_test_item();
    let rendered = detail_metadata_lines(&item, 31)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("PROJECT\n● app"));
    assert!(rendered.contains("STATUS\n● active"));
    assert!(rendered.contains("PRIORITY\n▲ urgent"));
    assert!(rendered.contains("LABELS\nbug, mobile"));
    assert!(rendered.contains("AVAILABILITY\nnone\n\nDUE\nnone\n\nREF"));
    assert!(rendered.contains("CONFLICTS\ntitle"));
    assert!(rendered.contains("current  Fix token refresh race"));
    assert!(rendered.contains("incoming Fix refresh race"));
    assert!(rendered.contains("c a current · c r incoming · c m manual"));
}

#[test]
fn detail_metadata_includes_recurrence_state_and_history() {
    let mut item = detail_test_item();
    let series_id: aven_core::recurrence::RecurrenceSeriesId = "7KQ9A1X4MV2P8D6R".parse().unwrap();
    item.recurrence = Some(crate::query::TaskRecurrenceSummary {
        series_id: series_id.clone(),
        series_ref: "RCR-A1".to_string(),
        slot_on: "2026-07-20".to_string(),
        rule_label: "weekdays at 09:00".to_string(),
        timezone: "Europe/Helsinki".to_string(),
        lifecycle: aven_core::recurrence::RecurrenceSeriesState::Paused,
        outcome: Some(aven_core::recurrence::RecurrenceOutcome::Skipped),
        projection_state: aven_core::recurrence::RecurrenceProjectionState::Archived,
    });
    item.recurrence_group = Some(crate::query::RecurrenceTaskGroup {
        series_id,
        series_ref: "RCR-A1".to_string(),
        counts: crate::query::RecurrenceCounts {
            series_ref: "RCR-A1".to_string(),
            completed: 8,
            skipped: 3,
            missed: 2,
            ..crate::query::RecurrenceCounts::default()
        },
    });

    let rendered = detail_metadata_lines(&item, 31)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("RECURRENCE\n↻ RCR-A1"));
    assert!(rendered.contains("schedule weekdays at 09:00"));
    assert!(rendered.contains("slot 2026-07-20"));
    assert!(rendered.contains("zone Europe/Helsinki"));
    assert!(rendered.contains("lifecycle paused"));
    assert!(rendered.contains("outcome skipped"));
    assert!(rendered.contains("projection archived"));
    assert!(rendered.contains("history t r h"));
    assert!(rendered.contains("SERIES HISTORY\ncompleted 8\nskipped 3\nmissed 2"));
}

#[test]
fn detail_metadata_splits_availability_across_bounded_lines() {
    let mut item = detail_test_item();
    item.task.available_at = Some("2999-07-17T12:30:00Z".to_string());

    let lines = detail_metadata_lines(&item, 20);
    let availability = lines
        .iter()
        .position(|line| line.to_string() == "AVAILABILITY")
        .unwrap();
    let values = &lines[availability + 1..availability + 3];

    assert!(values.iter().all(|line| !line.to_string().is_empty()));
    assert!(values.iter().all(|line| line.width() <= 20));
}

#[test]
fn detail_metadata_marks_epics_and_counts_children() {
    let item = detail_test_epic_item();

    let rendered = detail_metadata_lines(&item, 31)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains(" EPIC "));
    assert!(rendered.contains("CHILDREN\nopen=1 total=2"));
    assert!(!rendered.contains("APP-CHLD"));
    assert!(!rendered.contains("Build the first child task"));
    assert!(!rendered.contains("APP-DONE"));
}

#[test]
fn detail_metadata_uses_shared_epic_rollup_semantics() {
    let mut item = detail_test_epic_item();
    item.epic_rollup = Some(crate::query::EpicRollup {
        total: 4,
        open: 2,
        done: 1,
        canceled: 1,
        blocked: 1,
        overdue: 1,
        ready: 1,
        latest_activity_at: "2026-06-21T00:00:00Z".to_string(),
    });

    let rendered = detail_metadata_lines(&item, 40)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("CHILDREN\n2 open · 1 done · 1 canceled"));
    assert!(rendered.contains("1 overdue · 1 blocked · 1 ready"));
}

#[test]
fn detail_metadata_marks_epics_without_children() {
    let mut item = detail_test_item();
    item.task.is_epic = true;

    let rendered = detail_metadata_lines(&item, 31)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains(" EPIC "));
    assert!(rendered.contains("CHILDREN\nopen=0 total=0\nnone"));
}

#[test]
fn detail_copy_targets_map_displayed_values() {
    let item = detail_test_item();
    let expected = [
        (2, 5, item.display_ref.clone()),
        (88, 23, item.display_ref.clone()),
        (88, 26, local_timestamp_display(&item.task.created_at)),
        (88, 29, local_timestamp_display(&item.task.updated_at)),
    ];

    for (column, row, value) in expected {
        assert_eq!(
            detail_copy_target_at(&item, 120, 40, column, row).map(|hit| hit.value),
            Some(value)
        );
    }
    assert!(detail_copy_target_at(&item, 120, 40, 87, 23).is_none());
    assert!(detail_copy_target_at(&item, 120, 40, 88, 22).is_none());
    assert!(detail_copy_target_at(&item, 80, 40, 70, 26).is_none());
}

#[test]
fn detail_metadata_target_maps_editable_values_without_body_conflicts() {
    let item = detail_test_item();
    let expected = [
        (5, DetailMetadataTarget::Project),
        (8, DetailMetadataTarget::Status),
        (11, DetailMetadataTarget::Priority),
        (14, DetailMetadataTarget::Labels),
        (17, DetailMetadataTarget::Availability),
        (20, DetailMetadataTarget::Due),
    ];

    for (row, target) in expected {
        assert_eq!(
            detail_metadata_target_at(&item, 120, 40, 88, row),
            Some((target, 88, row))
        );
    }
    assert_eq!(detail_metadata_target_at(&item, 120, 40, 88, 7), None);
    assert_eq!(detail_metadata_target_at(&item, 120, 40, 50, 8), None);
    assert_eq!(detail_metadata_target_at(&item, 80, 40, 70, 11), None);
}

#[test]
fn detail_metadata_target_maps_both_date_summary_lines() {
    let mut item = detail_test_item();
    item.task.available_at = Some("2999-07-17T12:30:00Z".to_string());
    item.task.due_on = Some("2999-07-18".to_string());

    for (row, target) in [
        (17, DetailMetadataTarget::Availability),
        (18, DetailMetadataTarget::Availability),
        (21, DetailMetadataTarget::Due),
        (22, DetailMetadataTarget::Due),
    ] {
        assert_eq!(
            detail_metadata_target_at(&item, 120, 40, 88, row),
            Some((target, 88, row))
        );
    }
}
