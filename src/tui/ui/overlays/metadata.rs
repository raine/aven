use crate::tui::overlay::metadata::{
    MetadataFocus, MetadataView, metadata_display, metadata_layout, visible_start,
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
        for action in &layout.actions {
            if !action.visible {
                continue;
            }
            let (key, label) = (action.key, action.label);
            let selected = editor.focus == action.focus;
            let disabled = !action.enabled;
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
            frame.render_widget(Paragraph::new(line).style(style), action.area);
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
            let start = visible_start(&visible, view.selected, layout.body.height as usize);
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
            layout.browser_hints,
        );
        frame.render_widget(
            Paragraph::new(dialog_hint_line(&[("Esc", "done")]))
                .alignment(Alignment::Right)
                .style(style),
            layout.done,
        );
    }
}
