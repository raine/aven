use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::inline_code::wrap_with_code;
use super::super::input::cursor_cell;
use super::super::scroll::{clamp_scroll_start, render_vertical_scrollbar};
use super::super::sync_status_model::{SyncHealth, sync_status_summary};

use crate::sync::encrypted::Removal;
use crate::sync::encrypted::{InvitationCheck, LocalPhase, SetupPreview, Stage};
use crate::tui::overlay::{
    AutomaticSyncService, InvitationKind, SecretText, SyncAction, SyncDialogView, SyncPage,
    dialog_area, sync_actions,
};
use crate::tui::store::TuiSyncStatus;
use crate::tui::sync_errors::JOIN_TIMEOUT_EXPIRED;
use crate::tui::sync_operations::{
    DrainSummary, OperationFailure, OperationKind, OperationResult, RunningOperation, SyncActivity,
    short_device_ids,
};
use crate::tui::text::truncate_width;
use crate::tui::theme::{
    ACCENT, BG, BG_ALT, BG_PANEL, FG, FG_DIM, FG_MUTED, GREEN, INVERSE_FG, ORANGE, RED, SELECTED,
};

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
    /// Body line and cell range of each action, in action order.
    actions: Vec<ActionArea>,
}

#[derive(Clone, Copy)]
struct ActionArea {
    line: usize,
    start: u16,
    end: u16,
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
        if let Some(&ActionArea { line, .. }) = body.actions.get(view.state.selected) {
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
    let band = Rect {
        x: frame.area().x,
        width: frame.area().width,
        ..layout.area
    };
    frame.render_widget(Block::new().style(Style::new().bg(BG)), band);
    let visible = layout
        .body
        .lines
        .iter()
        .skip(layout.start)
        .take(layout.visible_rows)
        .cloned()
        .collect::<Vec<_>>();
    let title = match page_title(view) {
        Some(page) => format!("{SYNC_TITLE} › {page}"),
        None => SYNC_TITLE.to_string(),
    };
    let mut dialog = Dialog::new(&title, layout.area.width, layout.area.height);
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
    let column = column - layout.body_area.x;
    layout
        .body
        .actions
        .iter()
        .position(|area| area.line == line && (area.start..area.end).contains(&column))
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

/// The sub-page shown after "Sync" in the border title; the top-level page
/// has none unless it shows setup or join progress.
fn page_title(view: &SyncDialogView<'_>) -> Option<&'static str> {
    Some(match &view.state.page {
        SyncPage::Home => return view.activity.running.as_ref().and_then(progress_title),
        SyncPage::Invitation {
            kind: InvitationKind::Setup,
            ..
        } if view.status.phase == LocalPhase::SetupIncomplete => "Resume setup",
        SyncPage::Invitation {
            kind: InvitationKind::Setup,
            ..
        }
        | SyncPage::ConfirmSetup { .. } => "Set up sync",
        SyncPage::Invitation {
            kind: InvitationKind::Join,
            ..
        } if view.status.phase == LocalPhase::JoinIncomplete => "Use a new invitation",
        SyncPage::ConfirmJoin { replace: true, .. } => "Use a new invitation",
        SyncPage::Invitation { .. } | SyncPage::ConfirmJoin { .. } => "Join existing sync",
        SyncPage::Devices => "Manage devices",
        SyncPage::ConfirmRemove { .. } => "Remove device",
        SyncPage::ConfirmAutomaticSync { .. } => "Sync automatically",
    })
}

fn body(view: &SyncDialogView<'_>, width: usize) -> Body {
    let mut body = Body {
        lines: Vec::new(),
        actions: Vec::new(),
    };
    match &view.state.page {
        SyncPage::Home => home_lines(&mut body, view, width),
        SyncPage::Invitation { kind, input, error } => {
            invitation_lines(&mut body, view.status, *kind, input, *error, width)
        }
        SyncPage::ConfirmSetup {
            server, preview, ..
        } => confirm_setup_lines(&mut body, server, preview, width),
        SyncPage::ConfirmJoin {
            server, replace, ..
        } => confirm_join_lines(&mut body, server, *replace, width),
        SyncPage::ConfirmRemove { device } => {
            confirm_remove_lines(&mut body, view.activity, device, width)
        }
        SyncPage::ConfirmAutomaticSync { service } => {
            confirm_automatic_sync_lines(&mut body, service, view.status.interval_seconds, width)
        }
        SyncPage::Devices => {
            devices_lines(&mut body, view, width);
            return body;
        }
    }
    let actions = sync_actions(view.state, view.status, view.activity);
    if view.state.page.has_buttons() {
        body.lines.push(Line::from(""));
        push_buttons(&mut body, &actions, view.state.selected, width);
    } else if !actions.is_empty() {
        body.lines.push(Line::from(""));
        for (index, action) in actions.iter().enumerate() {
            body.actions.push(ActionArea {
                line: body.lines.len(),
                start: 0,
                end: width as u16,
            });
            body.lines
                .push(action_line(action.label(), index == view.state.selected));
        }
    }
    if view.state.page == SyncPage::Home && view.state.details {
        body.lines.push(Line::from(""));
        body.lines.push(super::shared::section_line("details"));
        body.lines.extend(detail_lines(view.status, width));
        if let Some(OperationResult::Failed(failure)) = &view.activity.last {
            body.lines.extend(wrapped_row(
                "Last error",
                &failure.message,
                Style::new().fg(FG_MUTED),
                width,
            ));
        }
    }
    body
}

fn home_lines(body: &mut Body, view: &SyncDialogView<'_>, width: usize) {
    let status = view.status;
    let lines = &mut body.lines;
    if status.phase == LocalPhase::NotSetUp && view.activity.running.is_none() {
        lines.extend(paragraph(
            "Keep your tasks in sync across devices. Set up sync from this computer's \
             tasks, or join sync that another device already uses.",
            Style::new().fg(FG_MUTED),
            width,
        ));
        push_last_result(lines, view.activity, width);
        return;
    }
    if let Some(running) = &view.activity.running
        && matches!(running.kind, OperationKind::Setup | OperationKind::Join)
    {
        progress_lines(lines, running, width);
        if running.kind == OperationKind::Join {
            push_expired_join_guidance(lines, view.activity, width);
        }
        return;
    }
    let summary = sync_status_summary(status);
    // A manual sync replaces the status in place, so nothing below moves.
    let (mark, headline, color) = match view.syncing {
        Some(started_at) => (spinner(started_at), "Syncing…", ACCENT),
        None => ("●", summary.headline(), summary.color()),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{mark} "), Style::new().fg(color)),
        Span::styled(
            headline,
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]));
    if let Some(state) = state_line(status, summary.health) {
        lines.push(Line::from(Span::styled(state, Style::new().fg(FG_MUTED))));
    }
    if summary.health == SyncHealth::AccessRefused {
        lines.extend(paragraph(
            "The server refused this device. It may have been removed from sync; check from \
             another device. Local tasks and images stay here.",
            Style::new().fg(FG),
            width,
        ));
    }

    if let Some(running) = &view.activity.running {
        lines.push(Line::from(""));
        progress_lines(lines, running, width);
        return;
    }
    push_last_result(lines, view.activity, width);
    if status.phase == LocalPhase::JoinIncomplete {
        push_expired_join_guidance(lines, view.activity, width);
    }

    lines.push(Line::from(""));
    if let Some(server) = &status.server {
        lines.extend(wrapped_row("Server", server, Style::new().fg(FG), width));
    }
    lines.extend(wrapped_row(
        "Automatic",
        if status.enabled { "on" } else { "off" },
        Style::new().fg(FG_MUTED),
        width,
    ));
    // Counts appear only when they need attention; the status covers zero.
    if status.pending_changes > 0 {
        lines.extend(wrapped_row(
            "Pending",
            &status.pending_changes.to_string(),
            Style::new().fg(ORANGE),
            width,
        ));
    }
    if status.conflicts > 0 {
        lines.extend(wrapped_row(
            "Conflicts",
            &status.conflicts.to_string(),
            Style::new().fg(ORANGE),
            width,
        ));
    }
    if let Some(invitation) = status.invitation {
        lines.extend(wrapped_row(
            "Invitation",
            &format!(
                "open, expires {}",
                crate::sync::encrypted::format_expiry(invitation.expires_at)
            ),
            Style::new().fg(ORANGE),
            width,
        ));
    }
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
}

/// Stages a running operation passes through, in order, with their active
/// and completed wording.
fn steps(kind: OperationKind) -> &'static [(Stage, &'static str, &'static str)] {
    match kind {
        OperationKind::Setup => &[
            (
                Stage::PreparingData,
                "Preparing your data",
                "Prepared your data",
            ),
            (
                Stage::UploadingData,
                "Uploading encrypted data",
                "Uploaded encrypted data",
            ),
            (Stage::FinishingSetup, "Finishing setup", "Finished setup"),
        ],
        OperationKind::Join => &[
            (
                Stage::WaitingForInviter,
                "Waiting for the other device",
                "The other device added this one",
            ),
            (
                Stage::DownloadingTasks,
                "Downloading tasks",
                "Downloaded tasks",
            ),
            (
                Stage::CatchingUp,
                "Catching up with recent changes",
                "Caught up with recent changes",
            ),
            (
                Stage::DownloadingImages,
                "Downloading images",
                "Downloaded images",
            ),
        ],
        OperationKind::Sync
        | OperationKind::ListDevices
        | OperationKind::RemoveDevice(_)
        | OperationKind::FinishRemoval => &[],
    }
}

/// Operations with staged progress name themselves in the border title.
fn progress_title(running: &RunningOperation) -> Option<&'static str> {
    match running.kind {
        OperationKind::Setup => Some("Setting up sync"),
        OperationKind::Join => Some("Joining sync"),
        OperationKind::Sync
        | OperationKind::ListDevices
        | OperationKind::RemoveDevice(_)
        | OperationKind::FinishRemoval => None,
    }
}

