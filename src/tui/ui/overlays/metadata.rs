use crate::tui::overlay::metadata::{
    MetadataFocus, MetadataView, metadata_display, metadata_footer_hints, metadata_layout,
    visible_start,
};
use crate::tui::text::truncate_width;
use crate::tui::theme::{ACCENT, BG_ALT, FG, FG_DIM, FG_MUTED, RED, SELECTED};
use crate::tui::ui::dialog::{Dialog, dialog_hint_line};
use ratatui::{
    Frame,
    layout::{Alignment, Rect, Size},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Wrap},
};
use unicode_width::UnicodeWidthStr;

pub(in crate::tui::ui) fn render(frame: &mut Frame, view: &MetadataView<'_>) {
    if view.editor.is_some_and(|editor| editor.discard) {
        super::confirm::render_confirm_with_hints(
            frame,
            &crate::tui::overlay::ConfirmView {
                title: "Discard metadata changes?".to_string(),
                prompt: "The metadata changes will be lost.".to_string(),
            },
            &[
                ("y", "discard"),
                ("n", "keep editing"),
                ("Esc", "keep editing"),
            ],
        );
        return;
    }
    let layout = metadata_layout(view, Size::new(frame.area().width, frame.area().height));
    let title = if view.editor.is_some() {
        "Edit metadata"
    } else {
        "Custom metadata"
    };
    let assigned = view
        .entries
        .iter()
        .filter(|entry| entry.value.is_some())
        .count();
    let dialog = Dialog::new(title, 72, layout.area.height);
    let dialog = if view.editor.is_none() {
        dialog.right_title(Line::styled(
            format!("{assigned} assigned"),
            Style::new().fg(FG_DIM),
        ))
    } else {
        dialog
    };
    dialog.render_block_at(frame, layout.area);
    let style = Style::new().fg(FG).bg(BG_ALT);
    if let Some(editor) = view.editor {
        let entry = &view.entries[view.selected];
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                truncate_width(&entry.field.key, layout.header.width as usize),
                Style::new().fg(FG_DIM),
            )]))
            .style(style),
            layout.header,
        );
        let input = &editor.input;
        let (start, left, _) = if editor.is_multiline() {
            (0, 0, 0)
        } else {
            editor.viewport(layout.body)
        };
        let lines = if !editor.is_multiline() && editor.focus == MetadataFocus::Input {
            let value = input.lines.first().map(String::as_str).unwrap_or_default();
            let cursor = crate::tui::text::char_boundary_at_or_before(value, input.column);
            vec![crate::tui::ui::input::input_line(
                "",
                &metadata_display(value),
                metadata_display(&value[..cursor]).len(),
            )]
        } else {
            input
                .lines
                .iter()
                .map(|line| Line::raw(metadata_display(line)))
                .collect()
        };
        frame.render_widget(
            Paragraph::new(lines)
                .style(style)
                .scroll((start as u16, left as u16)),
            layout.body,
        );
        let hint = view.error.unwrap_or(if editor.is_multiline() {
            "Read-only preview"
        } else {
            ""
        });
        frame.render_widget(
            Paragraph::new(hint)
                .style(Style::new().fg(if view.error.is_some() { RED } else { FG_DIM }))
                .wrap(Wrap { trim: false }),
            layout.error,
        );
        let hints = metadata_footer_hints(view, layout.header.width);
        for (i, (key, label)) in hints.into_iter().enumerate() {
            if label.is_empty() {
                continue;
            }
            let focus = [
                MetadataFocus::Save,
                MetadataFocus::ExternalEditor,
                MetadataFocus::Remove,
                MetadataFocus::Cancel,
            ][i];
            let selected = editor.focus == focus;
            let disabled = i == 0 && !editor.can_save();
            let mut line = if key.is_empty() {
                Line::styled(label, Style::new().fg(FG_MUTED))
            } else {
                dialog_hint_line(&[(key, label)])
            };
            for span in &mut line.spans {
                if disabled {
                    if !span.style.add_modifier.contains(Modifier::BOLD) {
                        span.style = span.style.fg(FG_DIM);
                    }
                } else if selected {
                    span.style = span.style.add_modifier(Modifier::UNDERLINED);
                }
            }
            frame.render_widget(Paragraph::new(line).style(style), layout.actions[i]);
        }
    } else {
        let filter = if view.filter.text.is_empty() {
            Line::styled("Type to filter fields…", Style::new().fg(FG_DIM))
        } else {
            Line::from(vec![
                Span::styled("/ ", Style::new().fg(ACCENT)),
                Span::raw(truncate_width(
                    &view.filter.text,
                    layout.header.width.saturating_sub(2) as usize,
                )),
            ])
        };
        frame.render_widget(Paragraph::new(filter).style(style), layout.header);
        let visible = view.visible();
        if visible.is_empty() {
            let message = if view.entries.is_empty() {
                "No metadata fields yet.\n\nDefine one through the CLI with --metadata key=value."
            } else {
                "No matching fields.\nClear the filter to see all fields."
            };
            frame.render_widget(
                Paragraph::new(message)
                    .style(Style::new().fg(FG_MUTED))
                    .wrap(Wrap { trim: false }),
                layout.body,
            );
        } else {
            let key_width = view
                .entries
                .iter()
                .map(|entry| entry.field.key.width())
                .max()
                .unwrap_or(0)
                .min(28)
                .min(layout.body.width.saturating_sub(8) as usize / 2);
            let value_width = (layout.body.width as usize).saturating_sub(key_width + 4);
            let start = visible_start(view, layout.body.height as usize);
            for (row, i) in visible
                .into_iter()
                .skip(start)
                .take(layout.body.height as usize)
                .enumerate()
            {
                let entry = &view.entries[i];
                let selected = i == view.selected;
                let value = match entry.value.as_deref() {
                    None => "Not set".to_string(),
                    Some(value) => value
                        .chars()
                        .map(|c| {
                            if c == '\n' {
                                '↵'
                            } else if c.is_control() {
                                ' '
                            } else {
                                c
                            }
                        })
                        .collect(),
                };
                let row_style = if selected { SELECTED } else { style };
                let value_color = if entry.value.is_none() { FG_DIM } else { FG };
                let key = truncate_width(&entry.field.key, key_width);
                let line = Line::from(vec![
                    Span::styled(if selected { "▸ " } else { "  " }, Style::new().fg(ACCENT)),
                    Span::styled(
                        format!("{key:key_width$}  "),
                        Style::new().fg(if selected { FG } else { FG_MUTED }),
                    ),
                    Span::styled(
                        truncate_width(&value, value_width),
                        Style::new().fg(value_color),
                    ),
                ]);
                frame.render_widget(
                    Paragraph::new(line).style(row_style),
                    Rect::new(
                        layout.body.x,
                        layout.body.y + row as u16,
                        layout.body.width,
                        1,
                    ),
                );
            }
        }
        frame.render_widget(
            Paragraph::new(dialog_hint_line(&[("↑↓", "select"), ("Enter", "edit")])).style(style),
            layout.actions[0].union(layout.actions[2]),
        );
        frame.render_widget(
            Paragraph::new(dialog_hint_line(&[("Esc", "done")]))
                .alignment(Alignment::Right)
                .style(style),
            layout.actions[3],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::metadata::{MetadataEditor, MetadataEntry};
    use crate::tui::overlay::{LineEdit, MultilineInputState, MultilineIntent};
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
        terminal.draw(|frame| render(frame, &view)).unwrap();
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
            input: MultilineInputState::from_value(
                MultilineIntent::CustomMetadata,
                "",
                "",
                "Alex".to_string(),
            ),
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
        for width in [100, 70, 40] {
            let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
            terminal.draw(|frame| render(frame, &view)).unwrap();
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
                let area = layout.actions[i];
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
            input: MultilineInputState::from_value(
                MultilineIntent::CustomMetadata,
                "",
                "",
                "é中".to_string(),
            ),
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
        terminal.draw(|frame| render(frame, &view)).unwrap();
        let layout = metadata_layout(&view, Size::new(100, 30));
        let buffer = terminal.backend().buffer();
        assert_eq!(layout.body.height, 1);
        assert_eq!(layout.body.y, layout.header.bottom());
        let cursor = crate::tui::ui::input::text_cursor_position(buffer).unwrap();
        assert_eq!(cursor.x, layout.body.x + 3);
        assert_eq!(cursor.y, layout.body.y);
        assert_eq!(layout.actions[1].x, layout.actions[0].right() + 2);
        assert_eq!(layout.actions[2].width, 0);
        assert_eq!(layout.actions[3].x, layout.actions[1].right() + 2);
    }

    #[test]
    fn blank_editor_disables_save_without_empty_string_instructions() {
        let entries = [entry("owner", Some("Alex"))];
        let filter = LineEdit::blank();
        for value in ["", " \t", "\n"] {
            let editor = MetadataEditor {
                input: MultilineInputState::from_value(
                    MultilineIntent::CustomMetadata,
                    "",
                    "",
                    value.to_string(),
                ),
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
            terminal.draw(|frame| render(frame, &view)).unwrap();
            let layout = metadata_layout(&view, Size::new(100, 30));
            let buffer = terminal.backend().buffer();
            assert_eq!(buffer[(layout.actions[0].x, layout.actions[0].y)].fg, FG);
            assert!(
                buffer[(layout.actions[0].x, layout.actions[0].y)]
                    .modifier
                    .contains(Modifier::BOLD)
            );
            assert_eq!(
                buffer[(layout.actions[0].x, layout.actions[0].y)].bg,
                BG_ALT
            );
            let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
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
                input: MultilineInputState::from_value(
                    MultilineIntent::CustomMetadata,
                    "",
                    "",
                    vec!["value"; count].join("\n"),
                ),
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
                terminal.draw(|frame| render(frame, &view)).unwrap();
                let layout = metadata_layout(&view, Size::new(width, height));
                assert!(layout.area.height <= count.clamp(1, 3) as u16 + 11);
                assert!(layout.body.height > 0);
                assert!(layout.body.bottom() <= layout.error.y);
                let buffer = terminal.backend().buffer();
                if layout.error.height > 0 {
                    assert_eq!(buffer[(layout.error.x, layout.error.y)].fg, RED);
                }
                assert_eq!(
                    buffer[(layout.actions[0].x, layout.actions[0].y)].bg,
                    BG_ALT
                );
                assert!(layout.actions[3].right() < width);
            }
        }
    }
}
