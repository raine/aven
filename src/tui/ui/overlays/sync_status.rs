use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::scroll::{clamp_scroll_start, render_vertical_scrollbar};
use super::super::sync_status_model::{SyncHealth, sync_status_summary};
use super::super::timestamps::{optional_local_timestamp_display, relative_timestamp_display};
use crate::tui::config_overlay::CONFIG_STATUS_TITLE;
use crate::tui::overlay::SyncStatusView;
use crate::tui::store::{SyncStatusCheck, TuiSyncStatus};
use crate::tui::text::cell_width_ranges;
use crate::tui::theme::{BG_ALT, FG, FG_DIM, FG_MUTED, ORANGE, RED};

const LABEL_WIDTH: usize = 16;
const MAX_DIALOG_WIDTH: u16 = 64;

pub(in crate::tui::ui) fn render_sync_status(frame: &mut Frame, view: &SyncStatusView<'_>) {
    let size = frame.area().as_size();
    let width = sync_status_dialog_width(size.width);
    let body_width = width.saturating_sub(5).max(1) as usize;
    let lines = sync_status_lines(view, body_width);
    let height = (lines.len() as u16)
        .saturating_add(4)
        .min(size.height.saturating_sub(2))
        .max(3);
    let visible_rows = height.saturating_sub(4) as usize;
    let start = clamp_scroll_start(view.state.scroll, lines.len(), visible_rows);
    let visible = lines
        .iter()
        .skip(start)
        .take(visible_rows)
        .cloned()
        .collect::<Vec<_>>();
    let dialog = if lines.len() > visible_rows {
        Dialog::new(CONFIG_STATUS_TITLE, width, height).right_title(Line::from(Span::styled(
            scroll_title(start, lines.len(), visible_rows),
            Style::new().fg(FG_MUTED),
        )))
    } else {
        Dialog::new(CONFIG_STATUS_TITLE, width, height)
    };
    let content = dialog.render_block(frame);
    let body = Rect {
        height: content.height.saturating_sub(2),
        ..content
    };
    let footer = Rect {
        y: content.y + content.height.saturating_sub(1),
        height: 1,
        ..content
    };

    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        body,
    );
    frame.render_widget(
        Paragraph::new(hint_line(view, lines.len() > visible_rows))
            .style(Style::new().fg(FG).bg(BG_ALT)),
        footer,
    );
    if lines.len() > visible_rows {
        render_vertical_scrollbar(frame, body, lines.len(), view.state.scroll);
    }
}

