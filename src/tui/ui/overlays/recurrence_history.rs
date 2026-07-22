use aven_core::query::{RecurrenceHistoryEntry, RecurrenceHistoryKind};
use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};

use super::super::dialog::{Dialog, dialog_hint_line};
use crate::tui::overlay::{
    RecurrenceHistoryEntryKey, RecurrenceHistoryMode, RecurrenceHistoryView,
    recurrence_history_entry_key,
};
use crate::tui::theme::{ACCENT, FG, FG_MUTED, SELECTED_BG};

const HISTORY_WIDTH: u16 = 112;

fn history_inner_area(area: Rect) -> Rect {
    Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(1),
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecurrenceHistoryLayout {
    pub(crate) area: Rect,
    pub(crate) rows: Rect,
    pub(crate) entry_height: u16,
    pub(crate) first_visible: usize,
    pub(crate) visible_entries: usize,
}

pub(crate) fn recurrence_history_layout(
    state: &RecurrenceHistoryView,
    terminal: Size,
) -> RecurrenceHistoryLayout {
    let mode_rows = mode_lines(&state.mode).len() as u16;
    let item_count = state.page.items.len();
    let bounds = Rect::new(0, 0, terminal.width, terminal.height);
    let max_area = crate::tui::overlay::dialog_area(bounds, HISTORY_WIDTH, terminal.height);
    let max_inner = history_inner_area(max_area);
    let entry_height = if max_inner.width < 76 { 3 } else { 2 };
    let available_rows = max_inner.height.saturating_sub(mode_rows);
    let visible_entries = item_count.min((available_rows / entry_height) as usize);
    let rows_height = (visible_entries as u16).saturating_mul(entry_height);
    let content_height = if visible_entries == 0 { 1 } else { rows_height };
    let height = content_height
        .saturating_add(mode_rows)
        .saturating_add(2)
        .max(6);
    let area = crate::tui::overlay::dialog_area(bounds, HISTORY_WIDTH, height);
    let inner = history_inner_area(area);
    let selected_index = state.selected.as_ref().and_then(|selected| {
        state
            .page
            .items
            .iter()
            .position(|entry| recurrence_history_entry_key(entry) == *selected)
    });
    let first_visible = if visible_entries == 0 {
        0
    } else {
        selected_index
            .map(|selected| selected.saturating_add(1).saturating_sub(visible_entries))
            .unwrap_or(0)
    };
    let rows = Rect::new(inner.x, inner.y, inner.width, rows_height);
    RecurrenceHistoryLayout {
        area,
        rows,
        entry_height,
        first_visible,
        visible_entries,
    }
}

pub(crate) fn recurrence_history_entry_at(
    state: &RecurrenceHistoryView,
    terminal: Size,
    column: u16,
    row: u16,
) -> Option<usize> {
    let layout = recurrence_history_layout(state, terminal);
    if column < layout.rows.x
        || column >= layout.rows.right()
        || row < layout.rows.y
        || row >= layout.rows.bottom()
    {
        return None;
    }
    let visual_index = row
        .saturating_sub(layout.rows.y)
        .checked_div(layout.entry_height)? as usize;
    if visual_index >= layout.visible_entries {
        return None;
    }
    let index = layout.first_visible + visual_index;
    (index < state.page.items.len()).then_some(index)
}

pub(in crate::tui::ui) fn render_recurrence_history(
    frame: &mut Frame,
    state: &RecurrenceHistoryView,
) {
    let range = if state.page.items.is_empty() {
        "0 of 0".to_string()
    } else {
        format!(
            "{}-{} of {}",
            state.page.offset + 1,
            state.page.offset + state.page.items.len(),
            state.page.total
        )
    };
    let title = format!("Recurrence history: {} ({range})", state.page.series_ref);
    let terminal = Size::new(frame.area().width, frame.area().height);
    let layout = recurrence_history_layout(state, terminal);
    let mut lines = state
        .page
        .items
        .iter()
        .skip(layout.first_visible)
        .take(layout.visible_entries)
        .flat_map(|entry| history_lines(entry, state.selected.as_ref(), layout.entry_height))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(Line::styled(
            "No recurrence history",
            Style::new().fg(FG_MUTED),
        ));
    }
    lines.extend(mode_lines(&state.mode));
    let height = (lines.len() as u16).saturating_add(2).max(6);
    Dialog::new(&title, HISTORY_WIDTH, height).render_text(frame, Text::from(lines));
}

