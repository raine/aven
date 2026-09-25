use super::super::layout::TableLayout;
use super::super::preview::task_preview_lines;
use super::super::sizing::*;
use super::*;
use crate::tui::theme::{FG, FG_MUTED, RED, YELLOW};
use chrono::TimeZone;

#[test]
fn hidden_ref_and_status_keep_epic_rollups_and_child_rows() {
    let mut list = epic_fixture(true);
    list.table.columns = vec![
        TableColumn::Title,
        TableColumn::Labels,
        TableColumn::Metadata,
        TableColumn::Project,
        TableColumn::Priority,
        TableColumn::Time,
    ];

    let rendered = buffer_text(&render_task_list_buffer(&list.source(), 120, 5));
    assert!(rendered.contains("SUMMARY"));
    assert!(rendered.contains("1/5"));
    assert!(rendered.contains("Verify recovery email"));
    assert!(!rendered.contains("STATUS"));
    assert!(!rendered.contains("APP-EPIC"));
    assert!(!rendered.contains("APP-CHLD"));
}

#[test]
fn reordered_epics_keep_summary_and_inline_editing_in_semantic_cells() {
    for width in [64, 120] {
        for selected in [0, 1] {
            let mut list = epic_fixture(true);
            list.table.columns = vec![
                TableColumn::Status,
                TableColumn::Time,
                TableColumn::Labels,
                TableColumn::Metadata,
                TableColumn::Project,
                TableColumn::Priority,
                TableColumn::Ref,
                TableColumn::Title,
            ];
            let editor = TextInputView {
                kind: TextInputKind::EditTitle,
                title: "Edit title".to_string(),
                prompt: String::new(),
                input: "edit".to_string(),
                cursor: 4,
            };
            let mut state = TableState::default();
            state.select(Some(selected));
            let area = Rect::new(0, 0, width, 5);
            let model = build_task_list_render_model(
                &list.source(),
                &mut state,
                Focus::Tasks,
                area,
                Some(&editor),
                &BTreeSet::new(),
            );
            let mut terminal = Terminal::new(TestBackend::new(width, 5)).unwrap();
            terminal
                .draw(|frame| {
                    render_task_list(
                        frame,
                        &list.source(),
                        &mut state,
                        Focus::Tasks,
                        area,
                        Some(&editor),
                        &BTreeSet::new(),
                    )
                })
                .unwrap();
            let buffer = terminal.backend().buffer();
            let title = model.layout.cell(
                TableColumn::Title,
                Rect::new(0, selected as u16 + 1, width, 1),
            );
            assert!(text_in_cell(buffer, title).contains("edit"));
            assert_eq!(buffer[(title.x + 4, title.y)].style().bg, Some(FG));
            let summary = model
                .layout
                .cell(TableColumn::Labels, Rect::new(0, 1, width, 1));
            assert!(text_in_cell(buffer, summary).contains("1/5"));
            let child_summary = Rect { y: 2, ..summary };
            assert!(text_in_cell(buffer, child_summary).trim().is_empty());
            for index in [0, 1] {
                let status = model
                    .layout
                    .cell(TableColumn::Status, Rect::new(0, index + 1, width, 1));
                let hit =
                    task_row_status_at_position(&list.source(), &state, area, status.x, status.y)
                        .unwrap();
                assert_eq!(hit.task_id, list.tasks[index as usize].task.id);
            }
            if width < 90 {
                assert_eq!(model.layout.cell(TableColumn::Project, area).width, 0);
            }
        }
    }
}

#[test]
fn collapsed_epic_rows_show_outcomes_signals_and_subtree_activity() {
    let list = epic_fixture(false);

    let rendered = buffer_text(&render_task_list_buffer(&list.source(), 120, 5));

    assert!(rendered.contains("SUMMARY"));
    assert!(!rendered.contains("CHILDREN"));
    assert!(!rendered.contains("SIGNALS"));
    assert!(rendered.contains("ACT"));
    assert!(rendered.contains("Ship account recovery"));
    assert!(rendered.contains("!1 ←1 · 1/5 done ×1"));
    assert!(!rendered.contains("Verify recovery email"));

    let preview = task_preview_lines(&list.tasks[0], 120, 12)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(preview.contains("children 3 open · 1 done · 1 canceled"));
    assert!(preview.contains("signals 1 overdue · 1 blocked · 1 ready"));
}

#[test]
fn expanded_epic_rows_keep_child_title_and_status_legible() {
    let list = epic_fixture(true);

    let rendered = buffer_text(&render_task_list_buffer(&list.source(), 120, 5));

    assert!(rendered.contains("Ship account recovery"));
    assert!(rendered.contains("Verify recovery email"));
    assert!(rendered.contains("active"));
    assert!(rendered.contains("!1 ←1 · 1/5 done ×1"));
}

