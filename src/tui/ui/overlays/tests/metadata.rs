use super::*;
use crate::tui::overlay::metadata::{MetadataEditor, MetadataEntry};
use crate::tui::overlay::metadata::{MetadataFocus, MetadataView, metadata_layout};
use crate::tui::overlay::{LineEdit, TextBuffer};
use crate::tui::theme::{FG_MUTED, SELECTED};
use crate::tui::ui::dialog::dialog_hint_line;
use ratatui::layout::Size;
use ratatui::{Terminal, backend::TestBackend};

fn entry(key: &str, value: Option<&str>) -> MetadataEntry {
    MetadataEntry {
        field: aven_core::metadata::MetadataField {
            id: crate::ids::MetadataFieldId::new(),
            workspace_id: crate::ids::WorkspaceId::new(),
            key: key.to_string(),
            created_at: String::new(),
            updated_at: String::new(),
        },
        value: value.map(str::to_string),
    }
}

#[test]
fn fields_align_values_and_show_unset_fields() {
    let entries = [
        entry("owner", Some("Alex")),
        entry("review-state", Some("")),
        entry("url", None),
    ];
    let filter = LineEdit::blank();
    let view = MetadataView {
        entries: &entries,
        filter: &filter,
        selected: 0,
        editor: None,
        error: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| render_non_help_overlay_content(frame, &OverlayView::Metadata(view.clone())))
        .unwrap();
    let layout = metadata_layout(&view, Size::new(100, 30));
    assert_eq!(layout.area.height, 9);
    let buffer = terminal.backend().buffer();
    let x = layout.body.x + 2 + "review-state".len() as u16 + 2;
    for (row, text) in ["Alex", "", "Not set"].into_iter().enumerate() {
        let y = layout.body.y + row as u16;
        let actual: String = (x..x + text.len() as u16)
            .map(|x| buffer[(x, y)].symbol())
            .collect();
        assert_eq!(actual, text);
        assert_eq!(buffer[(x, y)].fg, if row == 2 { FG_DIM } else { FG });
    }
    assert_eq!(
        buffer[(layout.body.right() - 1, layout.body.y)].bg,
        SELECTED.bg.unwrap()
    );
}

#[test]
fn editor_footer_uses_shared_hotkey_styles_and_click_geometry() {
    let entries = [entry("owner", Some("Alex"))];
    let filter = LineEdit::blank();
    let editor = MetadataEditor {
        input: TextBuffer::from_value("Alex".to_string()),
        focus: MetadataFocus::Input,
        discard: false,
    };
    let view = MetadataView {
        entries: &entries,
        filter: &filter,
        selected: 0,
        editor: Some(&editor),
        error: None,
    };
    for width in [100, 70, 40, 30] {
        let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
        terminal
            .draw(|frame| {
                render_non_help_overlay_content(frame, &OverlayView::Metadata(view.clone()))
            })
            .unwrap();
        let layout = metadata_layout(&view, Size::new(width, 24));
        let buffer = terminal.backend().buffer();
        for (i, key, label) in [
            (0, "Enter", "save"),
            (
                1,
                if width < 70 { "^X ^E" } else { "Ctrl+X Ctrl+E" },
                "editor",
            ),
            (3, "Esc", "cancel"),
        ] {
            let area = layout.actions[i].area;
            let text: String = (area.x..area.right())
                .map(|x| buffer[(x, area.y)].symbol())
                .collect();
            assert!(text.contains(&format!("{key} {label}")), "{width}: {text}");
            let expected = dialog_hint_line(&[(key, label)]);
            assert_eq!(
                buffer[(area.x, area.y)].fg,
                expected.spans[0].style.fg.unwrap()
            );
            assert!(buffer[(area.x, area.y)].modifier.contains(Modifier::BOLD));
            assert_eq!(buffer[(area.x + key.len() as u16 + 1, area.y)].fg, FG_MUTED);
            assert_eq!(buffer[(area.x, area.y)].bg, BG_ALT);
        }
    }
}

#[test]
fn input_uses_shared_cursor_and_footer_packs_visible_actions() {
    let entries = [entry("owner", None)];
    let filter = LineEdit::blank();
    let editor = MetadataEditor {
        input: TextBuffer::from_value("é中".to_string()),
        focus: MetadataFocus::Input,
        discard: false,
    };
    let view = MetadataView {
        entries: &entries,
        filter: &filter,
        selected: 0,
        editor: Some(&editor),
        error: None,
    };
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|frame| render_non_help_overlay_content(frame, &OverlayView::Metadata(view.clone())))
        .unwrap();
    let layout = metadata_layout(&view, Size::new(100, 30));
    let buffer = terminal.backend().buffer();
    assert_eq!(layout.body.height, 1);
    assert_eq!(layout.body.y, layout.header.bottom());
    let cursor = crate::tui::ui::input::text_cursor_position(buffer).unwrap();
    assert_eq!(cursor.x, layout.body.x + 3);
    assert_eq!(cursor.y, layout.body.y);
    assert_eq!(layout.actions[1].area.x, layout.actions[0].area.right() + 2);
    assert_eq!(layout.actions[2].area.width, 0);
    assert_eq!(layout.actions[3].area.x, layout.actions[1].area.right() + 2);
}

