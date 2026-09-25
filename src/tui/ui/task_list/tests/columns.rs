use super::super::layout::TableLayout;
use super::super::sizing::*;
use super::*;
use crate::tui::widgets::priority_icon;
use unicode_width::UnicodeWidthStr;

#[test]
fn ref_header_aligns_with_task_refs() {
    let mut list = TaskListFixture::new(vec![task_list_item("Aligned ref")]);
    list.view_state.query = TaskQuery::All;
    let buffer = render_task_list_buffer(&list.source(), 80, 4);
    let header = text_in_cell(&buffer, Rect::new(0, 0, 80, 1));
    let task = text_in_cell(&buffer, Rect::new(0, 1, 80, 1));
    let header_prefix = header.split_once("REF").unwrap().0;
    let task_prefix = task.split_once("APP-").unwrap().0;

    assert_eq!(header_prefix.width(), task_prefix.width());
}

#[test]
fn reordered_columns_align_headers_content_and_status_hits() {
    for width in [64, 120] {
        let mut item = task_list_item("Short title");
        item.labels = vec!["ios".to_string()];
        item.has_notes = true;
        item.task.priority = TaskPriority::High;
        item.task.due_on = Some("2999-01-01".to_string());
        let mut list = TaskListFixture::new(vec![item]);
        list.view_state.query = TaskQuery::All;
        for rotation in 0..9 {
            list.table.columns = TableColumn::ALL.to_vec();
            list.table.columns.rotate_left(rotation);
            let order = list.table.columns.clone();
            let area = Rect::new(5, 2, width, 5);
            let mut state = TableState::default();
            let model = build_task_list_render_model(
                &list.source(),
                &mut state,
                Focus::Tasks,
                area,
                None,
                &BTreeSet::new(),
            );
            let mut previous_right = area.x;
            for column in order {
                let cell = model.layout.cell(column, area);
                if cell.width == 0 {
                    continue;
                }
                assert!(
                    cell.x >= previous_right,
                    "{column:?} at rotation {rotation}"
                );
                previous_right = cell.right();
            }
            let buffer = render_task_list_buffer(&list.source(), width, 5);
            for (column, header, content) in [
                (TableColumn::Ref, "REF", "APP-"),
                (TableColumn::Title, "TITLE", "Short title"),
                (TableColumn::Labels, "LABELS", "ios"),
                (TableColumn::Metadata, "", "✎"),
                (TableColumn::Project, "PROJECT", "app"),
                (TableColumn::Status, "STATUS", "todo"),
                (TableColumn::Priority, "P", priority_icon("high")),
                (TableColumn::Time, "AGE", ""),
                (TableColumn::Due, "DUE", "Jan1"),
            ] {
                let cell = model.layout.cell(column, Rect::new(0, 0, width, 1));
                if column == TableColumn::Labels && width < 90 {
                    assert_eq!(cell.width, 0);
                    continue;
                }
                let header = header.chars().take(cell.width as usize).collect::<String>();
                assert!(text_in_cell(&buffer, cell).contains(&header), "{column:?}");
                let cell = Rect { y: 1, ..cell };
                assert!(
                    text_in_cell(&buffer, cell).contains(content),
                    "{column:?} at rotation {rotation} width {width}"
                );
            }
            let row = Rect::new(area.x, area.y + 1, width, 1);
            let status = model.layout.cell(TableColumn::Status, row);
            for x in area.x..area.right() {
                let hit = task_row_status_at_position(&list.source(), &state, area, x, row.y);
                assert_eq!(hit.is_some(), x >= status.x && x < status.right());
                if let Some(hit) = hit {
                    assert_eq!(hit.task_id, list.tasks[0].task.id);
                }
            }
        }
    }
}