#[test]
fn epic_summary_keeps_parent_task_metadata_in_its_own_lane() {
    let mut list = epic_fixture(false);
    list.tasks[0].unresolved_blocker_count = 2;

    let rendered = buffer_text(&render_task_list_buffer(&list.source(), 120, 5));

    assert!(rendered.contains("!1 ←1 · 1/5 done ×1"));
    assert!(rendered.contains("←2"));
}

#[test]
fn narrow_epic_rows_preserve_rollup_and_core_task_fields() {
    let list = epic_fixture(false);

    let rendered = buffer_text(&render_task_list_buffer(&list.source(), 64, 4));

    assert!(rendered.contains("SUMMARY"));
    assert!(!rendered.contains("CHILDREN"));
    assert!(!rendered.contains("SIGNALS"));
    assert!(rendered.contains("!1 ←1 · ✓1/5 ×1"));
    assert!(rendered.contains("todo"));
    assert!(rendered.contains("APP-"));
}

#[test]
fn epic_columns_keep_a_blank_gutter_at_normal_and_narrow_widths() {
    for width in [64, 120] {
        let list = epic_fixture(false);
        let buffer = render_task_list_buffer(&list.source(), width, 4);
        let columns = task_list_columns_for_tasks(
            &list.source(),
            width < 90,
            &[&list.tasks[0]],
            EpicSelectionContext::default(),
        );
        let columns = TableColumn::DEFAULT.map(|column| columns[column as usize]);
        let header_cells = Layout::horizontal(columns).areas::<8>(Rect::new(0, 0, width, 1));
        let row_cells = Layout::horizontal(columns).areas::<8>(Rect::new(0, 1, width, 1));

        for cells in [&header_cells, &row_cells] {
            for cell in cells.iter().take(7) {
                if cell.width > 0 {
                    assert_eq!(
                        buffer[(cell.x + cell.width - 1, cell.y)].symbol(),
                        " ",
                        "missing gutter at width {width} in {cell:?}"
                    );
                }
            }
        }
    }
}

#[test]
fn empty_epic_row_uses_standard_placeholders() {
    let mut list = epic_fixture(false);
    list.tasks[0].epic_children.clear();
    let rollup = crate::query::EpicRollup {
        latest_activity_at: list.tasks[0].task.updated_at.clone(),
        ..crate::query::EpicRollup::default()
    };
    list.tasks[0].epic_rollup = Some(rollup.clone());

    let preview = task_preview_lines(&list.tasks[0], 100, 12)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let cell = epic_summary_cell(&rollup, 24);
    assert_eq!(cell.to_string(), "-");
    assert_eq!(cell.spans[0].style.fg, Some(FG_MUTED));
    assert!(preview.contains("children none"));
}

