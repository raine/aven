use super::*;

#[test]
fn overlay_render_includes_multiline_submit_hints() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Description".to_string(),
        prompt: "Body".to_string(),
        lines: vec!["line one".to_string()],
        row: 0,
        column: 4,
        mode: MultilineInputMode::Compose,
    }));
    assert!(rendered.contains("Description"));
    assert!(rendered.contains("line one"));
    assert!(rendered.contains("Ctrl-Enter / ^S submit"));
}

#[test]
fn edit_description_empty_input_shows_placeholder() {
    let line = description_input_line("", 0, true);
    assert_eq!(line.spans[0].content.as_ref(), "E");
    assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
    assert_eq!(line.spans[0].style.bg, Some(FG));
    assert_eq!(
        line.spans[1].content.as_ref(),
        "nter task description here..."
    );
    assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
}

#[test]
fn edit_description_blank_line_does_not_show_placeholder() {
    let state = MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Edit description".to_string(),
        prompt: String::new(),
        lines: vec!["body".to_string(), String::new()],
        row: 1,
        column: 0,
        mode: MultilineInputMode::Compose,
    };
    let (lines, _) = description_editor_lines(&state, 80);
    assert!(!lines[1].to_string().contains("Enter task description here"));
    assert_eq!(lines[1].spans[1].content.as_ref(), " ");
    assert_eq!(lines[1].spans[1].style.bg, Some(FG));
}

#[test]
fn edit_description_overlay_wraps_long_lines() {
    let overlay = OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Edit description".to_string(),
        prompt: String::new(),
        lines: vec!["a".repeat(160)],
        row: 0,
        column: 150,
        mode: MultilineInputMode::Compose,
    });
    let rendered = render_overlay_view(overlay);
    assert!(rendered.contains("Edit description"));
    assert!(rendered.contains("Ctrl-Enter / ^S submit"));
    assert!(rendered.contains("Ctrl+X Ctrl+E editor"));
    assert!(rendered.contains("line 1/1"));
    assert!(!rendered.contains(&"a".repeat(160)));
}

#[test]
fn edit_description_overlay_sizes_height_to_wrapped_content() {
    let short = description_overlay_metrics(100, vec!["body".to_string()], 0, 4);
    let long = description_overlay_metrics(
        100,
        (0..16).map(|index| format!("line {index}")).collect(),
        15,
        7,
    );
    let wrapped = description_overlay_metrics(100, vec!["a".repeat(400)], 0, 390);
    assert!(short.rows < long.rows, "expected content-sized height");
    assert!(short.rows < wrapped.rows, "expected wrapped line height");
    assert!(
        short.rows >= 4,
        "expected useful minimum height, got {}",
        short.rows
    );
    assert!(
        long.rows <= 24,
        "expected terminal-relative cap, got {}",
        long.rows
    );
}

#[test]
fn edit_description_overlay_width_tracks_terminal_size() {
    let normal = description_overlay_metrics(100, vec!["body".to_string()], 0, 4);
    let wide = description_overlay_metrics(160, vec!["body".to_string()], 0, 4);
    assert!(wide.columns > normal.columns);
}

#[test]
fn edit_description_cursor_row_tracks_wrapped_segment() {
    let state = MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Edit description".to_string(),
        prompt: String::new(),
        lines: vec!["abcdefghij".to_string()],
        row: 0,
        column: 8,
        mode: MultilineInputMode::Compose,
    };
    let (lines, cursor_row) = description_editor_lines(&state, 4);
    assert_eq!(lines.len(), 3);
    assert_eq!(cursor_row, 2);
}

#[test]
fn overlay_render_omits_empty_multiline_prompt() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Edit description".to_string(),
        prompt: String::new(),
        lines: vec!["line one".to_string()],
        row: 0,
        column: 4,
        mode: MultilineInputMode::Compose,
    }));
    assert!(rendered.contains("Edit description"));
    assert!(rendered.contains("line one"));
    assert!(!rendered.contains("description:"));
    assert!(rendered.contains("Ctrl-Enter / ^S submit"));
}

