use super::super::hit_test::task_list_status_area;
use super::super::preview::task_preview_fields_line;
use super::super::view_model::TaskListProjection;
use super::*;
use crate::tui::widgets::title_cell;

#[test]
fn task_status_at_position_only_hits_status_column() {
    let list = TaskListFixture::new(vec![task_list_item("task")]);
    let source = list.source();
    let table_state = TableState::default();
    let area = Rect::new(0, 0, 140, 10);
    let task_id = list.tasks[0].task.id.clone();

    let projection = TaskListProjection::from_table_state(
        &source.view,
        &table_state,
        area.height.saturating_sub(1) as usize,
    );
    let status_area = task_list_status_area(&source, &projection, area, 1);
    let hit = task_row_status_at_position(&source, &table_state, area, status_area.x, 2).unwrap();
    assert_eq!(hit.task_index, 0);
    assert_eq!(hit.task_id, task_id);

    assert!(
        task_row_status_at_position(&source, &table_state, area, status_area.x - 1, 2).is_none()
    );
    assert!(
        task_row_status_at_position(
            &source,
            &table_state,
            area,
            status_area.x.saturating_add(status_area.width),
            2
        )
        .is_none()
    );
}

#[test]
fn epic_status_hit_testing_tracks_parent_and_expanded_child_rows() {
    for width in [64, 120] {
        let collapsed = epic_fixture(false);
        let collapsed_source = collapsed.source();
        let table_state = TableState::default();
        let area = Rect::new(0, 0, width, 5);
        let projection = TaskListProjection::from_table_state(
            &collapsed_source.view,
            &table_state,
            area.height.saturating_sub(1) as usize,
        );
        let status_area = task_list_status_area(&collapsed_source, &projection, area, 0);
        let parent_hit = task_row_status_at_position(
            &collapsed_source,
            &table_state,
            area,
            status_area.x,
            area.y + 1,
        )
        .unwrap();
        assert_eq!(parent_hit.task_id, collapsed.tasks[0].task.id);

        let expanded = epic_fixture(true);
        let expanded_source = expanded.source();
        let projection = TaskListProjection::from_table_state(
            &expanded_source.view,
            &table_state,
            area.height.saturating_sub(1) as usize,
        );
        let status_area = task_list_status_area(&expanded_source, &projection, area, 1);
        let child_hit = task_row_status_at_position(
            &expanded_source,
            &table_state,
            area,
            status_area.x,
            area.y + 2,
        )
        .unwrap();
        assert_eq!(child_hit.task_id, expanded.tasks[1].task.id);
    }
}

#[test]
fn task_status_at_position_respects_wide_sidebar_offset() {
    let list = TaskListFixture::new(vec![task_list_item("task")]);
    let source = list.source();
    let table_state = TableState::default();
    let area = Rect::new(26, 2, 114, 18);
    let task_id = list.tasks[0].task.id.clone();

    let projection = TaskListProjection::from_table_state(
        &source.view,
        &table_state,
        area.height.saturating_sub(1) as usize,
    );
    let status_area = task_list_status_area(&source, &projection, area, 1);
    let hit = task_row_status_at_position(&source, &table_state, area, status_area.x, 4).unwrap();

    assert_eq!(hit.task_index, 0);
    assert_eq!(hit.task_id, task_id);
}

#[test]
fn compact_status_column_shows_single_letter_header_and_icon() {
    let mut list = TaskListFixture::new(vec![task_list_item("task")]);
    list.table.compact_status = true;

    let source = list.source();
    let area = Rect::new(0, 0, 140, 8);
    let buffer = render_task_list_buffer(&source, area.width, area.height);
    let table_state = TableState::default();
    let projection = TaskListProjection::from_table_state(
        &source.view,
        &table_state,
        area.height.saturating_sub(1) as usize,
    );
    let visual_row = source.view.visual_row_for(0).unwrap();
    let status_area = task_list_status_area(&source, &projection, area, visual_row as u16);

    assert_eq!(status_area.width, 1);
    assert_eq!(buffer[(status_area.x, area.y)].symbol(), "S");
    assert_eq!(buffer[(status_area.x, status_area.y)].symbol(), "□");
    let rendered = buffer_text(&buffer);
    assert!(!rendered.contains("STATUS"), "{rendered}");
    assert!(!rendered.contains("□ todo"), "{rendered}");

    let hit =
        task_row_status_at_position(&source, &table_state, area, status_area.x, status_area.y)
            .unwrap();
    assert_eq!(hit.task_index, 0);
    assert!(
        task_row_status_at_position(
            &source,
            &table_state,
            area,
            status_area.x.saturating_add(1),
            status_area.y,
        )
        .is_none()
    );
}

#[test]
fn deleted_row_marks_metadata_column_and_keeps_status() {
    let mut item = task_list_item("original title");
    item.task.deleted = true;

    let buffer = render_task_row_buffer(&item, None);
    let rendered = buffer_text(&buffer);
    let cells = build_task_row_cells(
        &item,
        TaskTimeContext {
            now_seconds: 0,
            render_mode: TaskListRenderMode::Flat,
            due_order: false,
            show_due: true,
        },
        None,
        &[12, 40, 12, 6, 9, 10, 3, 5, 6],
        TaskRowState {
            selected: false,
            focused: false,
            marked: false,
        },
        EpicSelectionContext::default(),
    );

    assert!(rendered.contains("original title"));
    assert!(!rendered.contains("deleted original title"));
    assert_eq!(cells[3].to_string(), "×");
    assert_eq!(cells[5].to_string(), "□ todo");
    assert!(
        task_preview_fields_line(&item)
            .to_string()
            .contains("deleted yes")
    );
}

#[test]
fn recurring_rows_and_preview_show_series_context() {
    let mut item = task_list_item("daily review");
    let series_id: aven_core::recurrence::RecurrenceSeriesId = "7KQ9A1X4MV2P8D6R".parse().unwrap();
    item.recurrence = Some(crate::query::TaskRecurrenceSummary {
        series_id: series_id.clone(),
        series_ref: "RCR-A1".to_string(),
        slot_on: "2026-07-20".to_string(),
        rule_label: "daily at 09:00".to_string(),
        timezone: "Europe/Helsinki".to_string(),
        lifecycle: aven_core::recurrence::RecurrenceSeriesState::Active,
        outcome: None,
        projection_state: aven_core::recurrence::RecurrenceProjectionState::Projected,
    });
    item.recurrence_group = Some(crate::query::RecurrenceTaskGroup {
        series_id,
        series_ref: "RCR-A1".to_string(),
        counts: crate::query::RecurrenceCounts {
            series_ref: "RCR-A1".to_string(),
            completed: 4,
            skipped: 2,
            missed: 1,
            latest_slot_on: Some("2026-07-20".to_string()),
            ..crate::query::RecurrenceCounts::default()
        },
    });

    let row_text = title_cell(&item, 80).to_string();
    assert!(row_text.contains("↻"));
    assert!(row_text.contains("2026-07-20"));
    assert!(row_text.contains("RCR-A1"));
    assert!(row_text.contains("✓4"));
    assert!(row_text.contains("↷2"));
    assert!(row_text.contains("×1"));

    let preview = super::super::preview::task_preview_lines(&item, 80, 20)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(preview.contains("RCR-A1"));
    assert!(preview.contains("slot 2026-07-20"));
    assert!(preview.contains("daily at 09:00"));
    assert!(preview.contains("active"));
    assert!(preview.contains("4 completed"));
    assert!(preview.contains("2 skipped"));
    assert!(preview.contains("1 missed"));
}
