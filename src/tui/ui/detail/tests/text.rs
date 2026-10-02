use super::*;

#[test]
fn detail_content_includes_notes() {
    let item = detail_test_item();
    let rendered = detail_content_lines(&item, 60, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Fix token refresh race"));
    assert!(rendered.contains("Confirmed race in useTokenRefresh.ts"));
    assert!(!rendered.contains("2026-06-20T12:00:00Z"));
}

#[test]
fn detail_header_wraps_the_complete_title_before_metadata() {
    let mut item = detail_test_item();
    item.task.title = "Implement wrapped task titles in the detail pane".to_string();

    let lines = detail_header_options(&item, 18, None);
    let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();

    assert_eq!(
        &rendered[..3],
        &["Implement wrapped", "task titles in the", "detail pane"]
    );
    assert_eq!(rendered[3], "─".repeat(18));
    assert!(rendered[4].contains(&item.display_ref));
    assert!(!rendered.join("\n").contains('…'));
}

#[test]
fn detail_header_leads_with_project_context() {
    let item = detail_test_item();

    let lines = detail_header_options(&item, 60, None);
    let summary = lines
        .iter()
        .find(|line| line.to_string().contains(&item.display_ref))
        .expect("detail summary");

    assert!(summary.to_string().starts_with("● app / APP-7KQ9A1X"));
    assert_eq!(
        summary.spans[0].style.fg,
        Some(theme::project_color(&item.task.project_key))
    );
    let reference_prefix = summary
        .spans
        .iter()
        .find(|span| span.content == item.task.project_prefix.as_str())
        .expect("task reference prefix");
    assert_eq!(
        reference_prefix.style.fg,
        Some(theme::project_color(&item.task.project_key))
    );
}

#[test]
fn detail_header_breaks_words_that_exceed_narrow_widths() {
    let mut item = detail_test_item();
    item.task.title = "abcdefghij".to_string();

    let lines = detail_title_lines(&item, 4, None);
    let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();

    assert_eq!(rendered, ["abcd", "efgh", "ij"]);
    assert_eq!(rendered.concat(), item.task.title);
    assert!(lines.iter().all(|line| line.width() <= 4));
}

#[test]
fn detail_header_wraps_by_unicode_display_width() {
    let mut item = detail_test_item();
    item.task.title = "ab界cd 界界界".to_string();

    let lines = detail_title_lines(&item, 4, None);
    let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();

    assert_eq!(rendered, ["ab界", "cd", "界界", "界"]);
    assert!(lines.iter().all(|line| line.width() <= 4));

    item.task.title = "👩‍💻x".to_string();
    let emoji = detail_title_lines(&item, 2, None)
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>();
    assert_eq!(emoji, ["👩‍💻", "x"]);
}

#[test]
fn detail_header_marks_epics_with_star() {
    let mut item = detail_test_item();
    item.task.is_epic = true;

    let lines = detail_header_options(&item, 60, None);
    let marker = lines[2]
        .spans
        .iter()
        .find(|span| span.content == EPIC_MARKER)
        .expect("epic marker");

    assert_eq!(marker.style.fg, Some(theme::yellow()));
}

#[test]
fn inline_title_editing_stays_on_one_clipped_line() {
    let mut item = detail_test_item();
    item.task.title = "a committed title long enough to wrap".to_string();
    let editor = TextInputView {
        kind: crate::tui::overlay::TextInputKind::EditTitle,
        title: "Edit title".to_string(),
        prompt: String::new(),
        input: "an edited title that remains horizontal".to_string(),
        cursor: 30,
    };

    let lines = detail_header_options(&item, 10, Some(&editor));

    assert_eq!(lines.len(), 4);
    assert!(lines[0].width() <= 10);
    assert_eq!(lines[1].to_string(), "─".repeat(10));
    assert_ne!(lines[0].to_string(), editor.input);
}

#[test]
fn detail_text_mapping_handles_wide_title_characters() {
    let mut item = detail_test_item();
    item.task.title = "A界B".to_string();

    let first_wide_cell = detail_text_cell_at_position(&item, 80, 24, 3, 3, 0).unwrap();
    let second_wide_cell = detail_text_cell_at_position(&item, 80, 24, 4, 3, 0).unwrap();
    let selection = DetailTextSelection::new(item.task.id.clone(), 80, first_wide_cell);

    assert_eq!(first_wide_cell, second_wide_cell);
    assert_eq!(
        detail_selected_text(&item, &selection).as_deref(),
        Some("界")
    );
    let trailing_space = detail_text_cell_at_position(&item, 80, 24, 70, 3, 0).unwrap();
    let trailing_selection = DetailTextSelection::new(item.task.id.clone(), 80, trailing_space);
    assert_eq!(
        detail_selected_text(&item, &trailing_selection).as_deref(),
        Some("B")
    );

    item.task.title = "x".repeat(200);
    assert_eq!(detail_text_cell_at_position(&item, 120, 30, 88, 3, 0), None);
}

#[test]
fn detail_text_mapping_uses_scrolled_wrapped_description_lines() {
    let mut item = detail_test_item();
    item.task.description = "first paragraph with enough words to wrap across terminal lines and keep going for another line".to_string();
    let layout = detail_content_layout(Rect::new(0, 0, 70, 12));
    let document = detail_selectable_document(&item, layout.content_area.width as usize, None);
    assert!(document.description.len() > 1);
    let expected = document.description[1]
        .text
        .chars()
        .next()
        .unwrap()
        .to_string();
    let body_y = layout.content_area.y + 4;

    let cell =
        detail_text_cell_at_position(&item, 70, 12, layout.content_area.x + 2, body_y, 1).unwrap();
    let selection = DetailTextSelection::new(item.task.id.clone(), 70, cell);

    assert_eq!(
        detail_selected_text(&item, &selection).as_deref(),
        Some(expected.as_str())
    );
}

#[test]
fn detail_selection_copies_rendered_markdown_text() {
    let mut item = detail_test_item();
    item.task.description = "**bold** and `code`".to_string();
    let layout = detail_content_layout(Rect::new(0, 0, 80, 24));
    let document = detail_selectable_document(&item, layout.content_area.width as usize, None);
    let description = document.description.first().unwrap();
    let selection = DetailTextSelection {
        task_id: item.task.id.clone(),
        terminal_width: 80,
        anchor: TextCell {
            start: description.document_start,
            end: description.document_start + 1,
        },
        focus: TextCell {
            start: description.document_start + description.text.len() - 1,
            end: description.document_start + description.text.len(),
        },
    };

    assert_eq!(description.text, "bold and code");
    assert_eq!(
        detail_selected_text(&item, &selection).as_deref(),
        Some("bold and code")
    );
}

#[test]
fn detail_selection_highlights_the_selected_range() {
    let item = detail_test_item();
    let selection =
        DetailTextSelection::new(item.task.id.clone(), 80, TextCell { start: 0, end: 3 });

    let model = build_detail_content_model(
        &item,
        Rect::new(2, 3, 76, 18),
        0,
        None,
        None,
        &BTreeSet::new(),
        Some(&selection),
        None,
    );
    let selected = model.sticky_lines[0]
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "Fix")
        .unwrap();

    assert_eq!(selected.style.bg, Some(theme::accent()));
    assert_eq!(selected.style.fg, Some(theme::inverse_fg()));
}

