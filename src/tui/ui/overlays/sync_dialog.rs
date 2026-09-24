use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::scroll::{clamp_scroll_start, render_vertical_scrollbar};
use super::super::sync_status_model::{SyncHealth, sync_status_summary};
use crate::sync::encrypted::LocalPhase;
use crate::tui::overlay::{SyncDialogView, dialog_area, sync_actions};
use crate::tui::store::TuiSyncStatus;
use crate::tui::text::cell_width_ranges;
use crate::tui::theme::{BG_ALT, FG, FG_DIM, FG_MUTED, ORANGE, RED, SELECTED};

pub(crate) const SYNC_TITLE: &str = "Sync";
const LABEL_WIDTH: usize = 16;
const MAX_DIALOG_WIDTH: u16 = 64;

/// Where a click on the Sync dialog lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SyncDialogHit {
    Action(usize),
    Inside,
    Outside,
}

struct Body {
    lines: Vec<Line<'static>>,
    /// Body line of each action, in action order.
    action_lines: Vec<usize>,
}

struct Layout {
    area: Rect,
    body_area: Rect,
    footer: Rect,
    start: usize,
    visible_rows: usize,
    body: Body,
}

impl Layout {
    fn new(view: &SyncDialogView<'_>, terminal: Size) -> Self {
        let width = dialog_width(terminal.width);
        let body = body(view, width.saturating_sub(5).max(1) as usize);
        let height = (body.lines.len() as u16)
            .saturating_add(4)
            .min(terminal.height.saturating_sub(2))
            .max(3);
        let visible_rows = height.saturating_sub(4) as usize;
        let mut start = clamp_scroll_start(view.state.scroll, body.lines.len(), visible_rows);
        // Keep the focused action visible on short terminals.
        if let Some(&line) = body.action_lines.get(view.state.selected) {
            if line < start {
                start = line;
            } else if visible_rows > 0 && line >= start + visible_rows {
                start = line + 1 - visible_rows;
            }
        }
        let area = dialog_area(
            Rect::new(0, 0, terminal.width, terminal.height),
            width,
            height,
        );
        // Border and horizontal padding surround the content.
        let content = Rect {
            x: area.x.saturating_add(2),
            y: area.y.saturating_add(1),
            width: area.width.saturating_sub(4),
            height: area.height.saturating_sub(2),
        };
        Self {
            area,
            body_area: Rect {
                height: content.height.saturating_sub(2),
                ..content
            },
            footer: Rect {
                y: content.y + content.height.saturating_sub(1),
                height: 1,
                ..content
            },
            start,
            visible_rows,
            body,
        }
    }

    fn scrolling(&self) -> bool {
        self.body.lines.len() > self.visible_rows
    }
}

pub(in crate::tui::ui) fn render_sync_dialog(frame: &mut Frame, view: &SyncDialogView<'_>) {
    let layout = Layout::new(view, frame.area().as_size());
    let visible = layout
        .body
        .lines
        .iter()
        .skip(layout.start)
        .take(layout.visible_rows)
        .cloned()
        .collect::<Vec<_>>();
    let mut dialog = Dialog::new(SYNC_TITLE, layout.area.width, layout.area.height);
    if layout.scrolling() {
        dialog = dialog.right_title(Line::from(Span::styled(
            scroll_title(layout.start, layout.body.lines.len(), layout.visible_rows),
            Style::new().fg(FG_MUTED),
        )));
    }
    dialog.render_block_at(frame, layout.area);
    frame.render_widget(
        Paragraph::new(Text::from(visible)).style(Style::new().fg(FG).bg(BG_ALT)),
        layout.body_area,
    );
    frame.render_widget(
        Paragraph::new(hint_line(view, layout.scrolling())).style(Style::new().fg(FG).bg(BG_ALT)),
        layout.footer,
    );
    if layout.scrolling() {
        render_vertical_scrollbar(
            frame,
            layout.body_area,
            layout.body.lines.len(),
            layout.start as u16,
        );
    }
}

pub(crate) fn sync_dialog_hit(
    view: &SyncDialogView<'_>,
    terminal: Size,
    column: u16,
    row: u16,
) -> SyncDialogHit {
    let layout = Layout::new(view, terminal);
    if !layout.area.contains((column, row).into()) {
        return SyncDialogHit::Outside;
    }
    if !layout.body_area.contains((column, row).into()) {
        return SyncDialogHit::Inside;
    }
    let line = layout.start + usize::from(row - layout.body_area.y);
    layout
        .body
        .action_lines
        .iter()
        .position(|&action_line| action_line == line)
        .map_or(SyncDialogHit::Inside, SyncDialogHit::Action)
}

