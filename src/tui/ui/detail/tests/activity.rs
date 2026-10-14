use super::*;

#[test]
fn task_activity_is_collapsed_by_default_and_expands_with_idle_context() {
    let mut item = detail_test_item();
    item.queue.idle_seconds = Some(12 * 60);
    let target = crate::query::RecentActionTarget {
        display_ref: Some(item.display_ref.clone()),
        title: Some(item.task.title.clone()),
        project_key: Some(item.task.project_key.clone()),
        status: Some(item.task.status.as_str().to_string()),
        deleted: false,
    };
    item.activity = vec![
        crate::query::RecentActionItem {
            change_id: "note-change-id".to_string(),
            entity_type: "task".to_string(),
            entity_id: item.task.id.to_string(),
            op_type: crate::change_log::op_type::NOTE_ADD.to_string(),
            field: Some("notes".to_string()),
            created_at: item.task.queue_activity_at.clone(),
            synced: false,
            target: target.clone(),
            verb: "note".to_string(),
            summary: "added note: Fix token refresh race".to_string(),
            detail: Some("Confirmed race".to_string()),
            accent: "blue".to_string(),
            grouped_change_count: 1,
        },
        crate::query::RecentActionItem {
            change_id: "label-change-id".to_string(),
            entity_type: "task".to_string(),
            entity_id: item.task.id.to_string(),
            op_type: crate::change_log::op_type::LABEL_ADD.to_string(),
            field: Some("labels".to_string()),
            created_at: "2026-06-20T11:59:00Z".to_string(),
            synced: false,
            target: target.clone(),
            verb: "label".to_string(),
            summary: "added label: Fix token refresh race".to_string(),
            detail: Some("backend".to_string()),
            accent: "green".to_string(),
            grouped_change_count: 1,
        },
        crate::query::RecentActionItem {
            change_id: "description-change-id".to_string(),
            entity_type: "task".to_string(),
            entity_id: item.task.id.to_string(),
            op_type: crate::change_log::op_type::SET_FIELD.to_string(),
            field: Some("description".to_string()),
            created_at: "2026-06-20T11:58:00Z".to_string(),
            synced: false,
            target,
            verb: "details".to_string(),
            summary: "edited description: Fix token refresh race".to_string(),
            detail: Some("## Goal Turn planning notes into tasks".to_string()),
            accent: "blue".to_string(),
            grouped_change_count: 1,
        },
    ];

    let collapsed = detail_body_lines(&item, 100, None)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(collapsed.contains("ACTIVITY"));
    assert!(collapsed.contains("Show 3 events"));
    assert!(!collapsed.contains("added note"));
    assert!(collapsed.find("ACTIVITY").unwrap() > collapsed.find("WHAT THIS UNLOCKS").unwrap());

    let expanded_sections = [DetailSection::Activity].into_iter().collect();
    let rendered =
        detail_body_lines_with_pending_images(&item, 100, None, &expanded_sections, None, &[])
            .0
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

    assert!(rendered.contains(&local_activity_timestamp_display(
        &item.task.queue_activity_at
    )));
    let note_line = rendered
        .lines()
        .find(|line| line.contains("✎ added note"))
        .unwrap();
    assert!(note_line.contains("added note · Confirmed race"));
    assert!(!note_line.contains("Fix token refresh race"));
    assert!(rendered.contains("added label · backend"));
    assert!(rendered.contains("edited description"));
    assert!(!rendered.contains("Turn planning notes into tasks"));
    assert!(rendered.contains("idle 12m"));
    assert!(!rendered.contains("idle starts here"));
    assert!(rendered.contains("Hide activity"));
}