fn progress_lines(lines: &mut Vec<Line<'static>>, running: &RunningOperation, width: usize) {
    if progress_title(running).is_none() {
        lines.push(spinner_line(running));
        return;
    }
    let steps = steps(running.kind);
    let current = running
        .stage
        .and_then(|stage| steps.iter().position(|(step, ..)| *step == stage))
        .unwrap_or(0);
    for (index, (_, active, done)) in steps.iter().enumerate() {
        let line = if index < current {
            Line::from(vec![
                Span::styled("✓ ", Style::new().fg(GREEN)),
                Span::styled(*done, Style::new().fg(FG_MUTED)),
            ])
        } else if index == current {
            Line::from(vec![
                Span::styled(
                    format!("{} ", spinner(running.started_at)),
                    Style::new().fg(ACCENT),
                ),
                Span::styled(*active, Style::new().fg(FG)),
            ])
        } else {
            Line::from(vec![
                Span::styled("· ", Style::new().fg(FG_DIM)),
                Span::styled(*active, Style::new().fg(FG_DIM)),
            ])
        };
        lines.push(line);
    }
    lines.push(Line::from(""));
    let note = match (running.kind, running.stage) {
        (OperationKind::Join, Some(Stage::CatchingUp | Stage::DownloadingImages)) => {
            "Tasks are available. You can close this dialog and keep working while the \
             rest downloads."
        }
        (OperationKind::Join, _) => {
            "You can close this dialog. Editing waits until tasks are downloaded. Keep Add \
             device open on the other device."
        }
        _ => "You can close this dialog and keep working.",
    };
    lines.extend(paragraph(note, Style::new().fg(FG_MUTED), width));
}