#[test]
fn configured_subset_hides_columns_and_keeps_status_geometry() {
    let mut item = task_list_item("Visible title");
    item.labels = vec!["ios".to_string()];
    item.has_notes = true;
    let mut list = TaskListFixture::new(vec![item]);
    list.view_state.query = TaskQuery::All;
    list.table.columns = vec![TableColumn::Title, TableColumn::Status, TableColumn::Time];

    let area = Rect::new(0, 0, 120, 5);
    let mut state = TableState::default();
    let model = build_task_list_render_model(
        &list.source(),
        &mut state,
        Focus::Tasks,
        area,
        None,
        &BTreeSet::new(),
    );
    let title = model.layout.cell(TableColumn::Title, area);
    let status = model.layout.cell(TableColumn::Status, area);
    let time = model.layout.cell(TableColumn::Time, area);
    assert_eq!(status.x, title.right() + 1);
    assert_eq!(time.x, status.right() + 1);
    for column in [
        TableColumn::Ref,
        TableColumn::Labels,
        TableColumn::Metadata,
        TableColumn::Project,
        TableColumn::Priority,
    ] {
        assert_eq!(model.layout.cell(column, area).width, 0);
    }

    let rendered = buffer_text(&render_task_list_buffer(&list.source(), 120, 5));
    assert!(rendered.contains("TITLE"));
    assert!(rendered.contains("STATUS"));
    assert!(rendered.contains("Visible title"));
    assert!(!rendered.contains("REF"));
    assert!(!rendered.contains("LABELS"));
    assert!(!rendered.contains("PROJECT"));
    assert!(!rendered.contains("P"));
}

#[test]
fn fallback_state_gutter_preserves_single_column_content() {
    let mut item = task_list_item("Fallback title");
    item.labels = vec!["ios".to_string(), "ux".to_string()];
    item.has_notes = true;
    item.task.priority = TaskPriority::High;
    let mut list = TaskListFixture::new(vec![item]);
    list.view_state.query = TaskQuery::All;

    for (column, expected) in [
        (TableColumn::Status, "todo"),
        (TableColumn::Priority, priority_icon("high")),
        (TableColumn::Labels, "ios"),
        (TableColumn::Metadata, "✎"),
        (TableColumn::Project, "app"),
    ] {
        list.table.columns = vec![column];
        let area = Rect::new(0, 0, 120, 4);
        let mut state = TableState::default();
        state.select(Some(0));
        let model = build_task_list_render_model(
            &list.source(),
            &mut state,
            Focus::Tasks,
            area,
            None,
            &BTreeSet::from([list.tasks[0].task.id.clone()]),
        );
        assert_eq!(model.layout.state_column(), None);
        assert_eq!(model.layout.state_gutter(area).width, 3);
        let buffer = render_task_list_buffer_with_selection(&list.source(), 120, 4, true);
        let state_area = model.layout.state_gutter(Rect::new(0, 1, 120, 1));
        assert_eq!(text_in_cell(&buffer, state_area), "›● ");
        let content_area = model.layout.cell(column, Rect::new(0, 1, 120, 1));
        let content = text_in_cell(&buffer, content_area);
        assert!(!content.trim().is_empty(), "{column:?} lost all content");
        assert!(content.contains(expected), "{column:?}: {content:?}");
        if column == TableColumn::Status {
            for x in 0..120 {
                let hit = task_row_status_at_position(&list.source(), &state, area, x, 1);
                assert_eq!(
                    hit.is_some(),
                    x >= content_area.x && x < content_area.right(),
                    "status hit at {x}"
                );
            }
        }
    }
    list.table.columns = vec![TableColumn::Time];
    let buffer = render_task_list_buffer_with_selection(&list.source(), 120, 4, true);
    let mut state = TableState::default();
    state.select(Some(0));
    let model = build_task_list_render_model(
        &list.source(),
        &mut state,
        Focus::Tasks,
        Rect::new(0, 0, 120, 4),
        None,
        &BTreeSet::from([list.tasks[0].task.id.clone()]),
    );
    let time = text_in_cell(
        &buffer,
        model
            .layout
            .cell(TableColumn::Time, Rect::new(0, 1, 120, 1)),
    );
    assert!(!time.trim().is_empty(), "time content was clipped");
}

