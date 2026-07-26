use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, TableState, Wrap};

use crate::query::{RecurrenceSeriesDetail, RecurrenceSeriesListItem};
use crate::tui::app::Focus;
use crate::tui::list_surface::ListSurface;
use crate::tui::store::TuiStore;
use crate::tui::theme::{ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, SELECTED, SELECTED_INACTIVE};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecurrenceSeriesHit {
    pub(crate) series_index: usize,
    pub(crate) series_id: aven_core::recurrence::RecurrenceSeriesId,
    pub(crate) viewport_row: u16,
}

pub(crate) fn recurrence_series_at_position(
    store: &TuiStore,
    table_state: &TableState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<RecurrenceSeriesHit> {
    if column < area.x
        || column >= area.x.saturating_add(area.width)
        || row <= area.y
        || row >= area.y.saturating_add(area.height)
    {
        return None;
    }
    let viewport_row = row.saturating_sub(area.y).saturating_sub(1);
    let series_index = table_state.offset().saturating_add(viewport_row as usize);
    let item = store.recurrence_series.get(series_index)?;
    Some(RecurrenceSeriesHit {
        series_index,
        series_id: item.series.id.clone(),
        viewport_row,
    })
}

pub(super) fn render_recurrence_series(
    frame: &mut Frame,
    store: &TuiStore,
    list: &mut ListSurface,
    focus: Focus,
    area: Rect,
) {
    frame.render_widget(Block::new().style(Style::new().bg(BG)), area);
    if area.height == 0 || area.width == 0 {
        return;
    }
    let rows = Layout::vertical(vec![Constraint::Length(1); area.height as usize]).split(area);
    if rows.is_empty() {
        return;
    }
    render_header(frame, rows[0]);
    if store.recurrence_series.is_empty() {
        render_empty(frame, store, area);
        return;
    }
    let viewport_rows = rows.len().saturating_sub(1);
    if viewport_rows == 0 {
        return;
    }
    let selected = list
        .selected_task()
        .unwrap_or(0)
        .min(store.recurrence_series.len() - 1);
    list.select_task(Some(selected));
    let max_scroll = store.recurrence_series.len().saturating_sub(viewport_rows);
    let mut scroll = list.task_offset().min(max_scroll);
    if selected < scroll {
        scroll = selected;
    } else if selected >= scroll.saturating_add(viewport_rows) {
        scroll = selected.saturating_add(1).saturating_sub(viewport_rows);
    }
    list.set_task_offset(scroll);
    for (index, item) in store
        .recurrence_series
        .iter()
        .enumerate()
        .skip(scroll)
        .take(viewport_rows)
    {
        render_row(
            frame,
            item,
            rows[index - scroll + 1],
            selected == index,
            focus == Focus::Tasks,
        );
    }
}

fn render_header(frame: &mut Frame, area: Rect) {
    let style = Style::new()
        .fg(FG_DIM)
        .bg(BG_ALT)
        .add_modifier(Modifier::BOLD);
    let cells = Layout::horizontal(columns()).areas::<5>(area);
    for (cell, label) in
        cells
            .into_iter()
            .zip([" TITLE", " REF", " SCHEDULE", " OCCURRENCE", " STATE"])
    {
        frame.render_widget(Paragraph::new(label).style(style), cell);
    }
}

fn render_row(
    frame: &mut Frame,
    item: &RecurrenceSeriesListItem,
    area: Rect,
    selected: bool,
    focused: bool,
) {
    let style = if selected {
        if focused { SELECTED } else { SELECTED_INACTIVE }
    } else {
        Style::new().fg(FG).bg(BG)
    };
    frame.render_widget(Block::new().style(style), area);
    let occurrence = item
        .current_occurrence
        .as_ref()
        .map(|occurrence| format!("{} {}", occurrence.slot_on, occurrence.task_ref))
        .unwrap_or_else(|| "-".to_string());
    let schedule = crate::recurrence_input::natural_rule_label(item.series.rule);
    let values = [
        item.series.title.as_str(),
        item.series_ref.as_str(),
        schedule.as_str(),
        occurrence.as_str(),
        item.series.state.as_str(),
    ];
    for (cell, value) in Layout::horizontal(columns())
        .areas::<5>(area)
        .into_iter()
        .zip(values)
    {
        frame.render_widget(Paragraph::new(format!(" {value}")).style(style), cell);
    }
}

fn columns() -> [Constraint; 5] {
    [
        Constraint::Percentage(30),
        Constraint::Length(12),
        Constraint::Percentage(28),
        Constraint::Percentage(24),
        Constraint::Length(10),
    ]
}

fn render_empty(frame: &mut Frame, store: &TuiStore, area: Rect) {
    let searched = store.view_state.recurring.search.is_some();
    let title = if searched {
        "No recurring series match this search"
    } else {
        match store.view_state.recurring.lifecycle {
            crate::query::RecurrenceSeriesLifecycleFilter::Stopped => "No stopped recurring series",
            _ => "No recurring series in this scope",
        }
    };
    let content = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    };
    frame.render_widget(
        Paragraph::new(Text::from(vec![
            Line::from(""),
            Line::from(vec![Span::styled(
                format!("  {title}"),
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            )]),
            Line::from(vec![Span::styled(
                "  Use f r to change the lifecycle filter.",
                Style::new().fg(FG_DIM),
            )]),
        ]))
        .style(Style::new().bg(BG)),
        content,
    );
}