fn history_lines<'a>(
    entry: &'a RecurrenceHistoryEntry,
    selected: Option<&RecurrenceHistoryEntryKey>,
    entry_height: u16,
) -> Vec<Line<'a>> {
    let is_selected = selected.is_some_and(|key| *key == recurrence_history_entry_key(entry));
    let identity = match entry.kind {
        RecurrenceHistoryKind::Paused => format!(
            "{} to {}",
            entry.interval_started_at.as_deref().unwrap_or("unknown"),
            entry.interval_ended_at.as_deref().unwrap_or("present")
        ),
        _ => entry
            .slot_on
            .clone()
            .unwrap_or_else(|| "unknown".to_string()),
    };
    let kind = match entry.kind {
        RecurrenceHistoryKind::Completed => "completed",
        RecurrenceHistoryKind::Skipped => "skipped",
        RecurrenceHistoryKind::Missed => "missed",
        RecurrenceHistoryKind::Paused => "paused",
    };
    let resolved = entry.resolved_at.as_deref().unwrap_or("-");
    let task = entry.task_ref.as_deref().unwrap_or("-");
    let markers = [
        entry.corrected.then_some("corrected"),
        entry.archived_projection.then_some("archived"),
        matches!(entry.kind, RecurrenceHistoryKind::Missed).then_some("missed"),
        matches!(entry.kind, RecurrenceHistoryKind::Paused).then_some("pause"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join(",");
    let style = if is_selected {
        Style::new()
            .fg(FG)
            .bg(SELECTED_BG)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG)
    };
    let prefix = if is_selected { "> " } else { "  " };
    match entry_height {
        1 => vec![Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(
                format!(
                    "{identity:<25} {kind:<9} resolved {resolved:<25} task {task:<12} {markers}"
                ),
                style,
            ),
        ])],
        2 => vec![
            Line::styled(format!("{prefix}{identity} {kind}"), style),
            Line::styled(
                format!("  resolved {resolved}  task {task}  {markers}"),
                style,
            ),
        ],
        _ => vec![
            Line::styled(format!("{prefix}{identity} {kind}"), style),
            Line::styled(format!("  resolved {resolved}"), style),
            Line::styled(format!("  task {task}  {markers}"), style),
        ],
    }
}

#[cfg(test)]
fn history_line<'a>(
    entry: &'a RecurrenceHistoryEntry,
    selected: Option<&RecurrenceHistoryEntryKey>,
) -> Line<'a> {
    history_lines(entry, selected, 1)
        .into_iter()
        .next()
        .expect("history entry renders at least one line")
}

