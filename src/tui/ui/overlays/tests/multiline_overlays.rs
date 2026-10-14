use super::*;
use crate::tui::theme::Theme;
use crate::tui::ui::overlays::multiline::description_visual_row_count;

#[test]
fn edit_description_empty_input_shows_placeholder() {
    let line = description_input_line(&Theme::dark(), "", 0, true);
    assert_eq!(line.spans[0].content.as_ref(), "E");
    assert_eq!(line.spans[0].style.fg, Some(Theme::dark().bg_raised));
    assert_eq!(line.spans[0].style.bg, Some(Theme::dark().fg));
    assert_eq!(
        line.spans[1].content.as_ref(),
        "nter task description here..."
    );
    assert_eq!(line.spans[1].style.fg, Some(Theme::dark().fg_dim));
}

#[test]
fn edit_description_blank_line_does_not_show_placeholder() {
    let state = MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Edit description".to_string(),
        prompt: String::new(),
        lines: borrow_slice(vec!["body".to_string(), String::new()]),
        row: 1,
        column: 0,
        mode: MultilineInputMode::Compose,
    };
    let (lines, _) = description_editor_lines(&Theme::dark(), &state, 80);
    assert!(!lines[1].to_string().contains("Enter task description here"));
    assert_eq!(lines[1].spans[1].content.as_ref(), " ");
    assert_eq!(lines[1].spans[1].style.bg, Some(Theme::dark().fg));
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
        lines: borrow_slice(vec!["abcdefghij".to_string()]),
        row: 0,
        column: 8,
        mode: MultilineInputMode::Compose,
    };
    let (lines, cursor_row) = description_editor_lines(&Theme::dark(), &state, 4);
    assert_eq!(
        lines.iter().map(Line::to_string).collect::<Vec<_>>(),
        ["abcd", "efgh", "ij"]
    );
    assert_eq!(cursor_row, 2);
    assert_eq!(lines[cursor_row].spans[1].content, "i");
    assert_eq!(lines[cursor_row].spans[1].style.bg, Some(Theme::dark().fg));
}

#[test]
fn edit_description_wraps_end_caret_after_exact_width_line() {
    for body in ["abcd", "한글"] {
        let state = MultilineInputView {
            kind: MultilineInputKind::EditDescription,
            title: "Edit description".to_string(),
            prompt: String::new(),
            lines: borrow_slice(vec![body.to_string()]),
            row: 0,
            column: body.len(),
            mode: MultilineInputMode::Compose,
        };

        let (lines, cursor_row) = description_editor_lines(&Theme::dark(), &state, 4);

        assert_eq!(lines.len(), 2, "{body}");
        assert_eq!(cursor_row, 1, "{body}");
        assert_eq!(lines[1].width(), 1, "{body}");
        assert_eq!(description_visual_row_count(&state, 4), 2, "{body}");
    }
}

#[test]
fn overlay_render_omits_empty_multiline_prompt() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::EditDescription,
        title: "Edit description".to_string(),
        prompt: String::new(),
        lines: borrow_slice(vec!["line one".to_string()]),
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
    let line = add_note_input_line(&Theme::dark(), "", Some(0), true);
    assert_eq!(line.spans[0].content.as_ref(), "E");
    assert_eq!(line.spans[0].style.fg, Some(Theme::dark().bg_raised));
    assert_eq!(line.spans[0].style.bg, Some(Theme::dark().fg));
    assert_eq!(line.spans[1].content.as_ref(), "nter note body here...");
    assert_eq!(line.spans[1].style.fg, Some(Theme::dark().fg_dim));
}

#[test]
fn add_task_natural_overlay_uses_kind_and_add_task_free_text_style() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::AddTaskNatural,
        title: "Anything".to_string(),
        prompt: "wrong prompt".to_string(),
        lines: borrow_slice(vec![String::new()]),
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
        lines: borrow_slice(vec![String::new()]),
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
        lines: borrow_slice(vec![String::new()]),
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
        lines: borrow_slice(vec![String::new()]),
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
fn multiline_discard_confirmation_replaces_editor_with_explicit_controls() {
    #[rustfmt::skip]
    let cases = [
        (MultilineInputKind::AddNote, "Discard note draft?", "The note text will be lost."),
        (MultilineInputKind::EditDescription, "Discard description changes?", "The description changes will be lost."),
        (MultilineInputKind::ConflictManual, "Discard manual merge?", "The manual value will be lost."),
    ];
    for (kind, title, prompt) in cases {
        let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
            kind,
            title: "Draft editor".to_string(),
            prompt: String::new(),
            lines: borrow_slice(vec!["draft text".to_string()]),
            row: 0,
            column: 5,
            mode: MultilineInputMode::ConfirmDiscard,
        }));
        for expected in [
            title,
            prompt,
            "y discard",
            "n keep editing",
            "Esc keep editing",
        ] {
            assert!(rendered.contains(expected), "{kind:?}: {expected}");
        }
        for hidden in ["Draft editor", "draft text", "Ctrl-Enter / ^S submit"] {
            assert!(!rendered.contains(hidden), "{kind:?}: {hidden}");
        }
    }
}

#[test]
fn add_note_overlay_hides_placeholder_on_later_empty_lines() {
    let rendered = render_overlay_view(OverlayView::MultilineInput(MultilineInputView {
        kind: MultilineInputKind::AddNote,
        title: "Add note".to_string(),
        prompt: "note body:".to_string(),
        lines: borrow_slice(vec!["existing note text".to_string(), String::new()]),
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
        lines: borrow_slice(vec![body.clone()]),
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
                &Theme::dark(),
                &MultilineInputView {
                    kind: MultilineInputKind::EditDescription,
                    title: "Edit description".to_string(),
                    prompt: String::new(),
                    lines: borrow_slice(lines),
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
