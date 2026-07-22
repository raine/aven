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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RecurrenceHistoryLayout {
    pub(crate) area: Rect,
    pub(crate) rows: Rect,
}

pub(crate) fn recurrence_history_layout(
    terminal: Size,
    item_count: usize,
) -> RecurrenceHistoryLayout {
    let mode_rows = 4;
    let height = (item_count as u16)
        .saturating_add(mode_rows)
        .clamp(6, terminal.height);
    let area = crate::tui::overlay::dialog_area(
        Rect::new(0, 0, terminal.width, terminal.height),
        HISTORY_WIDTH,
        height,
    );
    let inner = Rect::new(
        area.x.saturating_add(2),
        area.y.saturating_add(1),
        area.width.saturating_sub(4),
        area.height.saturating_sub(2),
    );
    let rows = Rect::new(
        inner.x,
        inner.y,
        inner.width,
        (item_count as u16).min(inner.height.saturating_sub(1)),
    );
    RecurrenceHistoryLayout { area, rows }
}

pub(crate) fn recurrence_history_entry_at(
    state: &RecurrenceHistoryView,
    terminal: Size,
    column: u16,
    row: u16,
) -> Option<usize> {
    let layout = recurrence_history_layout(terminal, state.page.items.len());
    if column < layout.rows.x
        || column >= layout.rows.right()
        || row < layout.rows.y
        || row >= layout.rows.bottom()
    {
        return None;
    }
    let index = row.saturating_sub(layout.rows.y) as usize;
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
    let mut lines = state
        .page
        .items
        .iter()
        .map(|entry| history_line(entry, state.selected.as_ref()))
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

fn history_line<'a>(
    entry: &'a RecurrenceHistoryEntry,
    selected: Option<&RecurrenceHistoryEntryKey>,
) -> Line<'a> {
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
    Line::from(vec![
        Span::styled(if is_selected { "> " } else { "  " }, style),
        Span::styled(
            format!("{identity:<25} {kind:<9} resolved {resolved:<25} task {task:<12} {markers}"),
            style,
        ),
    ])
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
        let layout = recurrence_history_layout(terminal, 1);

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