pub(crate) fn sync_dialog_scroll_cap(view: &SyncDialogView<'_>, terminal: Size) -> u16 {
    let layout = Layout::new(view, terminal);
    layout
        .body
        .lines
        .len()
        .saturating_sub(layout.visible_rows)
        .min(u16::MAX as usize) as u16
}

fn body(view: &SyncDialogView<'_>, width: usize) -> Body {
    let status = view.status;
    let summary = sync_status_summary(status);
    let color = summary.color();
    let mut lines = vec![Line::from(vec![
        Span::styled("● ", Style::new().fg(color)),
        Span::styled(
            summary.headline(),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
    ])];
    lines.push(Line::from(Span::styled(
        state_line(status, summary.health, view.syncing),
        Style::new().fg(FG_MUTED),
    )));

    lines.push(Line::from(""));
    lines.extend(wrapped_row(
        "automatic",
        if status.enabled { "on" } else { "off" },
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(wrapped_row(
        "pending",
        &status.pending_changes.to_string(),
        attention_style(status.pending_changes > 0),
        width,
    ));
    lines.extend(wrapped_row(
        "conflicts",
        &status.conflicts.to_string(),
        attention_style(status.conflicts > 0),
        width,
    ));

    if !summary.issues.is_empty() {
        lines.push(Line::from(""));
        for issue in &summary.issues {
            lines.extend(wrapped_row(
                issue.label,
                &issue.value,
                Style::new().fg(ORANGE),
                width,
            ));
        }
    }

    let actions = sync_actions(view.state, status);
    let mut action_lines = Vec::with_capacity(actions.len());
    if !actions.is_empty() {
        lines.push(Line::from(""));
        for (index, action) in actions.iter().enumerate() {
            action_lines.push(lines.len());
            lines.push(action_line(action.label(), index == view.state.selected));
        }
    }

    if view.state.details {
        lines.push(Line::from(""));
        lines.push(super::shared::section_line("details"));
        lines.extend(detail_lines(status, width));
    }

    Body {
        lines,
        action_lines,
    }
}

fn action_line(label: &'static str, focused: bool) -> Line<'static> {
    if focused {
        Line::from(Span::styled(format!("› {label}"), SELECTED))
    } else {
        Line::from(Span::styled(format!("  {label}"), Style::new().fg(FG)))
    }
}

fn attention_style(attention: bool) -> Style {
    Style::new().fg(if attention { ORANGE } else { FG_MUTED })
}

fn state_line(status: &TuiSyncStatus, health: SyncHealth, syncing: bool) -> &'static str {
    if syncing {
        return "Syncing now";
    }
    match (health, status.phase) {
        (SyncHealth::RuntimeDisabled, _) => "Sync is disabled by the runtime override",
        (_, LocalPhase::NotSetUp) => "Run `aven sync setup` or `aven sync join` to sync",
        (_, LocalPhase::SetupIncomplete) => "Setup is unfinished",
        (_, LocalPhase::JoinIncomplete) => "Joining is unfinished",
        (_, LocalPhase::SetUp) => "Sync is end-to-end encrypted",
    }
}

fn detail_lines(status: &TuiSyncStatus, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.extend(wrapped_row(
        "interval",
        &format!("{} seconds", status.interval_seconds),
        Style::new().fg(FG_MUTED),
        width,
    ));
    let wake_style = Style::new().fg(if status.daemon_wake.ok { FG_MUTED } else { RED });
    lines.extend(wrapped_row(
        "wake address",
        &status.daemon_wake.value,
        wake_style,
        width,
    ));
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
    lines
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

fn hint_line(view: &SyncDialogView<'_>, scrolling: bool) -> Line<'static> {
    let mut hints = Vec::new();
    let actions = sync_actions(view.state, view.status);
    if actions.len() > 1 {
        hints.push(("↑↓", "select"));
    } else if scrolling {
        hints.push(("j/k", "scroll"));
    }
    if !actions.is_empty() {
        hints.push(("Enter", "choose"));
    }
    if view.status.conflicts > 0 {
        hints.push(("c", "conflicts"));
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

fn dialog_width(frame_width: u16) -> u16 {
    frame_width.saturating_sub(4).clamp(1, MAX_DIALOG_WIDTH)
}

#[cfg(test)]
pub(in crate::tui::ui) fn sync_dialog_lines_for_test(
    view: &SyncDialogView<'_>,
) -> Vec<Line<'static>> {
    body(view, 60).lines
}