/// After a join timed out in this session, explains what to do if the
/// invitation expired. A timeout alone does not show that it did.
fn push_expired_join_guidance(
    lines: &mut Vec<Line<'static>>,
    activity: &SyncActivity,
    width: usize,
) {
    if activity.join_timed_out {
        lines.push(Line::from(""));
        lines.extend(paragraph(
            JOIN_TIMEOUT_EXPIRED,
            Style::new().fg(FG_MUTED),
            width,
        ));
    }
}

/// Device operations report no engine stages; one truthful line covers them.
fn spinner_line(running: &RunningOperation) -> Line<'static> {
    let text = match running.kind {
        OperationKind::RemoveDevice(_) => "Removing access and securing future changes",
        OperationKind::FinishRemoval => "Securing future changes",
        _ => "Checking devices with the server",
    };
    Line::from(vec![
        Span::styled(
            format!("{} ", spinner(running.started_at)),
            Style::new().fg(ACCENT),
        ),
        Span::styled(text, Style::new().fg(FG)),
    ])
}

pub(super) fn spinner(started_at: Instant) -> &'static str {
    let frames = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
    frames[(started_at.elapsed().as_millis() as usize / 120) % frames.len()]
}

fn push_last_result(lines: &mut Vec<Line<'static>>, activity: &SyncActivity, width: usize) {
    let Some(result) = activity
        .last
        .as_ref()
        .filter(|_| activity.device_result().is_none())
    else {
        return;
    };
    match result {
        // A complete result says nothing the status and server rows don't.
        OperationResult::SetUp { drain, .. } | OperationResult::Joined { drain, .. } => {
            let Some(text) = drain_text(drain) else {
                return;
            };
            let headline = match result {
                OperationResult::SetUp { .. } => "Sync is set up",
                _ => "Joined sync",
            };
            lines.push(Line::from(""));
            lines.extend(paragraph_with_mark("✓", GREEN, headline, width));
            lines.extend(paragraph(text, Style::new().fg(FG_MUTED), width));
        }
        OperationResult::Removed(_) | OperationResult::RemovalFinished { .. } => {}
        OperationResult::Failed(failure) => {
            lines.push(Line::from(""));
            failure_lines(lines, failure, width);
        }
    }
}

fn failure_lines(lines: &mut Vec<Line<'static>>, failure: &OperationFailure, width: usize) {
    let headline = match failure.kind {
        OperationKind::Sync => "Sync didn't finish",
        OperationKind::Setup => "Setup didn't finish",
        OperationKind::Join => "Couldn't join sync",
        OperationKind::ListDevices => "Couldn't check devices",
        OperationKind::RemoveDevice(_) => "Removal didn't finish",
        OperationKind::FinishRemoval => "Securing future changes didn't finish",
    };
    lines.extend(paragraph_with_mark("!", ORANGE, headline, width));
    lines.extend(paragraph(&failure.message, Style::new().fg(FG), width));
    lines.extend(paragraph(
        "Press d for details.",
        Style::new().fg(FG_DIM),
        width,
    ));
}

