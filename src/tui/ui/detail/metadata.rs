use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};

use super::super::task_display::labels_display;
use super::super::timestamps::local_timestamp_display;
use crate::query::TaskListItem;
use crate::tui::text::truncate_width;
use crate::tui::theme::{ACCENT, BG, BORDER, FG, FG_DIM, FG_MUTED, INVERSE_FG, ORANGE, RED};
use crate::tui::widgets::{priority_short, status_chip};

use super::DetailCopyHit;
use super::document::{detail_body_area, detail_content_layout};
use super::relationships::{DetailEpicChild, epic_child_counts};
use crate::tui::theme;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailMetadataTarget {
    Project,
    Status,
    Priority,
    Labels,
    Availability,
    Due,
}

#[derive(Debug)]
struct DetailMetadataRows {
    availability: std::ops::Range<u16>,
    due: std::ops::Range<u16>,
    reference: u16,
    created: u16,
    updated: u16,
}
pub(super) fn render_detail_metadata(
    frame: &mut Frame,
    item: &TaskListItem,
    epic_children: &[DetailEpicChild],
    area: Rect,
) {
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_style(Style::new().fg(BORDER))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::from(detail_metadata_lines_with_children(
            item,
            epic_children,
            inner.width as usize,
        )))
        .style(Style::new().fg(FG).bg(BG)),
        inner,
    );
}