pub(super) fn render_recurrence_detail(
    frame: &mut Frame,
    detail: &RecurrenceSeriesDetail,
    scroll: u16,
) {
    let current = detail
        .current_occurrence
        .as_ref()
        .and_then(|occurrence| occurrence.task_id.as_ref())
        .map(|_| {
            format!(
                "{} {}",
                detail
                    .summary
                    .current_slot_on
                    .as_deref()
                    .unwrap_or("unknown date"),
                detail
                    .summary
                    .current_task_ref
                    .as_deref()
                    .unwrap_or("unknown task")
            )
        });
    let starts = detail.series.start_on.format("%b %-d");
    let available = detail
        .series
        .available_local_time
        .map(|time| time.format("%H:%M").to_string())
        .unwrap_or_else(|| "Start of day".to_string());
    let due = match detail.series.due_policy {
        aven_core::recurrence::RecurrenceDuePolicy::SameDay => "Same day",
        aven_core::recurrence::RecurrenceDuePolicy::None => "None",
    };
    let mut lines = vec![
        Line::from(vec![Span::styled(
            detail.series.title.clone(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        )]),
        Line::from(format!(
            "{} · {}",
            detail.summary.series_ref,
            detail.series.state.as_str()
        )),
        Line::from(""),
        Line::from(format!(
            "Repeat       {}",
            crate::recurrence_input::natural_rule_label(detail.series.rule)
        )),
        Line::from(format!("Available    {available}")),
        Line::from(format!("Due          {due}")),
        Line::from(format!("Starts       {starts}")),
    ];
    if detail.series.priority.as_str() != "none" {
        lines.push(Line::from(format!(
            "Priority     {}",
            detail.series.priority.as_str()
        )));
    }
    if !detail.labels.is_empty() {
        lines.push(Line::from(format!(
            "Labels       {}",
            detail.labels.join(", ")
        )));
    }
    if let Some(current) = current.as_deref() {
        lines.push(Line::from(format!("Current task {current}")));
    }
    if !detail.series.description.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(detail.series.description.clone()));
    }
    if !detail.lifecycle_conflicts.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(vec![Span::styled(
            "Lifecycle conflicts",
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        )]));
        lines.extend(detail.lifecycle_conflicts.iter().map(|conflict| {
            Line::from(format!(
                "{}: {} / {}",
                conflict.field, conflict.local_value, conflict.remote_value
            ))
        }));
    }
    lines.push(Line::from(""));
    let mut actions = Vec::new();
    if current.is_some() {
        actions.push("Enter Open task");
    }
    actions.push("e Edit");
    match detail.series.state {
        aven_core::recurrence::RecurrenceSeriesState::Active => actions.push("p Pause"),
        aven_core::recurrence::RecurrenceSeriesState::Paused => actions.push("p Resume"),
        aven_core::recurrence::RecurrenceSeriesState::Stopped => {}
    }
    actions.push("h History");
    if detail.series.state != aven_core::recurrence::RecurrenceSeriesState::Stopped {
        actions.push("s Stop");
    }
    actions.push("Esc Close");
    lines.push(Line::from(vec![Span::styled(
        actions.join("  "),
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
    )]));

    let area = frame.area();
    let width = area.width.saturating_sub(4).min(76);
    let height = (lines.len() as u16 + 2).min(area.height.saturating_sub(2));
    let dialog = Rect::new(
        area.x
            .saturating_add((area.width.saturating_sub(width)) / 2),
        area.y
            .saturating_add((area.height.saturating_sub(height)) / 2),
        width,
        height,
    );
    frame.render_widget(Clear, dialog);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .block(
                Block::new()
                    .borders(Borders::ALL)
                    .border_style(Style::new().fg(BORDER))
                    .title(" Recurring Task "),
            )
            .style(Style::new().fg(FG).bg(BG_ALT))
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        dialog,
    );
}