#[test]
fn add_note_empty_input_shows_placeholder() {
    let line = add_note_input_line("", Some(0), true);
    assert_eq!(line.spans[0].content.as_ref(), "E");
    assert_eq!(line.spans[0].style.fg, Some(BG_ALT));
    assert_eq!(line.spans[0].style.bg, Some(FG));
    assert_eq!(line.spans[1].content.as_ref(), "nter note body here...");
    assert_eq!(line.spans[1].style.fg, Some(FG_DIM));
}

#[test]
fn add_task_natural_overlay_uses_kind_and_add_task_free_text_style() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::AddTaskNatural,
        title: "Anything".to_string(),
        prompt: "wrong prompt".to_string(),
        lines: vec![String::new()],
        row: 0,
        column: 0,
        mode: MultilineInputMode::Compose,
    }));
    assert!(rendered.contains("Anything"));
    assert!(rendered.contains("Describe the task in natural language..."));
    assert!(rendered.contains("Ctrl-Enter / ^S parse"));
    assert!(rendered.contains("Enter newline"));
    assert!(!rendered.contains("wrong prompt"));
}

#[test]
fn edit_description_kind_does_not_use_natural_style_by_title() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Add task: natural language".to_string(),
        prompt: "body:".to_string(),
        lines: vec![String::new()],
        row: 0,
        column: 0,
        mode: MultilineInputMode::Compose,
    }));
    assert!(rendered.contains("Add task: natural language"));
    assert!(rendered.contains("Enter task description here"));
    assert!(rendered.contains("Ctrl-Enter / ^S submit"));
    assert!(!rendered.contains("Ctrl-Enter / ^S parse"));
    assert!(!rendered.contains("Describe the task in natural language..."));
}

#[test]
fn conflict_manual_multiline_uses_placeholder_style() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::ConflictManual,
        title: "Resolve manually".to_string(),
        prompt: "manual value for field=description:".to_string(),
        lines: vec![String::new()],
        row: 0,
        column: 0,
        mode: MultilineInputMode::Compose,
    }));
    assert!(rendered.contains("Resolve manually"));
    assert!(rendered.contains(CONFLICT_MANUAL_BODY_PLACEHOLDER));
    assert!(!rendered.contains("manual value for field=description:"));
    assert!(rendered.contains("Ctrl-Enter / ^S submit"));
}

#[test]
fn add_note_overlay_uses_placeholder_key_styles_and_spacing() {
    let overlay = OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::AddNote,
        title: "Add note".to_string(),
        prompt: "note body:".to_string(),
        lines: vec![String::new()],
        row: 0,
        column: 0,
        mode: MultilineInputMode::Compose,
    });
    let rendered = render_overlay_view(overlay.clone());
    assert!(rendered.contains("Add note"));
    assert!(rendered.contains("Enter note body here..."));
    assert!(rendered.contains("Ctrl-Enter / ^S submit"));

    let buffer = overlay_buffer(overlay);
    let hint_row = (0..buffer.area.height)
        .find(|row| buffer_row(&buffer, *row).contains("Ctrl-Enter / ^S submit"))
        .unwrap();
    let blank_row = buffer_row(&buffer, hint_row.saturating_sub(1));
    assert!(
        blank_row
            .trim_matches(|ch| ch == ' ' || ch == '│')
            .is_empty(),
        "expected blank row above key hints: {blank_row:?}"
    );
}