#[test]
fn fallback_state_gutter_keeps_singletons_usable_at_narrow_widths() {
    let mut item = task_list_item("Narrow fallback");
    item.task.priority = TaskPriority::High;
    let mut list = TaskListFixture::new(vec![item]);
    list.view_state.query = TaskQuery::All;

    for column in [
        TableColumn::Status,
        TableColumn::Priority,
        TableColumn::Time,
    ] {
        list.table.columns = vec![column];
        let area = Rect::new(0, 0, 16, 4);
        let mut state = TableState::default();
        state.select(Some(0));
        let model = build_task_list_render_model(
            &list.source(),
            &mut state,
            Focus::Tasks,
            area,
            None,
            &BTreeSet::from([list.tasks[0].task.id.clone()]),
        );
        let content_area = model.layout.cell(column, Rect::new(0, 1, 16, 1));
        assert!(content_area.width > 0, "{column:?}");
        let buffer = render_task_list_buffer_with_selection(&list.source(), 16, 4, true);
        let content = text_in_cell(&buffer, content_area);
        assert!(!content.trim().is_empty(), "{column:?}: {content:?}");
        assert_eq!(
            text_in_cell(&buffer, model.layout.state_gutter(Rect::new(0, 1, 16, 1))),
            "›● ",
            "{column:?}"
        );
    }
}

#[test]
fn empty_content_singletons_keep_a_visible_state_target() {
    let mut item = task_list_item("Fallback target");
    item.labels = vec!["ios".to_string()];
    let mut list = TaskListFixture::new(vec![item]);
    list.view_state.query = TaskQuery::All;
    for column in [
        TableColumn::Metadata,
        TableColumn::Priority,
        TableColumn::Labels,
    ] {
        list.table.columns = vec![column];
        let width = if column == TableColumn::Labels {
            24
        } else {
            40
        };
        let area = Rect::new(0, 0, width, 4);
        let mut state = TableState::default();
        state.select(Some(0));
        let model = build_task_list_render_model(
            &list.source(),
            &mut state,
            Focus::Tasks,
            area,
            None,
            &BTreeSet::from([list.tasks[0].task.id.clone()]),
        );
        assert_eq!(model.layout.state_column(), None);
        assert!(model.layout.state_gutter(area).width > 0, "{column:?}");
        if column != TableColumn::Labels || width < 90 {
            assert_eq!(model.layout.cell(column, area).width, 0, "{column:?}");
        }
        let buffer = render_task_list_buffer_with_selection(&list.source(), width, 4, true);
        assert_eq!(
            text_in_cell(
                &buffer,
                model.layout.state_gutter(Rect::new(0, 1, width, 1))
            ),
            "›● ",
            "{column:?}"
        );
    }
}

#[test]
fn hidden_ref_keeps_selection_and_marks_on_title() {
    let item = task_list_item("Visible title");
    let mut list = TaskListFixture::new(vec![item]);
    list.view_state.query = TaskQuery::All;
    list.table.columns = vec![TableColumn::Title, TableColumn::Status];
    let area = Rect::new(0, 0, 80, 5);
    let columns = task_list_columns(&list.source(), false);
    let layout = TableLayout::resolve(&columns, &list.table.columns, area.width);
    let cells = build_task_row_cells_for_columns(
        &list.tasks[0],
        TaskTimeContext {
            now_seconds: 0,
            render_mode: TaskListRenderMode::Flat,
            due_order: false,
            show_due: true,
        },
        None,
        TaskListCellLayout {
            widths: &layout.widths(),
            state_column: layout.state_column(),
            compact_status: false,
        },
        TaskRowState {
            selected: true,
            focused: true,
            marked: true,
        },
        EpicSelectionContext::default(),
    );
    assert_eq!(layout.state_column(), Some(TableColumn::Title));
    assert!(
        cells[TableColumn::Title as usize]
            .to_string()
            .starts_with("›● ")
    );
    assert!(
        cells[TableColumn::Title as usize]
            .to_string()
            .contains("Visible title")
    );

    let mut state = TableState::default();
    state.select(Some(0));
    assert!(task_row_at_position(&list.source(), &state, area, area.x + 1, area.y + 1).is_some());
}