fn short_id(device: &[u8; 32]) -> String {
    format!("{}…", &hex::encode(device)[..8])
}

fn devices_lines(body: &mut Body, view: &SyncDialogView<'_>, width: usize) {
    let activity = view.activity;
    let lines = &mut body.lines;
    let listing_failed = matches!(
        activity.device_result(),
        Some(OperationResult::Failed(failure)) if failure.kind == OperationKind::ListDevices
    );
    let freshness = match (&activity.devices, activity.running) {
        (_, Some(running)) if running.kind == OperationKind::ListDevices => None,
        (Some(snapshot), _) if listing_failed => Some(format!(
            "Showing the list from {}; the latest check failed.",
            elapsed(snapshot.checked_at)
        )),
        (Some(snapshot), _) if snapshot.after_removal => Some(format!(
            "Updated by the removal {}.",
            elapsed(snapshot.checked_at)
        )),
        (Some(snapshot), _) => Some(format!(
            "Checked with the server {}.",
            elapsed(snapshot.checked_at)
        )),
        (None, _) => None,
    };
    if let Some(freshness) = freshness {
        lines.extend(paragraph(&freshness, Style::new().fg(FG_MUTED), width));
    }
    if let Some(running) = &activity.running {
        lines.push(spinner_line(running));
    }
    match activity.device_result() {
        Some(OperationResult::Removed(removal)) => removal_lines(lines, removal, width),
        Some(OperationResult::RemovalFinished {
            key_rotation_pending,
        }) => {
            if *key_rotation_pending {
                lines.extend(paragraph_with_mark(
                    "!",
                    ORANGE,
                    "Securing future changes is still unfinished",
                    width,
                ));
                lines.extend(paragraph(
                    "Try again later. Any remaining device also finishes it when it syncs.",
                    Style::new().fg(FG_MUTED),
                    width,
                ));
            } else {
                lines.extend(paragraph_with_mark(
                    "✓",
                    GREEN,
                    "Future changes are secured",
                    width,
                ));
            }
        }
        Some(OperationResult::Failed(failure)) => failure_lines(lines, failure, width),
        _ => {
            if activity
                .devices
                .as_ref()
                .is_some_and(|snapshot| snapshot.listing.key_rotation_pending)
            {
                lines.extend(paragraph_with_mark(
                    "!",
                    ORANGE,
                    "Securing future changes after a removal is unfinished",
                    width,
                ));
            }
        }
    }

    let actions = sync_actions(view.state, view.status, activity);
    let devices = activity
        .devices
        .as_ref()
        .map(|snapshot| snapshot.listing.devices.as_slice())
        .unwrap_or_default();
    let short_ids = short_device_ids(devices);
    lines.push(Line::from(""));
    if devices.is_empty() && activity.running.is_none() && !listing_failed {
        lines.push(Line::from(Span::styled(
            "No devices listed yet. Press r to check with the server.",
            Style::new().fg(FG_DIM),
        )));
    }
    let columns = DeviceColumns::new(devices, &short_ids, width);
    let mut selected_device = None;
    for (index, action) in actions.iter().enumerate() {
        let focused = index == view.state.selected;
        let row = match action {
            SyncAction::Device(device_index) => {
                let device = &devices[*device_index];
                if focused {
                    selected_device = Some(device);
                }
                columns.row(device, &short_ids[*device_index], focused)
            }
            action => action_line(action.label(), focused),
        };
        body.actions.push(ActionArea {
            line: lines.len(),
            start: 0,
            end: width as u16,
        });
        lines.push(row);
    }

    if let Some(device) = selected_device {
        lines.push(Line::from(""));
        if let Some(label) = &device.label {
            lines.extend(wrapped_row("Name", label, Style::new().fg(FG), width));
        }
        lines.extend(wrapped_row(
            "Device ID",
            &elide_middle(&hex::encode(device.id), width.saturating_sub(LABEL_WIDTH)),
            Style::new().fg(FG),
            width,
        ));
        if device.current {
            lines.extend(paragraph(
                "Remove it from another device.",
                Style::new().fg(FG_DIM),
                width,
            ));
        }
    }
    if view.state.details
        && let Some(OperationResult::Failed(failure)) = activity.device_result()
    {
        lines.push(Line::from(""));
        lines.push(super::shared::section_line("details"));
        lines.extend(paragraph(
            &failure.message,
            Style::new().fg(FG_MUTED),
            width,
        ));
    }
}