fn mode_lines(mode: &RecurrenceHistoryMode) -> Vec<Line<'_>> {
    let mut lines = vec![Line::default()];
    match mode {
        RecurrenceHistoryMode::Browse => lines.push(dialog_hint_line(&[
            ("j/k", "select"),
            ("PgUp/PgDn", "page"),
            ("Enter", "open"),
            ("c", "correct"),
            ("Esc", "close"),
        ])),
        RecurrenceHistoryMode::Outcome { slot_on, picker } => {
            lines.push(Line::styled(
                format!("Correct {slot_on}"),
                Style::new().fg(ACCENT),
            ));
            lines.extend(picker.items.iter().enumerate().map(|(index, item)| {
                let prefix = if index == picker.selected { "> " } else { "  " };
                Line::from(format!("{prefix}{}", item.label))
            }));
            lines.push(dialog_hint_line(&[("Enter", "choose"), ("Esc", "back")]));
        }
        RecurrenceHistoryMode::ResolutionTime {
            slot_on,
            outcome,
            input,
        } => {
            lines.push(Line::styled(
                format!("{} {slot_on}", outcome.as_str()),
                Style::new().fg(ACCENT),
            ));
            lines.push(Line::from(format!(
                "Resolution time (blank uses now): {}",
                input.text
            )));
            lines.push(dialog_hint_line(&[("Enter", "record"), ("Esc", "back")]));
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::overlay::RecurrenceHistoryMode;
    use aven_core::query::RecurrenceHistoryPage;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    #[test]
    fn recurrence_history_line_renders_timestamp_ref_and_markers() {
        let entry = RecurrenceHistoryEntry {
            kind: RecurrenceHistoryKind::Completed,
            slot_on: Some("2026-07-20".to_string()),
            interval_started_at: None,
            interval_ended_at: None,
            task_id: None,
            task_ref: Some("AVN-1234".to_string()),
            openable: false,
            corrected: true,
            archived_projection: true,
            resolved_at: Some("2026-07-20T12:34:56Z".to_string()),
        };

        let rendered = history_line(
            &entry,
            Some(&RecurrenceHistoryEntryKey::Slot("2026-07-20".to_string())),
        )
        .to_string();

        assert!(rendered.contains("2026-07-20"));
        assert!(rendered.contains("completed"));
        assert!(rendered.contains("2026-07-20T12:34:56Z"));
        assert!(rendered.contains("AVN-1234"));
        assert!(rendered.contains("corrected,archived"));
    }

    #[test]
    fn recurrence_history_pause_and_miss_markers_are_explicit() {
        let missed = RecurrenceHistoryEntry {
            kind: RecurrenceHistoryKind::Missed,
            slot_on: Some("2026-07-19".to_string()),
            interval_started_at: None,
            interval_ended_at: None,
            task_id: None,
            task_ref: None,
            openable: false,
            corrected: false,
            archived_projection: false,
            resolved_at: None,
        };
        let pause = RecurrenceHistoryEntry {
            kind: RecurrenceHistoryKind::Paused,
            slot_on: None,
            interval_started_at: Some("2026-07-18T10:00:00Z".to_string()),
            interval_ended_at: None,
            task_id: None,
            task_ref: None,
            openable: false,
            corrected: false,
            archived_projection: false,
            resolved_at: None,
        };

        assert!(history_line(&missed, None).to_string().contains("missed"));
        let pause = history_line(&pause, None).to_string();
        assert!(pause.contains("paused"));
        assert!(pause.contains("present"));
        assert!(pause.contains("pause"));
    }

    #[test]
    fn recurrence_history_narrow_rows_keep_metadata_and_hit_testing_aligned() {
        let entry = RecurrenceHistoryEntry {
            kind: RecurrenceHistoryKind::Completed,
            slot_on: Some("2026-07-20".to_string()),
            interval_started_at: None,
            interval_ended_at: None,
            task_id: None,
            task_ref: Some("AVN-1234".to_string()),
            openable: false,
            corrected: true,
            archived_projection: true,
            resolved_at: Some("2026-07-20T12:34:56Z".to_string()),
        };
        let view = RecurrenceHistoryView {
            page: RecurrenceHistoryPage {
                series_ref: "RCR-TEST".to_string(),
                items: vec![entry],
                offset: 0,
                limit: 10,
                total: 1,
                has_more: false,
            },
            selected: Some(RecurrenceHistoryEntryKey::Slot("2026-07-20".to_string())),
            mode: RecurrenceHistoryMode::Browse,
        };

        for width in [60, 80, 112] {
            let mut terminal = Terminal::new(TestBackend::new(width, 24)).unwrap();
            terminal
                .draw(|frame| render_recurrence_history(frame, &view))
                .unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            assert!(rendered.contains("2026-07-20T12:34:56Z"));
            assert!(rendered.contains("AVN-1234"));
            assert!(rendered.contains("corrected"));
            assert!(rendered.contains("archived"));

            let terminal_size = Size::new(width, 24);
            let layout = recurrence_history_layout(&view, terminal_size);
            for row in layout.rows.y..layout.rows.y + layout.entry_height {
                assert_eq!(
                    recurrence_history_entry_at(&view, terminal_size, layout.rows.x, row),
                    Some(0)
                );
            }
        }
    }

    #[test]
    fn recurrence_history_full_page_reserves_mode_rows_and_footer_hints() {
        use crate::tui::overlay::{
            LineEdit, OverlayRoute, PickerItem, PickerState, RecurrenceHistoryMode,
        };
        use aven_core::recurrence::RecurrenceOutcome;

        let items = (1..=10)
            .map(|day| RecurrenceHistoryEntry {
                kind: RecurrenceHistoryKind::Missed,
                slot_on: Some(format!("2026-07-{day:02}")),
                interval_started_at: None,
                interval_ended_at: None,
                task_id: None,
                task_ref: None,
                openable: false,
                corrected: false,
                archived_projection: false,
                resolved_at: None,
            })
            .collect::<Vec<_>>();
        let page = RecurrenceHistoryPage {
            series_ref: "RCR-TEST".to_string(),
            items,
            offset: 0,
            limit: 10,
            total: 10,
            has_more: false,
        };
        let slot_on = chrono::NaiveDate::from_ymd_opt(2026, 7, 20).unwrap();
        let cases = [
            (
                RecurrenceHistoryMode::Browse,
                9,
                vec!["PgUp/PgDn", "Esc close"],
            ),
            (
                RecurrenceHistoryMode::Outcome {
                    slot_on,
                    picker: PickerState::new(
                        OverlayRoute::MessageOnly,
                        format!("Correct {slot_on}"),
                        vec![
                            PickerItem {
                                label: "Completed".to_string(),
                                value: "completed".to_string(),
                                selected: true,
                            },
                            PickerItem {
                                label: "Skipped".to_string(),
                                value: "skipped".to_string(),
                                selected: false,
                            },
                        ],
                        false,
                    ),
                },
                7,
                vec!["Correct 2026-07-20", "Completed", "Skipped", "Enter choose"],
            ),
            (
                RecurrenceHistoryMode::ResolutionTime {
                    slot_on,
                    outcome: RecurrenceOutcome::Completed,
                    input: LineEdit::new("2026-07-20T12:34:56Z".to_string()),
                },
                8,
                vec!["Resolution time", "2026-07-20T12:34:56Z", "Enter record"],
            ),
        ];
        let terminal_size = Size::new(120, 24);

        for (mode, expected_capacity, expected_text) in cases {
            let view = RecurrenceHistoryView {
                page: page.clone(),
                selected: Some(RecurrenceHistoryEntryKey::Slot("2026-07-01".to_string())),
                mode,
            };
            let layout = recurrence_history_layout(&view, terminal_size);
            let inner = history_inner_area(layout.area);

            assert_eq!(layout.visible_entries, expected_capacity);
            assert!(layout.rows.height + mode_lines(&view.mode).len() as u16 <= inner.height);
            for row in layout.rows.bottom()..inner.bottom() {
                assert_eq!(
                    recurrence_history_entry_at(&view, terminal_size, layout.rows.x, row,),
                    None
                );
            }

            let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
            terminal
                .draw(|frame| render_recurrence_history(frame, &view))
                .unwrap();
            let rendered = terminal
                .backend()
                .buffer()
                .content()
                .iter()
                .map(|cell| cell.symbol())
                .collect::<String>();
            for text in expected_text {
                assert!(rendered.contains(text), "missing rendered control: {text}");
            }
        }
    }

    #[test]
    fn recurrence_history_hit_test_ignores_footer_rows() {
        let entry = RecurrenceHistoryEntry {
            kind: RecurrenceHistoryKind::Missed,
            slot_on: Some("2026-07-19".to_string()),
            interval_started_at: None,
            interval_ended_at: None,
            task_id: None,
            task_ref: None,
            openable: false,
            corrected: false,
            archived_projection: false,
            resolved_at: None,
        };
        let view = RecurrenceHistoryView {
            page: RecurrenceHistoryPage {
                series_ref: "RCR-TEST".to_string(),
                items: vec![entry],
                offset: 0,
                limit: 10,
                total: 1,
                has_more: false,
            },
            selected: Some(RecurrenceHistoryEntryKey::Slot("2026-07-19".to_string())),
            mode: RecurrenceHistoryMode::Browse,
        };
        let terminal = Size::new(80, 12);
        let layout = recurrence_history_layout(&view, terminal);

        assert_eq!(
            recurrence_history_entry_at(&view, terminal, layout.rows.x, layout.rows.y,),
            Some(0)
        );
        assert_eq!(
            recurrence_history_entry_at(&view, terminal, layout.rows.x, layout.rows.bottom(),),
            None
        );
    }
}