pub(super) fn detail_metadata_lines_with_children(
    item: &TaskListItem,
    epic_children: &[DetailEpicChild],
    width: usize,
) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            if item.task.is_epic {
                " EPIC "
            } else {
                " TASK "
            },
            Style::new()
                .fg(INVERSE_FG)
                .bg(BORDER)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metadata_label("PROJECT"),
        Line::from(vec![
            Span::styled(
                "● ",
                Style::new().fg(theme::project_color(&item.task.project_key)),
            ),
            Span::styled(
                item.task.project_key.clone(),
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        metadata_label("STATUS"),
        status_chip(item.task.status.as_str()),
        Line::from(""),
        metadata_label("PRIORITY"),
        Line::from(Span::styled(
            priority_short(item.task.priority.as_str()),
            theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metadata_label("LABELS"),
        Line::from(labels_display(&item.labels, ", ")),
    ];
    let now_seconds = crate::queue::now_seconds();
    let availability = crate::tui::time::availability_summary_lines(
        item.task.available_at.as_deref().unwrap_or(""),
        item.queue.band == crate::queue::QueueBand::Available,
        now_seconds,
    );
    let availability_style = if availability.is_some() {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_MUTED)
    };
    lines.extend([Line::from(""), metadata_label("AVAILABILITY")]);
    for value in availability
        .map(Vec::from)
        .unwrap_or_else(|| vec!["none".to_string()])
    {
        lines.push(Line::from(Span::styled(
            truncate_width(&value, width),
            availability_style,
        )));
    }
    let due =
        crate::tui::time::due_summary_lines(item.task.due_on.as_deref().unwrap_or(""), now_seconds);
    let due_color = if item.task.due_on.is_none() || !item.task.status.is_open() {
        FG_MUTED
    } else {
        match crate::tui::time::due_state_at(item.task.due_on.as_deref().unwrap_or(""), now_seconds)
        {
            crate::due::DueState::Overdue(_) => RED,
            crate::due::DueState::Today => ORANGE,
            crate::due::DueState::Future(_) => ACCENT,
            crate::due::DueState::None => FG_MUTED,
        }
    };
    let due_style = Style::new().fg(due_color).add_modifier(Modifier::BOLD);
    lines.extend([Line::from(""), metadata_label("DUE")]);
    for value in due
        .map(Vec::from)
        .unwrap_or_else(|| vec!["none".to_string()])
    {
        lines.push(Line::from(Span::styled(
            truncate_width(&value, width),
            due_style,
        )));
    }
    lines.extend([
        Line::from(""),
        metadata_label("REF"),
        Line::from(Span::styled(
            item.display_ref.clone(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        metadata_label("CREATED"),
        Line::from(Span::styled(
            local_timestamp_display(&item.task.created_at),
            Style::new().fg(FG_MUTED),
        )),
        Line::from(""),
        metadata_label("UPDATED"),
        Line::from(Span::styled(
            local_timestamp_display(&item.task.updated_at),
            Style::new().fg(FG_MUTED),
        )),
    ]);
    if let Some(recurrence) = item.recurrence.as_ref() {
        let outcome = recurrence
            .outcome
            .map(|value| value.as_str())
            .unwrap_or("open");
        lines.extend([
            Line::from(""),
            metadata_label("RECURRENCE"),
            Line::from(Span::styled(
                format!("↻ {}", recurrence.series_ref),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            )),
            Line::from(format!("schedule {}", recurrence.rule_label)),
            Line::from(format!("slot {}", recurrence.slot_on)),
            Line::from(format!("zone {}", recurrence.timezone)),
            Line::from(format!("lifecycle {}", recurrence.lifecycle.as_str())),
            Line::from(format!("outcome {outcome}")),
            Line::from(format!(
                "projection {}",
                recurrence.projection_state.as_str()
            )),
            Line::from(Span::styled("history t r h", Style::new().fg(FG_MUTED))),
        ]);
    }
    if let Some(group) = item.recurrence_group.as_ref() {
        lines.extend([
            Line::from(""),
            metadata_label("SERIES HISTORY"),
            Line::from(format!("completed {}", group.counts.completed)),
            Line::from(format!("skipped {}", group.counts.skipped)),
            Line::from(format!("missed {}", group.counts.missed)),
        ]);
    }
    lines.extend(detail_epic_metadata_lines(item, epic_children));
    if item.has_conflict {
        lines.extend([Line::from(""), metadata_label("CONFLICTS")]);
        if item.conflicts.is_empty() {
            lines.push(Line::from(Span::styled(
                "open · c s details · c a this · c r other · c m manual",
                Style::new().fg(ORANGE).add_modifier(Modifier::BOLD),
            )));
        } else {
            for conflict in &item.conflicts {
                lines.push(Line::from(Span::styled(
                    conflict.field.clone(),
                    Style::new().fg(ORANGE).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(format!("this device  {}", conflict.local_value)));
                lines.push(Line::from(format!(
                    "other device {}",
                    conflict.remote_value
                )));
                lines.push(Line::from(Span::styled(
                    "c a this · c r other · c m manual",
                    Style::new().fg(FG_MUTED),
                )));
            }
        }
    }
    if item.task.deleted {
        lines.extend([
            Line::from(""),
            metadata_label("DELETED"),
            Line::from(Span::styled(
                "yes",
                Style::new().fg(RED).add_modifier(Modifier::BOLD),
            )),
        ]);
    }
    lines
}

fn detail_epic_metadata_lines(
    item: &TaskListItem,
    children: &[DetailEpicChild],
) -> Vec<Line<'static>> {
    if !item.task.is_epic {
        return Vec::new();
    }

    let counts = epic_child_counts(children);
    let progress = item.epic_rollup.as_ref().map_or_else(
        || format!("open={} total={}", counts.open, counts.total),
        |rollup| {
            format!(
                "{} open · {} done · {} canceled",
                rollup.open, rollup.done, rollup.canceled
            )
        },
    );
    let mut lines = vec![
        Line::from(""),
        metadata_label("CHILDREN"),
        Line::from(Span::styled(progress, Style::new().fg(FG_DIM))),
    ];
    if let Some(rollup) = item.epic_rollup.as_ref()
        && rollup.total > 0
    {
        lines.push(Line::from(Span::styled(
            format!(
                "{} overdue · {} blocked · {} ready",
                rollup.overdue, rollup.blocked, rollup.ready
            ),
            Style::new().fg(FG_DIM),
        )));
    }

    if children.is_empty() {
        return lines
            .into_iter()
            .chain(std::iter::once(Line::from(Span::styled(
                "none",
                Style::new().fg(FG_MUTED),
            ))))
            .collect();
    }

    lines
}

pub(super) fn metadata_label(label: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        label,
        Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
    ))
}

pub(crate) fn detail_copy_target_at(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    column: u16,
    row: u16,
) -> Option<DetailCopyHit> {
    let layout = detail_content_layout(Rect::new(0, 0, terminal_width, terminal_height));
    let header_ref_row = layout.content_area.y.saturating_add(2);
    if row == header_ref_row
        && column >= layout.content_area.x
        && column
            < layout
                .content_area
                .x
                .saturating_add(UnicodeWidthStr::width(item.display_ref.as_str()) as u16)
    {
        return Some(DetailCopyHit {
            value: item.display_ref.clone(),
        });
    }

    if layout.metadata_area.width == 0 {
        return None;
    }
    let body = detail_body_area(Rect::new(0, 0, terminal_width, terminal_height));
    let line = metadata_content_row(layout.metadata_area, body, column, row)?;
    let rows = detail_metadata_rows(item);
    let value = match line {
        value if value == rows.reference => item.display_ref.clone(),
        value if value == rows.created => local_timestamp_display(&item.task.created_at),
        value if value == rows.updated => local_timestamp_display(&item.task.updated_at),
        _ => return None,
    };
    let value_start = layout.metadata_area.x.saturating_add(2);
    let value_end = value_start.saturating_add(UnicodeWidthStr::width(value.as_str()) as u16);
    (column >= value_start && column < value_end).then_some(DetailCopyHit { value })
}

pub(crate) fn detail_metadata_target_at(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    column: u16,
    row: u16,
) -> Option<(DetailMetadataTarget, u16, u16)> {
    let layout = detail_content_layout(Rect::new(0, 0, terminal_width, terminal_height));
    if layout.metadata_area.width == 0 {
        return None;
    }
    let body = detail_body_area(Rect::new(0, 0, terminal_width, terminal_height));
    let line = metadata_content_row(layout.metadata_area, body, column, row)?;
    let rows = detail_metadata_rows(item);
    let target = match line {
        3 => DetailMetadataTarget::Project,
        6 => DetailMetadataTarget::Status,
        9 => DetailMetadataTarget::Priority,
        12 => DetailMetadataTarget::Labels,
        value if rows.availability.contains(&value) => DetailMetadataTarget::Availability,
        value if rows.due.contains(&value) => DetailMetadataTarget::Due,
        _ => return None,
    };
    Some((target, column, row))
}

fn detail_metadata_rows(item: &TaskListItem) -> DetailMetadataRows {
    let now_seconds = crate::queue::now_seconds();
    let availability_len = u16::from(
        crate::tui::time::availability_summary_lines(
            item.task.available_at.as_deref().unwrap_or(""),
            item.queue.band == crate::queue::QueueBand::Available,
            now_seconds,
        )
        .is_some(),
    ) + 1;
    let due_len = u16::from(
        crate::tui::time::due_summary_lines(item.task.due_on.as_deref().unwrap_or(""), now_seconds)
            .is_some(),
    ) + 1;
    let availability_start = 15;
    let due_start = availability_start + availability_len + 2;
    let reference = due_start + due_len + 2;

    DetailMetadataRows {
        availability: availability_start..availability_start + availability_len,
        due: due_start..due_start + due_len,
        reference,
        created: reference + 3,
        updated: reference + 6,
    }
}

pub(super) fn metadata_content_row(
    metadata_area: Rect,
    body: Rect,
    column: u16,
    row: u16,
) -> Option<u16> {
    if column <= metadata_area.x
        || column >= metadata_area.x.saturating_add(metadata_area.width)
        || row < body.y
        || row >= body.y.saturating_add(body.height)
    {
        return None;
    }
    Some(row.saturating_sub(body.y))
}