const UNNAMED_DEVICE: &str = "Unnamed device";
const THIS_DEVICE: &str = "This device";
const COLUMN_GAP: usize = 3;

/// Column widths for the device list. The name column shrinks first so a row
/// never wraps; the ID prefix and status keep their full width.
struct DeviceColumns {
    name: usize,
    id: usize,
}

impl DeviceColumns {
    fn new(devices: &[crate::sync::encrypted::Device], short_ids: &[String], width: usize) -> Self {
        let id = short_ids
            .iter()
            .map(|id| id.trim_end_matches('…').len())
            .max()
            .unwrap_or(0);
        let status = if devices.iter().any(|device| device.current) {
            THIS_DEVICE.width()
        } else {
            0
        };
        let widest_name = devices
            .iter()
            .map(|device| device.label.as_deref().unwrap_or(UNNAMED_DEVICE).width())
            .max()
            .unwrap_or(0);
        let fixed = 2 + COLUMN_GAP + id + if status > 0 { COLUMN_GAP + status } else { 0 };
        let name = widest_name.min(width.saturating_sub(fixed)).max(1);
        Self { name, id }
    }

    fn row(
        &self,
        device: &crate::sync::encrypted::Device,
        short_id: &str,
        focused: bool,
    ) -> Line<'static> {
        let base = if focused {
            SELECTED
        } else {
            Style::new().fg(FG)
        };
        let dim = base.fg(FG_DIM);
        let (name, name_style) = match &device.label {
            Some(label) => (label.as_str(), base),
            None => (UNNAMED_DEVICE, dim),
        };
        let name = truncate_width(name, self.name);
        let name_pad = self.name.saturating_sub(name.width());
        let mut spans = vec![
            Span::styled(if focused { "› " } else { "  " }, base),
            Span::styled(name, name_style),
            Span::styled(" ".repeat(name_pad + COLUMN_GAP), base),
        ];
        let id = short_id.trim_end_matches('…');
        if device.current {
            spans.push(Span::styled(format!("{id:<width$}", width = self.id), dim));
            spans.push(Span::styled(" ".repeat(COLUMN_GAP), base));
            spans.push(Span::styled(THIS_DEVICE, base));
        } else {
            spans.push(Span::styled(id.to_string(), dim));
        }
        Line::from(spans)
    }
}

/// Shortens `value` to `max_width` cells by replacing its middle with an
/// ellipsis, keeping both ends recognizable.
fn elide_middle(value: &str, max_width: usize) -> String {
    let len = value.chars().count();
    if len <= max_width {
        return value.to_string();
    }
    let keep = max_width.saturating_sub(1);
    let head = keep.div_ceil(2);
    let tail = keep - head;
    let chars = value.chars().collect::<Vec<_>>();
    let mut elided = chars[..head].iter().collect::<String>();
    elided.push('…');
    elided.extend(&chars[len - tail..]);
    elided
}

fn removal_lines(lines: &mut Vec<Line<'static>>, removal: &Removal, width: usize) {
    let id = short_id(&removal.device);
    match (removal.access_revoked, removal.key_rotation_pending) {
        (true, false) => {
            lines.extend(paragraph_with_mark(
                "✓",
                GREEN,
                &format!("Removed {id} from sync"),
                width,
            ));
            lines.extend(paragraph(
                "Future changes use new keys. The removed device keeps what it already \
                 downloaded.",
                Style::new().fg(FG_MUTED),
                width,
            ));
        }
        (true, true) => {
            lines.extend(paragraph_with_mark(
                "✓",
                GREEN,
                &format!("Access removed for {id}"),
                width,
            ));
            lines.extend(paragraph_with_mark(
                "!",
                ORANGE,
                "Securing future changes is unfinished. Finish removal continues it, and \
                 any remaining device finishes it when it syncs.",
                width,
            ));
        }
        (false, _) => {
            lines.extend(paragraph_with_mark(
                "!",
                ORANGE,
                &format!("Removal of {id} isn't confirmed yet"),
                width,
            ));
            lines.extend(paragraph(
                "Resume removal to continue it.",
                Style::new().fg(FG_MUTED),
                width,
            ));
        }
    }
}

fn confirm_remove_lines(body: &mut Body, activity: &SyncActivity, device: &[u8; 32], width: usize) {
    let devices = activity
        .devices
        .as_ref()
        .map(|snapshot| snapshot.listing.devices.as_slice())
        .unwrap_or_default();
    let label = devices
        .iter()
        .position(|listed| listed.id == *device)
        .map(|index| match &devices[index].label {
            Some(label) => format!("{} ({label})", short_device_ids(devices)[index]),
            None => short_device_ids(devices)[index].clone(),
        })
        .unwrap_or_else(|| short_id(device));
    let lines = &mut body.lines;
    lines.extend(paragraph(
        &format!("Remove {label} from sync?"),
        Style::new().fg(FG),
        width,
    ));
    lines.extend(paragraph(
        "This device will lose access to future synced changes. Tasks and images it \
         already downloaded will remain on it.",
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Device ID",
        Style::new().fg(FG_DIM),
    )));
    lines.extend(paragraph(&hex::encode(device), Style::new().fg(FG), width));
}