#[test]
fn hidden_status_has_no_mouse_target_but_rows_remain_selectable() {
    let item = task_list_item("No status target");
    let mut list = TaskListFixture::new(vec![item]);
    list.view_state.query = TaskQuery::All;
    list.table.columns = vec![TableColumn::Ref, TableColumn::Title, TableColumn::Time];
    let area = Rect::new(3, 2, 80, 5);
    let mut state = TableState::default();
    let model = build_task_list_render_model(
        &list.source(),
        &mut state,
        Focus::Tasks,
        area,
        None,
        &BTreeSet::new(),
    );
    assert_eq!(model.layout.cell(TableColumn::Status, area).width, 0);
    for row in area.y..area.bottom() {
        for column in area.x..area.right() {
            assert!(
                task_row_status_at_position(&list.source(), &state, area, column, row).is_none()
            );
        }
    }
    assert!(task_row_at_position(&list.source(), &state, area, area.x + 1, area.y + 1).is_some());
}

#[test]
fn hidden_columns_fit_a_narrow_table_without_phantom_gaps() {
    let item = task_list_item("A narrow title");
    let mut list = TaskListFixture::new(vec![item]);
    list.view_state.query = TaskQuery::All;
    list.table.columns = vec![TableColumn::Title, TableColumn::Status, TableColumn::Time];
    let area = Rect::new(0, 0, 24, 4);
    let mut state = TableState::default();
    let model = build_task_list_render_model(
        &list.source(),
        &mut state,
        Focus::Tasks,
        area,
        None,
        &BTreeSet::new(),
    );
    let title = model.layout.cell(TableColumn::Title, area);
    let status = model.layout.cell(TableColumn::Status, area);
    let time = model.layout.cell(TableColumn::Time, area);
    assert!(title.width > 0);
    assert!(status.width > 0);
    assert!(time.width > 0);
    assert_eq!(status.x, title.right() + 1);
    assert_eq!(time.x, status.right() + 1);
    assert!(buffer_text(&render_task_list_buffer(&list.source(), 24, 4)).contains("STATUS"));
}

#[test]
fn reordered_time_column_keeps_contextual_headings_and_values() {
    for (query, due_order, deferred, heading) in [
        (TaskQuery::Queue, false, false, "IDLE"),
        (TaskQuery::Upcoming, false, true, "WHEN"),
        (TaskQuery::All, true, false, "DUE"),
        (TaskQuery::All, false, true, "TIME"),
        (TaskQuery::All, false, false, "AGE"),
    ] {
        let mut item = task_list_item("time context");
        if deferred {
            item.task.available_at = Some("2999-01-01T00:00:00Z".to_string());
        }
        let mut list = TaskListFixture::new(vec![item]);
        list.view_state.query = query;
        if due_order {
            list.view_state.order = crate::tui::store::TaskOrder::DueOn;
        }
        list.table.columns.rotate_right(1);
        let buffer = render_task_list_buffer(&list.source(), 120, 8);
        assert!(text_in_cell(&buffer, Rect::new(0, 0, 4, 1)).contains(heading));
        let mut state = TableState::default();
        let model = build_task_list_render_model(
            &list.source(),
            &mut state,
            Focus::Tasks,
            Rect::new(0, 0, 120, 8),
            None,
            &BTreeSet::new(),
        );
        for (index, row) in model.rows.iter().enumerate() {
            if let TaskListRenderRow::Task(row) = row {
                let expected = row.cells[TableColumn::Time as usize].to_string();
                let expected = expected.chars().take(4).collect::<String>();
                assert!(
                    text_in_cell(&buffer, Rect::new(0, index as u16 + 1, 4, 1)).contains(&expected)
                );
            }
        }
    }
}

#[test]
fn explicit_default_order_preserves_rendering() {
    let mut list = epic_fixture(true);
    for width in [40, 64, 120] {
        let default = render_task_list_buffer(&list.source(), width, 5);
        list.table.columns = TableColumn::DEFAULT.to_vec();
        assert_eq!(default, render_task_list_buffer(&list.source(), width, 5));
    }
}