#[test]
fn add_note_discard_confirmation_replaces_editor_with_explicit_controls() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::AddNote,
        title: "Add note".to_string(),
        prompt: "note body:".to_string(),
        lines: vec!["draft note text".to_string()],
        row: 0,
        column: 5,
        mode: MultilineInputMode::ConfirmDiscard,
    }));

    assert!(rendered.contains("Discard note draft?"));
    assert!(rendered.contains("The note text will be lost."));
    assert!(rendered.contains("y discard"));
    assert!(rendered.contains("n keep editing"));
    assert!(rendered.contains("Esc keep editing"));
    assert!(!rendered.contains("Add note"));
    assert!(!rendered.contains("draft note text"));
    assert!(!rendered.contains("Ctrl-Enter / ^S submit"));
}

#[test]
fn description_discard_confirmation_replaces_editor_with_explicit_controls() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Edit description".to_string(),
        prompt: String::new(),
        lines: vec!["changed description".to_string()],
        row: 0,
        column: 10,
        mode: MultilineInputMode::ConfirmDiscard,
    }));

    assert!(rendered.contains("Discard description changes?"));
    assert!(rendered.contains("The description changes will be lost."));
    assert!(rendered.contains("y discard"));
    assert!(rendered.contains("n keep editing"));
    assert!(!rendered.contains("Edit description"));
    assert!(!rendered.contains("changed description"));
}

#[test]
fn conflict_manual_discard_confirmation_replaces_editor_with_explicit_controls() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::ConflictManual,
        title: "Resolve conflict: manual".to_string(),
        prompt: "manual value for field=description:".to_string(),
        lines: vec!["changed value".to_string()],
        row: 0,
        column: 12,
        mode: MultilineInputMode::ConfirmDiscard,
    }));

    assert!(rendered.contains("Discard manual merge?"));
    assert!(rendered.contains("The manual value will be lost."));
    assert!(rendered.contains("y discard"));
    assert!(rendered.contains("n keep editing"));
    assert!(!rendered.contains("Resolve conflict: manual"));
    assert!(!rendered.contains("changed value"));
}

#[test]
fn add_note_overlay_hides_placeholder_on_later_empty_lines() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::AddNote,
        title: "Add note".to_string(),
        prompt: "note body:".to_string(),
        lines: vec!["existing note text".to_string(), String::new()],
        row: 1,
        column: 0,
        mode: MultilineInputMode::Compose,
    }));

    assert!(rendered.contains("existing note text"));
    assert!(!rendered.contains("Enter note body here..."));
    assert!(rendered.contains("Ctrl-Enter / ^S submit"));
}

#[test]
fn add_note_overlay_keeps_hints_visible_when_input_wraps() {
    let body = "wrapped note text ".repeat(12);
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::AddNote,
        title: "Add note".to_string(),
        prompt: "note body:".to_string(),
        lines: vec![body.clone()],
        row: 0,
        column: body.len(),
        mode: MultilineInputMode::Compose,
    }));

    assert!(rendered.contains("wrapped note text"));
    assert!(rendered.contains("Ctrl-Enter / ^S submit"));
    assert!(rendered.contains("Esc cancel"));
}

struct DescriptionOverlayMetrics {
    rows: usize,
    columns: usize,
}

fn description_overlay_metrics(
    terminal_width: u16,
    lines: Vec<String>,
    row: usize,
    column: usize,
) -> DescriptionOverlayMetrics {
    let backend = TestBackend::new(terminal_width, 30);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|frame| {
            render_multiline_input(
                frame,
                &MultilineInputView {
                    kind: MultilineInputKind::EditDescription,
                    title: "Edit description".to_string(),
                    prompt: String::new(),
                    lines,
                    row,
                    column,
                    mode: MultilineInputMode::Compose,
                },
            )
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    let rows = (0..buffer.area.height)
        .filter(|row| buffer_row(buffer, *row).contains("│"))
        .count();
    let top_row = (0..buffer.area.height)
        .map(|row| buffer_row(buffer, row))
        .find(|row| row.contains('╭'))
        .unwrap();
    let columns = top_row.chars().filter(|ch| *ch == '─').count();
    DescriptionOverlayMetrics { rows, columns }
}