/// Coarse age of a verified observation; never a wall-clock claim.
fn elapsed(since: Instant) -> String {
    match since.elapsed().as_secs() / 60 {
        0 => "just now".to_string(),
        1 => "1 minute ago".to_string(),
        minutes => format!("{minutes} minutes ago"),
    }
}

/// Distinguishes task synchronization from image availability; `None` when
/// both are complete.
use aven_core::sync::client::tail::ImageTransfer;

fn drain_text(drain: &DrainSummary) -> Option<&'static str> {
    if !drain.tasks_current {
        return Some("Some changes are still waiting. Sync now continues.");
    }
    Some(match drain.images {
        ImageTransfer::Complete => return None,
        ImageTransfer::Pending => {
            "Tasks are in sync. Images are still transferring; Sync now continues."
        }
        ImageTransfer::Unavailable => {
            "Tasks are in sync. Some images are unavailable on the server."
        }
        ImageTransfer::Failed => {
            "Tasks are in sync. Some image transfers failed; Sync now retries them."
        }
    })
}

fn invitation_lines(
    body: &mut Body,
    status: &TuiSyncStatus,
    kind: InvitationKind,
    input: &SecretText,
    error: Option<&'static str>,
    width: usize,
) {
    let lines = &mut body.lines;
    let guidance = match kind {
        InvitationKind::Setup if status.phase == LocalPhase::SetupIncomplete => {
            "Setup started earlier and didn't finish. Paste the same setup invitation to \
             continue it; nothing is captured again."
        }
        InvitationKind::Setup => {
            "Paste the setup invitation printed by `aven server setup` on your server."
        }
        InvitationKind::Join if status.phase == LocalPhase::JoinIncomplete => {
            "On the device that created the first invitation, use Add device or run \
             `aven sync invite` to create a new invitation, then paste it here. \
             Invitations from other devices can't be used."
        }
        InvitationKind::Join => {
            "On a device that already syncs, use Add device or run `aven sync invite`, \
             then paste the invitation here."
        }
    };
    lines.extend(paragraph(guidance, Style::new().fg(FG_MUTED), width));
    lines.push(Line::from(""));
    let (text, color) = match (kind, input.check()) {
        (_, InvitationCheck::Empty) => {
            // The same cursor-on-placeholder cell other TUI inputs draw.
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{:<LABEL_WIDTH$}", "Invitation"),
                    Style::new().fg(FG_DIM),
                ),
                cursor_cell("P"),
                Span::styled("aste the invitation here", Style::new().fg(FG_DIM)),
            ]));
            if let Some(error) = error {
                lines.extend(paragraph(error, Style::new().fg(RED), width));
            }
            return;
        }
        (InvitationKind::Setup, InvitationCheck::Setup(server))
        | (InvitationKind::Join, InvitationCheck::Device(server)) => (format!("✓ {server}"), GREEN),
        (InvitationKind::Setup, InvitationCheck::Device(_)) => (
            "Device invitation; use Join existing sync instead".to_string(),
            RED,
        ),
        (InvitationKind::Join, InvitationCheck::Setup(_)) => {
            ("Setup invitation; use Set up sync instead".to_string(), RED)
        }
        (_, InvitationCheck::Incomplete) => (
            "Incomplete invitation; part may be missing".to_string(),
            RED,
        ),
        (_, InvitationCheck::Unknown) => ("Not an Aven invitation".to_string(), RED),
    };
    lines.extend(wrapped_row(
        "Invitation",
        &text,
        Style::new().fg(color),
        width,
    ));
    if let Some(error) = error {
        lines.extend(paragraph(error, Style::new().fg(RED), width));
    }
}