#[test]
fn due_column_shows_every_row_deadline_across_views_and_ordering() {
    let mut list = epic_fixture(true);
    list.tasks[0].task.due_on = Some("2999-01-01".to_string());
    list.tasks[1].task.due_on = Some("2999-02-02".to_string());
    list.table.columns = vec![TableColumn::Ref, TableColumn::Title, TableColumn::Due];

    for (query, order) in [
        (TaskQuery::Epics, TaskOrder::Updated),
        (TaskQuery::Epics, TaskOrder::DueOn),
        (TaskQuery::All, TaskOrder::Created),
        (TaskQuery::All, TaskOrder::DueOn),
    ] {
        list.view_state.query = query;
        list.view_state.order = order;
        let mut state = TableState::default();
        let area = Rect::new(0, 0, 80, 5);
        let model = build_task_list_render_model(
            &list.source(),
            &mut state,
            Focus::Tasks,
            area,
            None,
            &BTreeSet::new(),
        );
        let buffer = render_task_list_buffer(&list.source(), area.width, area.height);
        let due = model.layout.cell(TableColumn::Due, area);
        assert_eq!(
            text_in_cell(&buffer, due).trim(),
            "DUE",
            "{query:?} {order:?}"
        );
        let rows = model
            .rows
            .iter()
            .filter(|row| matches!(row, TaskListRenderRow::Task(_)))
            .count();
        assert_eq!(rows, 2, "{query:?} {order:?}");
        for (index, expected) in ["Jan1", "Feb2"].into_iter().enumerate() {
            let cell = Rect {
                y: index as u16 + 1,
                ..due
            };
            assert_eq!(
                text_in_cell(&buffer, cell).trim(),
                expected,
                "{query:?} {order:?} row {index}"
            );
        }
    }
}

#[test]
fn due_column_leaves_undated_tasks_blank() {
    let item = task_list_item("no deadline");
    let mut list = TaskListFixture::new(vec![item]);
    list.view_state.query = TaskQuery::All;
    list.table.columns = vec![TableColumn::Title, TableColumn::Due];

    let area = Rect::new(0, 0, 80, 4);
    let mut state = TableState::default();
    let model = build_task_list_render_model(
        &list.source(),
        &mut state,
        Focus::Tasks,
        area,
        None,
        &BTreeSet::new(),
    );
    let buffer = render_task_list_buffer(&list.source(), area.width, area.height);
    let due = model.layout.cell(TableColumn::Due, area);
    assert_eq!(text_in_cell(&buffer, due).trim(), "DUE");
    assert!(
        text_in_cell(&buffer, Rect { y: 1, ..due })
            .trim()
            .is_empty()
    );
}

#[test]
fn dedicated_due_column_keeps_time_contextual() {
    let mut item = task_list_item("overdue deadline");
    item.task.due_on = Some("2000-01-01".to_string());
    let mut list = TaskListFixture::new(vec![item]);
    list.view_state.query = TaskQuery::All;
    list.table.columns = vec![TableColumn::Title, TableColumn::Due, TableColumn::Time];

    let area = Rect::new(0, 0, 80, 4);
    for order in [TaskOrder::Created, TaskOrder::DueOn] {
        list.view_state.order = order;
        let mut state = TableState::default();
        let model = build_task_list_render_model(
            &list.source(),
            &mut state,
            Focus::Tasks,
            area,
            None,
            &BTreeSet::new(),
        );
        let buffer = render_task_list_buffer(&list.source(), area.width, area.height);
        let due = model.layout.cell(TableColumn::Due, area);
        let time = model.layout.cell(TableColumn::Time, area);
        assert_eq!(text_in_cell(&buffer, due).trim(), "DUE", "{order:?}");
        assert_eq!(text_in_cell(&buffer, time).trim(), "AGE", "{order:?}");
        let TaskListRenderRow::Task(row) = &model.rows[0] else {
            panic!("expected task row");
        };
        assert_eq!(row.cells[TableColumn::Due as usize].to_string(), "late!");
        assert_ne!(row.cells[TableColumn::Time as usize].to_string(), "late!");
    }
    list.table.columns = vec![TableColumn::Title, TableColumn::Time];
    list.view_state.order = TaskOrder::DueOn;
    let mut state = TableState::default();
    let model = build_task_list_render_model(
        &list.source(),
        &mut state,
        Focus::Tasks,
        area,
        None,
        &BTreeSet::new(),
    );
    let buffer = render_task_list_buffer(&list.source(), area.width, area.height);
    let time = model.layout.cell(TableColumn::Time, area);
    assert_eq!(text_in_cell(&buffer, time).trim(), "DUE");
    let TaskListRenderRow::Task(row) = &model.rows[0] else {
        panic!("expected task row");
    };
    assert_eq!(row.cells[TableColumn::Time as usize].to_string(), "late!");
}