fn sync_status_lines(view: &SyncStatusView<'_>, width: usize) -> Vec<Line<'static>> {
    let status = view.status;
    let summary = sync_status_summary(status);
    let color = summary.color();
    let headline = summary.headline();
    let mut lines = vec![Line::from(vec![
        Span::styled("● ", Style::new().fg(color)),
        Span::styled(
            headline,
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])];

    lines.push(Line::from(Span::styled(
        recency_line(status, summary.health, view.syncing, view.now),
        Style::new().fg(FG_MUTED),
    )));

    lines.push(Line::from(""));
    lines.extend(wrapped_row(
        "server",
        summary.server.as_deref().unwrap_or("not configured"),
        Style::new().fg(if summary.server.is_some() {
            FG
        } else {
            FG_MUTED
        }),
        width,
    ));
    lines.extend(wrapped_row(
        "pending",
        &status.pending_changes.to_string(),
        if status.pending_changes > 0 {
            Style::new().fg(ORANGE)
        } else {
            Style::new().fg(FG_MUTED)
        },
        width,
    ));
    lines.extend(wrapped_row(
        "conflicts",
        &status.conflicts.to_string(),
        if status.conflicts > 0 {
            Style::new().fg(ORANGE)
        } else {
            Style::new().fg(FG_MUTED)
        },
        width,
    ));

    if !summary.issues.is_empty() {
        lines.push(Line::from(""));
        for issue in &summary.issues {
            lines.extend(wrapped_row(
                issue.label,
                &issue.value,
                Style::new().fg(if issue.error { RED } else { ORANGE }),
                width,
            ));
        }
    }

    if view.state.details {
        lines.push(Line::from(""));
        lines.push(super::shared::section_line("details"));
        lines.extend(detail_lines(status, width));
    }

    lines
}

fn recency_line(
    status: &TuiSyncStatus,
    health: SyncHealth,
    syncing: bool,
    now: time::OffsetDateTime,
) -> String {
    if syncing {
        return "Manual sync is in progress".to_string();
    }
    match health {
        SyncHealth::RuntimeDisabled => "Sync is disabled by the runtime override".to_string(),
        SyncHealth::LocalOnly => "Tasks are stored on this device".to_string(),
        SyncHealth::Error if status.last_attempt.is_some() => format!(
            "Last attempt {}",
            relative_timestamp_display(status.last_attempt.as_deref(), now, "never")
        ),
        _ if status.last_success.is_some() => format!(
            "Last synced {}",
            relative_timestamp_display(status.last_success.as_deref(), now, "never")
        ),
        _ => "No successful sync recorded".to_string(),
    }
}

fn detail_lines(status: &TuiSyncStatus, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    append_check(
        &mut lines,
        "configured",
        status.configured_server.as_ref(),
        "not configured",
        width,
    );
    lines.extend(wrapped_row(
        "database pin",
        status.pinned_server.as_deref().unwrap_or("none"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    append_check(
        &mut lines,
        "server match",
        status.server_match.as_ref(),
        "not checked",
        width,
    );
    append_check(
        &mut lines,
        "daemon server",
        status.daemon_server.as_ref(),
        "not configured",
        width,
    );
    lines.extend(wrapped_row(
        "auth token",
        if status.auth_token_configured {
            "configured"
        } else {
            "not configured"
        },
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(wrapped_row(
        "interval",
        &format!("{} seconds", status.interval_seconds),
        Style::new().fg(FG_MUTED),
        width,
    ));
    append_check(
        &mut lines,
        "wake address",
        Some(&status.daemon_wake),
        "not checked",
        width,
    );
    lines.extend(wrapped_row(
        "sync cursor",
        status.sync_cursor.as_deref().unwrap_or("missing"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(wrapped_row(
        "local sequence",
        status.local_sequence.as_deref().unwrap_or("missing"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(wrapped_row(
        "last attempt",
        &optional_local_timestamp_display(status.last_attempt.as_deref(), "never"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(wrapped_row(
        "last success",
        &optional_local_timestamp_display(status.last_success.as_deref(), "never"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(wrapped_row(
        "last pushed",
        status.last_pushed.as_deref().unwrap_or("unknown"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(wrapped_row(
        "last pulled",
        status.last_pulled.as_deref().unwrap_or("unknown"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(wrapped_row(
        "last cursor",
        status.last_cursor.as_deref().unwrap_or("unknown"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines
}

fn append_check(
    lines: &mut Vec<Line<'static>>,
    label: &str,
    check: Option<&SyncStatusCheck>,
    fallback: &str,
    width: usize,
) {
    let (value, style) = match check {
        Some(check) if check.ok => (check.value.as_str(), Style::new().fg(FG_MUTED)),
        Some(check) => (check.value.as_str(), Style::new().fg(RED)),
        None => (fallback, Style::new().fg(FG_MUTED)),
    };
    lines.extend(wrapped_row(label, value, style, width));
}

fn wrapped_row(label: &str, value: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    let value_width = width.saturating_sub(LABEL_WIDTH).max(1);
    let ranges = cell_width_ranges(value, value_width);
    ranges
        .into_iter()
        .enumerate()
        .map(|(index, (start, end))| {
            let label = if index == 0 { label } else { "" };
            Line::from(vec![
                Span::styled(format!("{label:<LABEL_WIDTH$}"), Style::new().fg(FG_DIM)),
                Span::styled(value[start..end].to_string(), style),
            ])
        })
        .collect()
}

fn hint_line(view: &SyncStatusView<'_>, scrolling: bool) -> Line<'static> {
    let mut hints = Vec::new();
    let summary = sync_status_summary(view.status);
    if scrolling {
        hints.push(("j/k", "scroll"));
    }
    if !view.syncing && summary.can_manual_sync {
        hints.push((
            "S",
            if view.status.last_error_value().is_some() {
                "retry"
            } else {
                "sync"
            },
        ));
    }
    if view.status.conflicts > 0 {
        hints.push(("c", "open"));
    }
    hints.push((
        "d",
        if view.state.details {
            "summary"
        } else {
            "details"
        },
    ));
    hints.push(("Esc", "close"));
    dialog_hint_line(&hints)
}

fn scroll_title(start: usize, total: usize, visible: usize) -> String {
    let current = start.saturating_add(1).min(total);
    let last = start.saturating_add(visible).min(total);
    format!(" {current}-{last}/{total} ")
}

fn sync_status_dialog_width(frame_width: u16) -> u16 {
    frame_width.saturating_sub(4).clamp(1, MAX_DIALOG_WIDTH)
}

pub(crate) fn sync_status_scroll_cap(
    status: &TuiSyncStatus,
    details: bool,
    terminal_size: Size,
) -> u16 {
    let width = sync_status_dialog_width(terminal_size.width);
    let body_width = width.saturating_sub(5).max(1) as usize;
    let view = SyncStatusView {
        state: crate::tui::overlay::SyncStatusState { details, scroll: 0 },
        status,
        syncing: false,
        now: time::OffsetDateTime::UNIX_EPOCH,
    };
    let line_count = sync_status_lines(&view, body_width).len();
    let height = (line_count as u16)
        .saturating_add(4)
        .min(terminal_size.height.saturating_sub(2))
        .max(3);
    let visible_rows = height.saturating_sub(4) as usize;
    line_count
        .saturating_sub(visible_rows)
        .min(u16::MAX as usize) as u16
}

#[cfg(test)]
pub(in crate::tui::ui) fn sync_status_lines_for_test(
    view: &SyncStatusView<'_>,
) -> Vec<Line<'static>> {
    sync_status_lines(view, 60)
}
