use super::*;

#[test]
fn overlay_render_includes_text_panel_content_and_hint() {
    let rendered = render_overlay_view(OverlayView::TextPanel(TextPanelView {
        title: "Conflict details".to_string(),
        lines: borrow_slice(vec![
            "field=title".to_string(),
            "local a: local title".to_string(),
        ]),
        scroll: 0,
    }));
    assert!(rendered.contains("Conflict details"));
    assert!(rendered.contains("field=title"));
    assert!(rendered.contains("Enter/Esc close"));
}

#[test]
fn changelog_renders_markdown_and_reader_controls() {
    let rendered = render_overlay_view(OverlayView::Changelog {
        markdown: "## v1.2.3\n\n- Added **reader** support.",
        scroll: 0,
    });

    assert!(rendered.contains("Changelog"));
    assert!(rendered.contains("v1.2.3"));
    assert!(rendered.contains("Added reader support."));
    assert!(rendered.contains("j/k line"));
    assert!(rendered.contains("d/u half"));
    assert!(rendered.contains("PgUp/PgDn page"));
}

#[test]
fn changelog_draws_shared_scrollbar_when_content_overflows() {
    let rendered = render_overlay_view(OverlayView::Changelog {
        markdown: borrow_value(format!(
            "## Unreleased\n\n{}",
            "- release note\n".repeat(40)
        ))
        .as_str(),
        scroll: 0,
    });

    assert!(rendered.contains("▲"));
    assert!(rendered.contains("▼"));
}

#[test]
fn overlay_render_includes_search_title_and_input() {
    let rendered = render_overlay_view(OverlayView::Search {
        input: "query".to_string(),
        cursor: 5,
        results: borrow_slice(vec![search_result_item("Query result")]),
        selected: 0,
        total_matches: 12,
        stale: false,
        no_matches_cached: false,
        intent: SearchKind::Navigate,
    });
    assert!(rendered.contains("Search"));
    assert!(rendered.contains("query"));
    assert!(rendered.contains("Query result"));
    assert!(rendered.contains("1 of 12"));
    assert!(rendered.contains("age="));
}

#[test]
fn search_overlay_shows_empty_result_summary() {
    let rendered = render_overlay_view(OverlayView::Search {
        input: "missing".to_string(),
        cursor: 7,
        results: &[],
        selected: 0,
        total_matches: 0,
        stale: false,
        no_matches_cached: false,
        intent: SearchKind::Navigate,
    });

    assert!(rendered.contains("0 matches"));
    assert!(!rendered.contains("No matching tasks"));
}

#[test]
fn stale_search_overlay_keeps_empty_state_blank() {
    let rendered = render_overlay_view(OverlayView::Search {
        input: "query".to_string(),
        cursor: 5,
        results: &[],
        selected: 0,
        total_matches: 0,
        stale: true,
        no_matches_cached: false,
        intent: SearchKind::Navigate,
    });

    assert!(!rendered.contains("searching..."));
    assert!(!rendered.contains("No matching tasks"));
    assert!(!rendered.contains("0 matches"));
}

#[test]
fn stale_search_overlay_preserves_cached_empty_summary() {
    let rendered = render_overlay_view(OverlayView::Search {
        input: "quer".to_string(),
        cursor: 4,
        results: &[],
        selected: 0,
        total_matches: 0,
        stale: true,
        no_matches_cached: true,
        intent: SearchKind::Navigate,
    });

    assert!(rendered.contains("0 matches"));
    assert!(!rendered.contains("searching..."));
    assert!(!rendered.contains("No matching tasks"));
}

#[test]
fn add_dependency_search_explains_blocker_selection() {
    let rendered = render_overlay_view(OverlayView::Search {
        input: String::new(),
        cursor: 0,
        results: &[],
        selected: 0,
        total_matches: 0,
        stale: false,
        no_matches_cached: false,
        intent: SearchKind::AddDependency,
    });

    assert!(rendered.contains("Add dependency"));
    assert!(rendered.contains("Search for the task that blocks this task"));
    assert!(rendered.contains("Enter add selected as blocker"));
    assert!(!rendered.contains("Tab open results"));
}