fn confirm_setup_lines(body: &mut Body, server: &str, preview: &SetupPreview, width: usize) {
    let lines = &mut body.lines;
    lines.extend(wrapped_row("Server", server, Style::new().fg(FG), width));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Use this computer's data:",
        Style::new().fg(FG),
    )));
    lines.extend(indented(
        &plural(preview.workspaces as u64, "workspace"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(indented(
        &format!(
            "{} tasks (including scheduled and recurring)",
            preview.tasks.max(0)
        ),
        Style::new().fg(FG_MUTED),
        width,
    ));
    if preview.missing_images > 0 {
        lines.extend(indented(
            &format!(
                "{} missing on this computer; other devices will see {} as unavailable.",
                plural(preview.missing_images, "image"),
                if preview.missing_images == 1 {
                    "it"
                } else {
                    "them"
                }
            ),
            Style::new().fg(ORANGE),
            width,
        ));
    }
    lines.push(Line::from(""));
    lines.extend(paragraph(
        "Other devices will receive this data when they join.",
        Style::new().fg(FG_MUTED),
        width,
    ));
    if preview.leaves_unencrypted_server {
        lines.extend(paragraph(
            "This database stops using its previous unencrypted sync server.",
            Style::new().fg(FG_MUTED),
            width,
        ));
    }
}

fn confirm_join_lines(body: &mut Body, server: &str, replace: bool, width: usize) {
    let lines = &mut body.lines;
    lines.extend(wrapped_row("Server", server, Style::new().fg(FG), width));
    lines.push(Line::from(""));
    if replace {
        lines.extend(paragraph(
            "This device keeps its identity. The earlier invitation is kept too, so if \
             the other device already added this device with it, joining finishes with \
             that.",
            Style::new().fg(FG_MUTED),
            width,
        ));
        lines.push(Line::from(""));
    }
    lines.extend(paragraph(
        "This computer will download the synced tasks, then their images. Keep Add \
         device or `aven sync invite` open on the other device until joining finishes.",
        Style::new().fg(FG_MUTED),
        width,
    ));
}

fn confirm_automatic_sync_lines(
    body: &mut Body,
    service: &AutomaticSyncService,
    interval_seconds: u64,
    width: usize,
) {
    let lines = &mut body.lines;
    let muted = Style::new().fg(FG_MUTED);
    match service {
        AutomaticSyncService::Install => {
            let service_kind = if cfg!(target_os = "linux") {
                "a systemd user service, aven-daemon.service"
            } else {
                "a LaunchAgent in ~/Library/LaunchAgents"
            };
            lines.extend(paragraph(
                &format!(
                    "This turns on sync.enabled in your config and installs {service_kind}. \
                     It starts when you log in and syncs this database after each change \
                     and every {interval_seconds} seconds."
                ),
                Style::new().fg(FG),
                width,
            ));
            lines.push(Line::from(""));
            lines.extend(paragraph(
                "To undo it, run `aven daemon uninstall` and \
                 `aven config set sync.enabled false`.",
                muted,
                width,
            ));
        }
        AutomaticSyncService::OtherDatabase(path) => {
            lines.extend(paragraph(
                "This turns on sync.enabled in your config. It installs nothing, because \
                 the background service syncs your default database and this is another \
                 one.",
                Style::new().fg(FG),
                width,
            ));
            lines.push(Line::from(""));
            lines.extend(paragraph(
                &format!(
                    "To sync this database in the background instead, run `{}`.",
                    crate::tui::overlay::daemon_install_command(path)
                ),
                muted,
                width,
            ));
        }
        AutomaticSyncService::Unsupported => {
            lines.extend(paragraph(
                "This turns on sync.enabled in your config. It installs nothing, because \
                 background services aren't supported on this platform.",
                Style::new().fg(FG),
                width,
            ));
            lines.push(Line::from(""));
            lines.extend(paragraph(
                "Keep `aven daemon` running to sync in the background.",
                muted,
                width,
            ));
        }
    }
}

fn plural(count: u64, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

fn push_buttons(body: &mut Body, actions: &[SyncAction], selected: usize, width: usize) {
    let labels = actions
        .iter()
        .map(|action| format!(" {} ", action.label()))
        .collect::<Vec<_>>();
    let total = labels.iter().map(|label| label.width() as u16).sum::<u16>()
        + labels.len().saturating_sub(1) as u16;
    let mut column = (width as u16).saturating_sub(total);
    let line = body.lines.len();
    let mut spans = vec![Span::raw(" ".repeat(column as usize))];
    for (index, label) in labels.into_iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw(" "));
            column += 1;
        }
        let end = column + label.width() as u16;
        body.actions.push(ActionArea {
            line,
            start: column,
            end,
        });
        spans.push(button_span(
            label,
            index == selected,
            index + 1 == actions.len(),
        ));
        column = end;
    }
    body.lines.push(Line::from(spans));
}

fn button_span(label: String, focused: bool, primary: bool) -> Span<'static> {
    let fill = if focused { ACCENT } else { BG_PANEL };
    let foreground = if focused {
        INVERSE_FG
    } else if primary {
        ACCENT
    } else {
        FG_MUTED
    };
    let mut style = Style::new().fg(foreground).bg(fill);
    if focused {
        style = style.add_modifier(Modifier::BOLD);
    }
    Span::styled(label, style)
}

fn paragraph(text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    wrap_with_code(text, style, width)
}

/// A list item: every wrapped line keeps the same two-cell indent.
fn indented(text: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    let mut lines = paragraph(text, style, width.saturating_sub(2));
    for line in &mut lines {
        line.spans.insert(0, Span::raw("  "));
    }
    lines
}

