use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Rect, Size};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};
use unicode_width::UnicodeWidthStr;

use super::super::dialog::{Dialog, dialog_hint_line};
use super::super::scroll::{clamp_scroll_start, render_vertical_scrollbar};
use super::super::sync_status_model::{SyncHealth, sync_status_summary};

use crate::sync::encrypted::Removal;
use crate::sync::encrypted::{LocalPhase, SetupPreview, Stage};
use crate::tui::overlay::{
    InvitationKind, SecretText, SyncAction, SyncDialogView, SyncPage, dialog_area, sync_actions,
};
use crate::tui::store::TuiSyncStatus;
use crate::tui::sync_errors::JOIN_TIMEOUT_EXPIRED;
use crate::tui::sync_operations::{
    DrainSummary, OperationFailure, OperationKind, OperationResult, RunningOperation, SyncActivity,
    short_device_ids,
};
use crate::tui::text::cell_width_ranges;
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
                "last error",
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
        lines.push(Line::from(Span::styled(
            "Local only",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        )));
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
    let color = summary.color();
    lines.push(Line::from(vec![
        Span::styled("● ", Style::new().fg(color)),
        Span::styled(
            summary.headline(),
            Style::new().fg(color).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        state_line(status, summary.health, view.syncing),
        Style::new().fg(FG_MUTED),
    )));
    if status.phase == LocalPhase::SetupRecoveryRequired {
        lines.extend(paragraph(
            "Local editing and export still work. Back up this database, then restore it to a new path for a local-only copy.",
            Style::new().fg(FG),
            width,
        ));
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
    if let Some(invitation) = status.invitation {
        lines.extend(wrapped_row(
            "invitation",
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

fn progress_lines(lines: &mut Vec<Line<'static>>, running: &RunningOperation, width: usize) {
    let heading = match running.kind {
        OperationKind::Setup => "Setting up sync",
        OperationKind::Join => "Joining sync",
        OperationKind::Sync
        | OperationKind::ListDevices
        | OperationKind::RemoveDevice(_)
        | OperationKind::FinishRemoval => {
            lines.push(spinner_line(running));
            return;
        }
    };
    lines.push(Line::from(Span::styled(
        heading,
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    )));
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

fn spinner(started_at: Instant) -> &'static str {
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
    lines.push(Line::from(""));
    match result {
        OperationResult::SetUp { server, drain } | OperationResult::Joined { server, drain } => {
            let headline = match result {
                OperationResult::SetUp { .. } => format!("Sync is set up with {server}"),
                _ => format!("Joined sync with {server}"),
            };
            lines.extend(paragraph_with_mark("✓", GREEN, &headline, width));
            lines.extend(paragraph(
                drain_text(drain),
                Style::new().fg(FG_MUTED),
                width,
            ));
        }
        OperationResult::Removed(_) | OperationResult::RemovalFinished { .. } => {}
        OperationResult::Failed(failure) => failure_lines(lines, failure, width),
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
    lines.push(Line::from(Span::styled(
        "Devices",
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    )));
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
    let mut selected_device = None;
    for (index, action) in actions.iter().enumerate() {
        let focused = index == view.state.selected;
        let row = match action {
            SyncAction::Device(device_index) => {
                let device = &devices[*device_index];
                if focused {
                    selected_device = Some(device);
                }
                let identity = match &device.label {
                    Some(label) => format!("{}  {label}", short_ids[*device_index]),
                    None => short_ids[*device_index].clone(),
                };
                let row = if device.current {
                    format!("{identity:<32}This device")
                } else {
                    identity
                };
                action_line_owned(row, focused)
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
        lines.push(Line::from(Span::styled(
            "Device ID",
            Style::new().fg(FG_DIM),
        )));
        lines.extend(paragraph(
            &hex::encode(device.id),
            Style::new().fg(FG),
            width,
        ));
        lines.extend(paragraph(
            if device.current {
                "This device can be removed only from another device in sync."
            } else {
                "Enter removes this device from sync."
            },
            Style::new().fg(FG_MUTED),
            width,
        ));
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
    lines.push(Line::from(Span::styled(
        format!("Remove device {label}?"),
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    )));
    lines.push(Line::from(""));
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

fn action_line_owned(label: String, focused: bool) -> Line<'static> {
    if focused {
        Line::from(Span::styled(format!("› {label}"), SELECTED))
    } else {
        Line::from(Span::styled(format!("  {label}"), Style::new().fg(FG)))
    }
}

/// Distinguishes task synchronization from image availability.
fn drain_text(drain: &DrainSummary) -> &'static str {
    if !drain.tasks_current {
        return "Some changes are still waiting. Sync now continues.";
    }
    match drain.images {
        "complete" => "Tasks and images are in sync.",
        "pending" => "Tasks are in sync. Images are still transferring; Sync now continues.",
        "unavailable" => "Tasks are in sync. Some images are unavailable on the server.",
        _ => "Tasks are in sync. Some image transfers failed; Sync now retries them.",
    }
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
    let (heading, guidance) = match kind {
        InvitationKind::Setup if status.phase == LocalPhase::SetupIncomplete => (
            "Resume setup",
            "Setup started earlier and didn't finish. Paste the same setup invitation to \
             continue it; nothing is captured again.",
        ),
        InvitationKind::Setup => (
            "Set up sync",
            "Paste the setup invitation printed by `aven server setup` on your server.",
        ),
        InvitationKind::Join if status.phase == LocalPhase::JoinIncomplete => (
            "Use a new invitation",
            "On the device that created the first invitation, use Add device or run \
             `aven sync invite` to create a new invitation, then paste it here. \
             Invitations from other devices can't be used.",
        ),
        InvitationKind::Join => (
            "Join existing sync",
            "On a device that already syncs, use Add device or run `aven sync invite`, \
             then paste the invitation here.",
        ),
    };
    lines.push(Line::from(Span::styled(
        heading,
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    )));
    lines.extend(paragraph(guidance, Style::new().fg(FG_MUTED), width));
    lines.push(Line::from(""));
    let field = if input.chars() == 0 {
        Span::styled("paste the invitation", Style::new().fg(FG_DIM))
    } else {
        Span::styled(
            format!("{} pasted", plural(input.chars() as u64, "character")),
            Style::new().fg(FG),
        )
    };
    lines.push(Line::from(vec![
        Span::styled(
            format!("{:<LABEL_WIDTH$}", "invitation"),
            Style::new().fg(FG_DIM),
        ),
        field,
    ]));
    if let Some(error) = error {
        lines.extend(paragraph(error, Style::new().fg(RED), width));
    }
    lines.push(Line::from(""));
    lines.extend(paragraph(
        "The invitation is secret. It isn't shown or saved.",
        Style::new().fg(FG_DIM),
        width,
    ));
}

fn confirm_setup_lines(body: &mut Body, server: &str, preview: &SetupPreview, width: usize) {
    let lines = &mut body.lines;
    lines.push(Line::from(Span::styled(
        "Set up sync",
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    )));
    lines.extend(wrapped_row("server", server, Style::new().fg(FG), width));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Use this computer's data:",
        Style::new().fg(FG),
    )));
    lines.push(Line::from(Span::styled(
        format!("  {}", plural(preview.workspaces as u64, "workspace")),
        Style::new().fg(FG_MUTED),
    )));
    lines.extend(paragraph(
        &format!(
            "  {} non-deleted task records, including scheduled and recurring occurrences",
            preview.tasks.max(0)
        ),
        Style::new().fg(FG_MUTED),
        width,
    ));
    if preview.missing_images > 0 {
        lines.extend(paragraph(
            &format!(
                "  {} missing on this computer; other devices will see {} as \
                 unavailable.",
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
        "Other devices will receive this data when they join. After setup, this \
         database can no longer use backup restore or import.",
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
    lines.push(Line::from(Span::styled(
        if replace {
            "Continue joining with a new invitation"
        } else {
            "Join existing sync"
        },
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    )));
    lines.extend(wrapped_row("server", server, Style::new().fg(FG), width));
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
    wrap_words(text, width.max(1))
        .into_iter()
        .map(|line| Line::from(Span::styled(line, style)))
        .collect()
}

/// Wraps at spaces, splitting only words wider than the line.
fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let needed = if current.is_empty() {
            word.width()
        } else {
            current.width() + 1 + word.width()
        };
        if needed > width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        if word.width() > width {
            for (start, end) in cell_width_ranges(word, width) {
                if !current.is_empty() {
                    lines.push(std::mem::take(&mut current));
                }
                current.push_str(&word[start..end]);
            }
            continue;
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
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

fn attention_style(attention: bool) -> Style {
    Style::new().fg(if attention { ORANGE } else { FG_MUTED })
}

fn state_line(status: &TuiSyncStatus, health: SyncHealth, syncing: bool) -> &'static str {
    if syncing {
        return "Syncing now";
    }
    match (health, status.phase) {
        (SyncHealth::AccessRefused, _) => "Access unconfirmed",
        (SyncHealth::RuntimeDisabled, _) => "Sync is disabled by the runtime override",
        (_, LocalPhase::NotSetUp) => "This database is local only",
        (_, LocalPhase::SetupIncomplete) => "Setup started here and didn't finish",
        (_, LocalPhase::SetupRecoveryRequired) => {
            "This refused setup needs backup and restore to a new path"
        }
        (_, LocalPhase::JoinIncomplete) => "Joining started here and didn't finish",
        (_, LocalPhase::SetUp) => "Sync is end-to-end encrypted",
    }
}

fn detail_lines(status: &TuiSyncStatus, width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if status.set_up && status.enabled {
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
    }
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
    if let Some(at) = &status.access_refused_at {
        lines.extend(wrapped_row(
            "access refused",
            at,
            Style::new().fg(RED),
            width,
        ));
    }
    lines
}

fn wrapped_row(label: &str, value: &str, style: Style, width: usize) -> Vec<Line<'static>> {
    let value_width = width.saturating_sub(LABEL_WIDTH).max(1);
    wrap_words(value, value_width)
        .into_iter()
        .enumerate()
        .map(|(index, value)| {
            let label = if index == 0 { label } else { "" };
            Line::from(vec![
                Span::styled(format!("{label:<LABEL_WIDTH$}"), Style::new().fg(FG_DIM)),
                Span::styled(value, style),
            ])
        })
        .collect()
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
            hints.push(("Enter", "continue"));
            hints.push(("Ctrl-U", "clear"));
            hints.push(("Esc", "back"));
        }
        SyncPage::Devices => {
            hints.push(("↑↓", "select"));
            if actions
                .get(view.state.selected)
                .is_some_and(|action| !matches!(action, SyncAction::Device(_)))
            {
                hints.push(("Enter", "choose"));
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
        | SyncPage::ConfirmRemove { .. } => {
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
#[test]
fn words_wrap_at_spaces_and_split_only_long_words() {
    assert_eq!(
        wrap_words("backup restore or import", 10),
        ["backup", "restore or", "import"]
    );
    assert_eq!(wrap_words("abcdefghij", 4), ["abcd", "efgh", "ij"]);
    assert_eq!(wrap_words("", 4), [""]);
}

#[cfg(test)]
pub(in crate::tui::ui) fn sync_dialog_lines_for_test(
    view: &SyncDialogView<'_>,
) -> Vec<Line<'static>> {
    body(view, 60).lines
}
