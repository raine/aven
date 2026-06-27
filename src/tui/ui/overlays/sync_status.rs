use ratatui::Frame;
use ratatui::style::Style;
use ratatui::text::{Line, Text};
use ratatui::widgets::Paragraph;

use super::super::dialog::{Dialog, dialog_hint_line};
use crate::tui::config_overlay::CONFIG_STATUS_TITLE;
use crate::tui::store::{SyncStatusCheck, TuiSyncStatus};
use crate::tui::theme::{BG_ALT, FG, FG_MUTED, GREEN, RED};

pub(in crate::tui::ui) fn render_sync_status(frame: &mut Frame, status: &TuiSyncStatus) {
    let width = frame.area().width.saturating_sub(8).clamp(64, 88);
    let lines = sync_status_lines(status);
    let height = (lines.len() as u16)
        .saturating_add(2)
        .min(frame.area().height.saturating_sub(2))
        .max(3);
    let content = Dialog::new(CONFIG_STATUS_TITLE, width, height).render_block(frame);

    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG_ALT)),
        content,
    );
}

fn sync_status_lines(status: &TuiSyncStatus) -> Vec<Line<'static>> {
    let mut lines = vec![
        section_line("connection"),
        status_row(
            "server reach",
            reachability(status),
            if status.last_error_value().is_some() {
                Style::new().fg(RED)
            } else if status.last_success.is_some() {
                Style::new().fg(GREEN)
            } else {
                Style::new().fg(FG_MUTED)
            },
        ),
        check_row(
            "configured server",
            status.configured_server.as_ref(),
            "not configured",
        ),
        value_row(
            "pinned server",
            status.pinned_server.as_deref().unwrap_or("none"),
            Style::new().fg(FG_MUTED),
        ),
    ];
    if let Some(server_match) = &status.server_match {
        lines.push(status_row(
            "server match",
            if server_match.ok {
                "yes".to_string()
            } else {
                server_match.value.clone()
            },
            check_style(server_match),
        ));
    }
    lines.extend([
        check_row(
            "daemon server",
            status.daemon_server.as_ref(),
            "not configured",
        ),
        status_row(
            "auth token",
            if status.auth_token_configured {
                "configured".to_string()
            } else {
                "not configured".to_string()
            },
            if status.auth_token_configured {
                Style::new().fg(GREEN)
            } else {
                Style::new().fg(FG_MUTED)
            },
        ),
        value_row(
            "interval",
            format!("{} seconds", status.interval_seconds),
            Style::new().fg(FG_MUTED),
        ),
        check_row("daemon wake", Some(&status.daemon_wake), "not checked"),
        Line::from(""),
        section_line("state"),
        count_row("pending changes", status.pending_changes),
        count_row("conflicts", status.conflicts),
        value_row(
            "sync cursor",
            status.sync_cursor.as_deref().unwrap_or("missing"),
            Style::new().fg(FG_MUTED),
        ),
        value_row(
            "local sequence",
            status.local_sequence.as_deref().unwrap_or("missing"),
            Style::new().fg(FG_MUTED),
        ),
        Line::from(""),
        section_line("last sync"),
        value_row(
            "last attempt",
            status.last_attempt.as_deref().unwrap_or("never"),
            Style::new().fg(FG_MUTED),
        ),
        value_row(
            "last synced",
            status.last_success.as_deref().unwrap_or("never"),
            if status.last_success.is_some() {
                Style::new().fg(GREEN)
            } else {
                Style::new().fg(FG_MUTED)
            },
        ),
        value_row(
            "last error",
            status.last_error_value().unwrap_or("none"),
            if status.last_error_value().is_some() {
                Style::new().fg(RED)
            } else {
                Style::new().fg(FG_MUTED)
            },
        ),
        value_row(
            "last pushed",
            status.last_pushed.as_deref().unwrap_or("unknown"),
            Style::new().fg(FG_MUTED),
        ),
        value_row(
            "last pulled",
            status.last_pulled.as_deref().unwrap_or("unknown"),
            Style::new().fg(FG_MUTED),
        ),
        value_row(
            "last cursor",
            status.last_cursor.as_deref().unwrap_or("unknown"),
            Style::new().fg(FG_MUTED),
        ),
        Line::from(""),
        dialog_hint_line(&[("Enter/Esc", "close")]),
    ]);
    if let Some(error) = &status.config_error {
        lines.insert(1, status_row("config", error.clone(), Style::new().fg(RED)));
    }
    lines
}

const LABEL_WIDTH: usize = 18;

fn section_line(label: &str) -> Line<'static> {
    super::shared::section_line(label)
}

fn count_row(label: &str, value: i64) -> Line<'static> {
    super::shared::count_row(LABEL_WIDTH, label, value)
}

fn value_row(label: &str, value: impl Into<String>, value_style: Style) -> Line<'static> {
    super::shared::value_row(LABEL_WIDTH, label, value, value_style)
}

fn check_row(label: &str, check: Option<&SyncStatusCheck>, fallback: &str) -> Line<'static> {
    match check {
        Some(check) => status_row(label, check.value.clone(), check_style(check)),
        None => value_row(label, fallback, Style::new().fg(FG_MUTED)),
    }
}

fn status_row(label: &str, value: String, value_style: Style) -> Line<'static> {
    value_row(label, value, value_style)
}

fn check_style(check: &SyncStatusCheck) -> Style {
    if check.ok {
        Style::new().fg(GREEN)
    } else {
        Style::new().fg(RED)
    }
}

fn reachability(status: &TuiSyncStatus) -> String {
    if !status.enabled {
        return "sync disabled".to_string();
    }
    if status.last_error_value().is_some() {
        return "last attempt failed".to_string();
    }
    if status.last_success.is_some() {
        return "last sync reached server".to_string();
    }
    "unknown, no sync recorded".to_string()
}

#[cfg(test)]
pub(in crate::tui::ui) fn sync_status_lines_for_test(status: &TuiSyncStatus) -> Vec<Line<'static>> {
    sync_status_lines(status)
}