#[test]
fn blank_editor_disables_save_without_empty_string_instructions() {
    let entries = [entry("owner", Some("Alex"))];
    let filter = LineEdit::blank();
    for value in ["", " \t", "\n"] {
        let editor = MetadataEditor {
            input: TextBuffer::from_value(value.to_string()),
            focus: MetadataFocus::Save,
            discard: false,
        };
        let view = MetadataView {
            entries: &entries,
            filter: &filter,
            selected: 0,
            editor: Some(&editor),
            error: None,
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal
            .draw(|frame| {
                render_non_help_overlay_content(frame, &OverlayView::Metadata(view.clone()))
            })
            .unwrap();
        let layout = metadata_layout(&view, Size::new(100, 30));
        let buffer = terminal.backend().buffer();
        assert_eq!(
            buffer[(layout.actions[0].area.x, layout.actions[0].area.y)].fg,
            FG
        );
        assert!(
            buffer[(layout.actions[0].area.x, layout.actions[0].area.y)]
                .modifier
                .contains(Modifier::BOLD)
        );
        assert_eq!(
            buffer[(layout.actions[0].area.x, layout.actions[0].area.y)].bg,
            BG_ALT
        );
        let text = buffer_text(terminal.backend());
        assert!(text.contains("remove field"));
        assert!(!text.contains("empty string"));
        assert!(!text.contains("^S"));
    }
}

#[test]
fn editor_sizes_to_content_and_keeps_error_and_actions_visible() {
    let entries = [entry("owner", Some(""))];
    let filter = LineEdit::blank();
    for count in [1, 6, 20] {
        let editor = MetadataEditor {
            input: TextBuffer::from_value(vec!["value"; count].join("\n")),
            focus: MetadataFocus::Save,
            discard: false,
        };
        let view = MetadataView {
            entries: &entries,
            filter: &filter,
            selected: 0,
            editor: Some(&editor),
            error: Some("Save failed"),
        };
        for (width, height) in [(100, 30), (70, 18), (40, 12), (30, 8)] {
            let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
            terminal
                .draw(|frame| {
                    render_non_help_overlay_content(frame, &OverlayView::Metadata(view.clone()))
                })
                .unwrap();
            let layout = metadata_layout(&view, Size::new(width, height));
            assert!(layout.area.height <= count.clamp(1, 3) as u16 + 11);
            assert!(layout.body.height > 0);
            assert!(layout.body.bottom() <= layout.error.y);
            let buffer = terminal.backend().buffer();
            if layout.error.height > 0 {
                assert_eq!(buffer[(layout.error.x, layout.error.y)].fg, RED);
            }
            assert_eq!(
                buffer[(layout.actions[0].area.x, layout.actions[0].area.y)].bg,
                BG_ALT
            );
            assert!(layout.actions[3].area.right() < width);
        }
    }
}

#[test]
fn metadata_uses_shared_dialog_chrome() {
    let filter = LineEdit::blank();
    assert_overlay_uses_dialog_chrome(
        OverlayView::Metadata(MetadataView {
            entries: &[],
            filter: &filter,
            selected: 0,
            editor: None,
            error: None,
        }),
        "Custom metadata",
    );
}

#[test]
fn every_visible_action_renders_its_projected_label_and_hotkey() {
    for assigned in [false, true] {
        let entries = [entry("owner", assigned.then_some("original"))];
        let filter = LineEdit::blank();
        for value in ["", "value", "line\r\nnext"] {
            for focus in [
                MetadataFocus::Input,
                MetadataFocus::Save,
                MetadataFocus::ExternalEditor,
                MetadataFocus::Remove,
                MetadataFocus::Cancel,
            ] {
                let editor = MetadataEditor {
                    input: TextBuffer::from_value(value.to_string()),
                    focus,
                    discard: false,
                };
                let view = MetadataView {
                    entries: &entries,
                    filter: &filter,
                    selected: 0,
                    editor: Some(&editor),
                    error: None,
                };
                for width in [100, 70, 40, 30] {
                    let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
                    terminal
                        .draw(|frame| {
                            render_non_help_overlay_content(
                                frame,
                                &OverlayView::Metadata(view.clone()),
                            )
                        })
                        .unwrap();
                    let layout = metadata_layout(&view, Size::new(width, 24));
                    for action in layout.actions.iter().filter(|action| action.visible) {
                        let area = action.area;
                        let row = buffer_row(terminal.backend().buffer(), area.y);
                        let actual: String = row
                            .chars()
                            .skip(area.x as usize)
                            .take(area.width as usize)
                            .collect();
                        let expected = if action.key.is_empty() {
                            action.label.to_string()
                        } else {
                            format!("{} {}", action.key, action.label)
                        };
                        assert_eq!(actual, expected, "{width} {focus:?}");
                        if !action.key.is_empty() {
                            assert!(
                                terminal.backend().buffer()[(area.x, area.y)]
                                    .modifier
                                    .contains(Modifier::BOLD)
                            );
                        }
                    }
                }
            }
        }
    }
}