#[test]
fn epic_summary_preserves_outcome_and_signal_semantics() {
    let rollup = crate::query::EpicRollup {
        total: 6,
        open: 3,
        done: 2,
        canceled: 1,
        blocked: 1,
        overdue: 1,
        ready: 1,
        latest_activity_at: String::new(),
    };

    let normal = epic_summary_cell(&rollup, 24);
    assert_eq!(normal.to_string(), "!1 ←1 · 2/6 done ×1");
    assert!(normal.width() <= 24);
    let compact = epic_summary_cell(&rollup, 17);
    assert_eq!(compact.to_string(), "!1 ←1 · ✓2/6 ×1");
    assert!(compact.width() <= 17);
    for summary in [&normal, &compact] {
        assert_eq!(
            summary
                .spans
                .iter()
                .find(|span| span.content.contains('×'))
                .unwrap()
                .style
                .fg,
            Some(RED)
        );
        assert!(
            summary
                .spans
                .iter()
                .filter(|span| { span.content.starts_with('!') || span.content.starts_with('←') })
                .all(|span| !span.style.add_modifier.contains(Modifier::BOLD))
        );
        assert!(
            !summary
                .spans
                .iter()
                .find(|span| span.content.contains('/'))
                .unwrap()
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }

    let open_only = crate::query::EpicRollup {
        total: 3,
        open: 3,
        ready: 3,
        ..crate::query::EpicRollup::default()
    };
    assert_eq!(
        epic_summary_cell(&open_only, 24).to_string(),
        "3 ready · 0/3 done"
    );

    let resolved = crate::query::EpicRollup {
        total: 2,
        done: 2,
        ..crate::query::EpicRollup::default()
    };
    let resolved = epic_summary_cell(&resolved, 24);
    assert_eq!(resolved.to_string(), "resolved · 2/2 done");
    assert!(
        !resolved.spans[0]
            .style
            .add_modifier
            .contains(Modifier::BOLD)
    );

    let canceled = crate::query::EpicRollup {
        total: 2,
        canceled: 2,
        ..crate::query::EpicRollup::default()
    };
    let canceled = epic_summary_cell(&canceled, 24);
    assert_eq!(canceled.to_string(), "resolved · 0/2 done ×2");
    assert_eq!(canceled.spans[3].style.fg, Some(RED));

    let stalled = crate::query::EpicRollup {
        total: 3,
        open: 3,
        ..crate::query::EpicRollup::default()
    };
    let stalled = epic_summary_cell(&stalled, 24);
    assert_eq!(stalled.to_string(), "0 ready · 0/3 done");
    assert_eq!(stalled.spans[0].style.fg, Some(YELLOW));
}

#[test]
fn epic_time_column_uses_subtree_activity_and_respects_due_order() {
    let (mut parent, child) = epic_parent_and_child();
    let now = chrono::Utc
        .with_ymd_and_hms(2026, 6, 22, 0, 0, 0)
        .single()
        .unwrap()
        .timestamp();
    parent.task.due_on = Some("2026-06-23".to_string());

    assert_eq!(
        epic_activity_cell(&parent, now, false, true).to_string(),
        "1d"
    );
    assert_eq!(
        epic_activity_cell(&parent, now, true, true).to_string(),
        task_time_cell(&parent, now, TaskListRenderMode::Epics, true, true).to_string()
    );
    let child_activity = task_seconds_since(&child.task.updated_at, now)
        .map(compact_age)
        .unwrap();
    assert_eq!(child_activity, "1d");
}

#[test]
fn epic_header_switches_between_activity_and_due_semantics() {
    let columns = [
        Constraint::Length(14),
        Constraint::Fill(1),
        Constraint::Length(20),
        Constraint::Length(18),
        Constraint::Length(9),
        Constraint::Length(10),
        Constraint::Length(3),
        Constraint::Length(5),
        Constraint::Length(6),
    ];
    for (due_order, expected, absent) in [(false, "ACT", "DUE"), (true, "DUE", "ACT")] {
        let backend = TestBackend::new(120, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                render_task_header(
                    frame,
                    frame.area(),
                    TableLayout::resolve(&columns, &TableColumn::DEFAULT, frame.area().width),
                    TaskListRenderMode::Epics,
                    false,
                    due_order,
                    false,
                )
            })
            .unwrap();
        let rendered = buffer_text(terminal.backend().buffer());
        assert!(rendered.contains(expected));
        assert!(!rendered.contains(absent));
    }
}

#[tokio::test]
async fn loaded_epic_expands_into_rendered_and_hit_tested_child_row() {
    let mut store = test_store_with_tasks(vec![
        task_list_item("Ship account recovery"),
        task_list_item("Verify recovery email"),
    ])
    .await;
    let task_id = |store: &TuiStore, title: &str| {
        store
            .tasks
            .iter()
            .find(|item| item.task.title == title)
            .unwrap()
            .task
            .id
            .clone()
    };
    let parent_id = task_id(&store, "Ship account recovery");
    let child_id = task_id(&store, "Verify recovery email");
    let mut conn = aven_core::test_support::acquire(&store.database())
        .await
        .unwrap();
    crate::operations::add_task_to_epic(
        &mut conn,
        &crate::workspaces::Workspace::default(),
        &child_id,
        &parent_id,
    )
    .await
    .unwrap();
    drop(conn);
    store.show_view(TaskQuery::Epics).await.unwrap();
    let parent_index = store
        .tasks
        .iter()
        .position(|item| item.task.id == parent_id)
        .unwrap();
    store
        .toggle_selected_epic(Some(parent_index))
        .await
        .unwrap()
        .unwrap();

    let area = Rect::new(0, 0, 120, 6);
    let rendered = buffer_text(&render_task_list_buffer(
        &TaskListSource::from_store(&store),
        area.width,
        area.height,
    ));
    assert!(rendered.contains("Ship account recovery"));
    assert!(rendered.contains("Verify recovery email"));
    assert!(rendered.contains("▾"));

    let table_state = TableState::default();
    for id in [&parent_id, &child_id] {
        let index = store
            .tasks
            .iter()
            .position(|item| &item.task.id == id)
            .unwrap();
        let row = area.y + 1 + task_visual_row(&store, index).unwrap() as u16;
        let hit = (area.x..area.right())
            .find_map(|x| task_status_at_position(&store, &table_state, area, x, row))
            .unwrap();
        assert_eq!(&hit.task_id, id);
    }
}