fn paragraph_with_mark(mark: &str, color: Color, text: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines = paragraph(
        text,
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
        width.saturating_sub(2),
    );
    for (index, line) in lines.iter_mut().enumerate() {
        let prefix = if index == 0 {
            Span::styled(format!("{mark} "), Style::new().fg(color))
        } else {
            Span::raw("  ")
        };
        line.spans.insert(0, prefix);
    }
    lines
}

fn action_line(label: &'static str, focused: bool) -> Line<'static> {
    if focused {
        Line::from(Span::styled(format!("› {label}"), SELECTED))
    } else {
        Line::from(Span::styled(format!("  {label}"), Style::new().fg(FG)))
    }
}

/// Explains the status headline; a set-up database needs no explanation.
fn state_line(status: &TuiSyncStatus, health: SyncHealth) -> Option<&'static str> {
    Some(match (health, status.phase) {
        (SyncHealth::AccessRefused, _) => "Access unconfirmed",
        (SyncHealth::RuntimeDisabled, _) => "Sync is disabled by the runtime override",
        (_, LocalPhase::NotSetUp) => "This database is local only",
        (_, LocalPhase::SetupIncomplete) => "Setup started here and didn't finish",
        (_, LocalPhase::JoinIncomplete) => "Joining started here and didn't finish",
        (_, LocalPhase::SetUp) => return None,
    })
}

fn detail_lines(status: &TuiSyncStatus, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if status.set_up && status.enabled {
        lines.extend(wrapped_row(
            "Interval",
            &format!("{} seconds", status.interval_seconds),
            Style::new().fg(FG_MUTED),
            width,
        ));
        let wake_style = Style::new().fg(if status.daemon_wake.ok { FG_MUTED } else { RED });
        lines.extend(wrapped_row(
            "Wake address",
            &status.daemon_wake.value,
            wake_style,
            width,
        ));
    }
    lines.extend(wrapped_row(
        "Sync cursor",
        status.sync_cursor.as_deref().unwrap_or("missing"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    lines.extend(wrapped_row(
        "Local sequence",
        status.local_sequence.as_deref().unwrap_or("missing"),
        Style::new().fg(FG_MUTED),
        width,
    ));
    if let Some(at) = &status.access_refused_at {
        lines.extend(wrapped_row(
            "Access refused",
            at,
            Style::new().fg(RED),
            width,
        ));
    }
    lines
}

fn wrapped_row(label: &str, value: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    let value_width = width.saturating_sub(LABEL_WIDTH).max(1);
    let mut lines = wrap_with_code(value, style, value_width);
    for (index, line) in lines.iter_mut().enumerate() {
        let label = if index == 0 { label } else { "" };
        line.spans.insert(
            0,
            Span::styled(format!("{label:<LABEL_WIDTH$}"), Style::new().fg(FG_DIM)),
        );
    }
    lines
}

fn hint_line(view: &SyncDialogView<'_>, scrolling: bool) -> Line<'static> {
    let mut hints = Vec::new();
    let actions = sync_actions(view.state, view.status, view.activity);
    match view.state.page {
        SyncPage::Home => {
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
        }
        SyncPage::Invitation { .. } => {
            hints.push(("←→", "select"));
            hints.push(("Enter", "choose"));
            hints.push(("Ctrl-U", "clear"));
            hints.push(("Esc", "back"));
        }
        SyncPage::Devices => {
            hints.push(("↑↓", "select"));
            let devices = view
                .activity
                .devices
                .as_ref()
                .map(|snapshot| snapshot.listing.devices.as_slice())
                .unwrap_or_default();
            match actions.get(view.state.selected) {
                Some(SyncAction::Device(index)) => {
                    if devices.get(*index).is_some_and(|device| !device.current) {
                        hints.push(("Enter", "remove"));
                    }
                }
                Some(_) => hints.push(("Enter", "choose")),
                None => {}
            }
            if matches!(
                actions.get(view.state.selected),
                Some(SyncAction::Device(_))
            ) {
                hints.push(("y", "copy ID"));
            }
            hints.push(("r", "refresh"));
            hints.push(("Esc", "back"));
        }
        SyncPage::ConfirmSetup { .. }
        | SyncPage::ConfirmJoin { .. }
        | SyncPage::ConfirmRemove { .. }
        | SyncPage::ConfirmAutomaticSync { .. } => {
            hints.push(("←→", "select"));
            hints.push(("Enter", "choose"));
            hints.push(("Esc", "back"));
        }
    }
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

#[cfg(test)]
pub(in crate::tui::ui) fn sync_dialog_lines_for_test_width(
    view: &SyncDialogView<'_>,
    width: usize,
) -> Vec<Line<'static>> {
    body(view, width).lines
}