#[test]
fn add_epic_child_create_action_spans_row_and_updates_enter_hint() {
    let mut create = search_result_item("Create a child using \"Ship account security\"...");
    create.display_ref.clear();
    create.create_new = true;
    let intent = SearchKind::AddEpicChild {
        display_ref: "APP-YDKM".to_string(),
    };
    let buffer = overlay_buffer(OverlayView::Search {
        input: "Ship account security".to_string(),
        cursor: 21,
        results: borrow_slice(vec![create, search_result_item("Ship account security")]),
        selected: 0,
        total_matches: 1,
        stale: false,
        no_matches_cached: false,
        intent,
    });
    let create_row = (0..buffer.area.height)
        .map(|row| buffer_row(&buffer, row))
        .find(|row| row.contains("Create a child using"))
        .unwrap();
    let rendered = buffer
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect::<String>();

    assert!(create_row.contains("▸ Create a child using"));
    assert!(rendered.contains("  Open task authoring"));
    assert!(rendered.contains("Enter create child"));
    assert!(!rendered.contains("Enter add selected child"));
}

#[test]
fn add_epic_child_task_selection_uses_add_enter_hint() {
    let mut create = search_result_item("Create a new child task...");
    create.display_ref.clear();
    create.create_new = true;
    let rendered = render_overlay_view(OverlayView::Search {
        input: String::new(),
        cursor: 0,
        results: borrow_slice(vec![create, search_result_item("Existing task")]),
        selected: 1,
        total_matches: 1,
        stale: false,
        no_matches_cached: false,
        intent: SearchKind::AddEpicChild {
            display_ref: "APP-YDKM".to_string(),
        },
    });

    assert!(rendered.contains("Enter add selected child"));
    assert!(rendered.contains("Tab/Shift+Tab select"));
    assert!(!rendered.contains("Enter create child"));
}

#[test]
fn search_overlay_marks_epic_results_with_star() {
    let mut result = search_result_item("Query result");
    result.is_epic = true;
    let rendered = render_overlay_view(OverlayView::Search {
        input: "query".to_string(),
        cursor: 5,
        results: borrow_slice(vec![result]),
        selected: 0,
        total_matches: 1,
        stale: false,
        no_matches_cached: false,
        intent: SearchKind::Navigate,
    });

    assert!(rendered.contains(crate::tui::ui::task_list::EPIC_MARKER));
}

#[test]
fn search_overlay_colors_project_prefix() {
    let buffer = overlay_buffer(OverlayView::Search {
        input: "query".to_string(),
        cursor: 5,
        results: borrow_slice(vec![search_result_item("Query result")]),
        selected: 0,
        total_matches: 12,
        stale: false,
        no_matches_cached: false,
        intent: SearchKind::Navigate,
    });
    let prefix_cell = buffer
        .content
        .iter()
        .find(|cell| cell.symbol() == "A" && cell.fg == theme::project_color("aven"))
        .unwrap();

    assert_eq!(prefix_cell.fg, theme::project_color("aven"));
}

#[test]
fn search_overlay_vertical_position_ignores_result_count() {
    let empty = overlay_buffer(OverlayView::Search {
        input: "query".to_string(),
        cursor: 5,
        results: &[],
        selected: 0,
        total_matches: 0,
        stale: false,
        no_matches_cached: false,
        intent: SearchKind::Navigate,
    });
    let populated = overlay_buffer(OverlayView::Search {
        input: "query".to_string(),
        cursor: 5,
        results: borrow_slice(vec![
            search_result_item("First result"),
            search_result_item("Second result"),
        ]),
        selected: 0,
        total_matches: 12,
        stale: false,
        no_matches_cached: false,
        intent: SearchKind::Navigate,
    });
    let title_row = |buffer: &ratatui::buffer::Buffer| {
        (0..buffer.area.height)
            .find(|row| buffer_row(buffer, *row).contains("Search"))
            .unwrap()
    };

    assert_eq!(title_row(&empty), title_row(&populated));
}

#[test]
fn text_panel_scroll_offset_changes_visible_content() {
    let rendered = render_overlay_view(OverlayView::TextPanel(TextPanelView {
        title: "Long panel".to_string(),
        lines: borrow_slice((0..20).map(|index| format!("Line {index}")).collect()),
        scroll: 8,
    }));
    assert!(rendered.contains("Line 8"));
    assert!(!rendered.contains("Line 0"));
}