#[test]
fn detail_content_renders_markdown_description_and_notes() {
    let mut item = detail_test_item();
    item.task.description = "## Context\n- **One** item".to_string();
    item.notes[0].body = "Use `aven show` after edits".to_string();

    let rendered = detail_content_lines(&item, 60, None)
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Context"));
    assert!(rendered.contains("- One item"));
    assert!(rendered.contains("aven show"));
    assert!(!rendered.contains("`aven show`"));
}

#[test]
fn detail_markdown_links_hide_destinations_and_have_mouse_targets() {
    let mut item = detail_test_item();
    item.task.description =
        "See the [Aven guide](https://aven.raine.dev/guide/) for details.".to_string();
    item.notes[0].body = "Review the [task docs](https://aven.raine.dev/tasks/).".to_string();
    let expanded_sections = BTreeSet::new();
    let context = detail_query_context(100, 30, 0, &expanded_sections, None);
    let document = DetailDocument::build(&item, &context);
    let rendered = document
        .model
        .lines
        .iter()
        .map(Line::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    let link = document
        .geometry
        .body
        .hyperlinks
        .first()
        .expect("description link");
    let row = document
        .layout
        .content_area
        .y
        .saturating_add(document.sticky_height() as u16)
        .saturating_add(link.line_index.saturating_sub(document.model.body_start) as u16);
    let column = document
        .layout
        .content_area
        .x
        .saturating_add(link.start_column as u16);

    assert!(rendered.contains("See the Aven guide for details."));
    assert!(rendered.contains("Review the task docs."));
    assert!(!rendered.contains("https://"));
    assert_eq!(
        document
            .geometry
            .body
            .hyperlinks
            .iter()
            .map(|link| link.url.as_str())
            .collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "https://aven.raine.dev/guide/",
            "https://aven.raine.dev/tasks/",
        ])
    );
    assert_eq!(
        document.link_at_position(column, row).as_deref(),
        Some("https://aven.raine.dev/guide/")
    );
    assert!(
        document
            .link_at_position(column.saturating_sub(1), row)
            .is_none()
    );
}

#[test]
fn detail_description_lines_keep_quote_rail() {
    let mut item = detail_test_item();
    item.task.description = "## Context\nsecond line".to_string();
    let lines = detail_content_lines(&item, 60, None);
    let description_lines: Vec<_> = lines
        .into_iter()
        .filter(|line| {
            let text = line.to_string();
            text.contains("Context") || text.contains("second")
        })
        .collect();
    assert!(!description_lines.is_empty());
    for line in description_lines {
        assert!(
            line.spans
                .first()
                .is_some_and(|span| span.content.as_ref() == "│ "),
            "missing quote rail: {line:?}"
        );
    }
}

#[test]
fn detail_note_lines_keep_quote_rail() {
    let mut item = detail_test_item();
    item.notes[0].body = "Use `aven` here".to_string();
    let lines = detail_content_lines(&item, 60, None);
    let note_lines: Vec<_> = lines
        .into_iter()
        .filter(|line| line.to_string().contains("aven"))
        .collect();
    assert_eq!(note_lines.len(), 1);
    let line = &note_lines[0];
    assert!(
        line.spans
            .first()
            .is_some_and(|span| span.content.as_ref() == "│ "),
        "missing quote rail: {line:?}"
    );
    let code_span = line
        .spans
        .iter()
        .find(|span| span.content.as_ref() == "aven")
        .expect("missing rendered inline code span");
    assert_eq!(
        code_span.style.fg,
        Some(crate::tui::theme::blue()),
        "inline code foreground style was not preserved"
    );
    assert!(
        !line.to_string().contains('`'),
        "inline code markers leaked into rendered note text"
    );
}
