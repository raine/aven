use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph, TableState, Wrap};

use crate::query::{RecurrenceSeriesDetail, RecurrenceSeriesListItem};
use crate::tui::app::Focus;
use crate::tui::list_surface::ListSurface;
use crate::tui::store::TuiStore;
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, FG_MUTED, SELECTED, SELECTED_INACTIVE,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecurrenceSeriesHit {
    pub(crate) series_index: usize,
    pub(crate) series_id: aven_core::recurrence::RecurrenceSeriesId,
    pub(crate) viewport_row: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecurrenceSeriesAreas {
    table: Rect,
    preview: Rect,
}

fn recurrence_series_areas(area: Rect) -> RecurrenceSeriesAreas {
    let preview_height = if area.height >= 32 {
        12
    } else if area.height >= 24 {
        8
    } else {
        0
    };
    let [table, preview] = if preview_height > 0 {
        Layout::vertical([Constraint::Fill(1), Constraint::Length(preview_height)]).areas(area)
    } else {
        [area, Rect::default()]
    };
    RecurrenceSeriesAreas { table, preview }
}

pub(crate) fn recurrence_series_at_position(
    store: &TuiStore,
    table_state: &TableState,
    area: Rect,
    column: u16,
    row: u16,
) -> Option<RecurrenceSeriesHit> {
    let area = recurrence_series_areas(area).table;
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
    let areas = recurrence_series_areas(area);
    let rows = Layout::vertical(vec![Constraint::Length(1); areas.table.height as usize])
        .split(areas.table);
    if rows.is_empty() {
        return;
    }
    render_header(frame, rows[0]);
    if store.recurrence_series.is_empty() {
        render_empty(frame, store, areas.table);
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
    if areas.preview.height > 0 {
        render_recurrence_preview(frame, &store.recurrence_series[selected], areas.preview);
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

fn display_ref_spans(display_ref: &str, prefix_color: ratatui::style::Color) -> Vec<Span<'static>> {
    if let Some((prefix, suffix)) = display_ref.split_once('-') {
        vec![
            Span::styled(
                prefix.to_string(),
                Style::new().fg(prefix_color).add_modifier(Modifier::BOLD),
            ),
            Span::styled("-", Style::new().fg(FG_DIM)),
            Span::styled(suffix.to_string(), Style::new().fg(FG_MUTED)),
        ]
    } else {
        vec![Span::styled(
            display_ref.to_string(),
            Style::new().fg(FG_MUTED),
        )]
    }
}

fn recurrence_ref_line(item: &RecurrenceSeriesListItem) -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    spans.extend(display_ref_spans(&item.series_ref, ACCENT));
    Line::from(spans)
}

fn occurrence_line(item: &RecurrenceSeriesListItem) -> Line<'static> {
    let Some(occurrence) = item.current_occurrence.as_ref() else {
        return Line::from(" -");
    };
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(occurrence.slot_on.clone(), Style::new().fg(FG_MUTED)),
        Span::raw(" "),
    ];
    spans.extend(display_ref_spans(
        &occurrence.task_ref,
        theme::project_color(&item.project_key),
    ));
    Line::from(spans)
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
    let schedule = crate::recurrence_input::natural_rule_label(item.series.rule);
    let values = [
        Line::from(format!(" {}", item.series.title)),
        recurrence_ref_line(item),
        Line::from(format!(" {schedule}")),
        occurrence_line(item),
        Line::from(format!(" {}", item.series.state.as_str())),
    ];
    for (cell, value) in Layout::horizontal(columns())
        .areas::<5>(area)
        .into_iter()
        .zip(values)
    {
        frame.render_widget(Paragraph::new(value).style(style), cell);
    }
}

fn render_recurrence_preview(frame: &mut Frame, item: &RecurrenceSeriesListItem, area: Rect) {
    let block = Block::new()
        .title(" SELECTED ")
        .borders(Borders::TOP)
        .border_style(Style::new().fg(BORDER))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::from(recurrence_preview_lines(
            item,
            inner.height as usize,
        )))
        .style(Style::new().fg(FG).bg(BG))
        .wrap(Wrap { trim: false }),
        inner,
    );
}

fn recurrence_preview_lines(item: &RecurrenceSeriesListItem, height: usize) -> Vec<Line<'static>> {
    let mut heading = display_ref_spans(&item.series_ref, ACCENT);
    heading.push(Span::raw("  "));
    heading.push(Span::styled(
        item.series.title.clone(),
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    ));

    let mut fields = vec![
        Span::styled("project ", Style::new().fg(FG_DIM)),
        Span::styled(
            item.project_key.clone(),
            Style::new()
                .fg(theme::project_color(&item.project_key))
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  state ", Style::new().fg(FG_DIM)),
        Span::styled(
            item.series.state.as_str().to_string(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
    ];
    if item.series.priority.as_str() != "none" {
        fields.extend([
            Span::styled("  priority ", Style::new().fg(FG_DIM)),
            Span::styled(
                item.series.priority.as_str().to_string(),
                theme::priority_style(item.series.priority.as_str()).add_modifier(Modifier::BOLD),
            ),
        ]);
    }

    let available = item
        .series
        .available_local_time
        .map(|time| time.format("%H:%M").to_string())
        .unwrap_or_else(|| "start of day".to_string());
    let due = match item.series.due_policy {
        aven_core::recurrence::RecurrenceDuePolicy::SameDay => "same day",
        aven_core::recurrence::RecurrenceDuePolicy::None => "none",
    };
    let schedule = crate::recurrence_input::natural_rule_label(item.series.rule);
    let timing = Line::from(vec![
        Span::styled("repeat ", Style::new().fg(FG_DIM)),
        Span::styled(schedule, Style::new().fg(FG)),
        Span::styled("  available ", Style::new().fg(FG_DIM)),
        Span::styled(available, Style::new().fg(FG_MUTED)),
        Span::styled("  due ", Style::new().fg(FG_DIM)),
        Span::styled(due, Style::new().fg(FG_MUTED)),
        Span::styled("  starts ", Style::new().fg(FG_DIM)),
        Span::styled(
            item.series.start_on.format("%b %-d").to_string(),
            Style::new().fg(FG_MUTED),
        ),
    ]);

    let mut lines = vec![Line::from(heading), Line::from(fields), timing];
    if let Some(occurrence) = item.current_occurrence.as_ref() {
        let mut occurrence_spans = vec![
            Span::styled("current task ", Style::new().fg(FG_DIM)),
            Span::styled(occurrence.slot_on.clone(), Style::new().fg(FG_MUTED)),
            Span::raw(" "),
        ];
        occurrence_spans.extend(display_ref_spans(
            &occurrence.task_ref,
            theme::project_color(&item.project_key),
        ));
        lines.push(Line::from(occurrence_spans));
    }
    if !item.series.description.is_empty() && lines.len() < height {
        lines.push(Line::from(""));
        lines.extend(
            item.series
                .description
                .lines()
                .map(|line| Line::from(Span::styled(line.to_string(), Style::new().fg(FG_MUTED)))),
        );
    }
    lines.truncate(height);
    lines
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

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveTime, Weekday};

    use super::*;
    use crate::choices::{TaskPriority, TaskStatus};
    use crate::ids::{ProjectId, TaskId, WorkspaceId};
    use crate::query::RecurrenceOccurrenceLink;
    use aven_core::recurrence::{
        RecurrenceDuePolicy, RecurrenceRule, RecurrenceSeriesId, RecurrenceSeriesState, TimeZoneId,
    };
    use aven_core::types::RecurrenceSeries;

    fn item() -> RecurrenceSeriesListItem {
        RecurrenceSeriesListItem {
            series: RecurrenceSeries {
                workspace_id: WorkspaceId::new(),
                id: RecurrenceSeriesId::new(),
                title: "Publish weekly project update".to_string(),
                description: "Summarize progress and open decisions.".to_string(),
                project_id: ProjectId::new(),
                priority: TaskPriority::High,
                initial_status: TaskStatus::Todo,
                rule: RecurrenceRule::weekly(Weekday::Fri),
                timezone: "UTC".parse::<TimeZoneId>().unwrap(),
                start_on: NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
                available_local_time: Some(NaiveTime::from_hms_opt(9, 0, 0).unwrap()),
                due_policy: RecurrenceDuePolicy::SameDay,
                state: RecurrenceSeriesState::Active,
                stopped_at: None,
                created_at: "2026-07-29T12:00:00Z".to_string(),
                updated_at: "2026-07-29T12:00:00Z".to_string(),
                deleted: false,
            },
            series_ref: "RCR-6OGN".to_string(),
            project_key: "docs".to_string(),
            rule_label: "Every Friday".to_string(),
            current_occurrence: Some(RecurrenceOccurrenceLink {
                slot_on: "2026-07-31".to_string(),
                task_id: TaskId::new(),
                task_ref: "DCS-F5ZB".to_string(),
            }),
        }
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    #[test]
    fn recurring_series_preview_uses_task_list_height_breakpoints() {
        let tall = recurrence_series_areas(Rect::new(0, 0, 100, 40));
        assert_eq!(tall.table.height, 28);
        assert_eq!(tall.preview.height, 12);

        let medium = recurrence_series_areas(Rect::new(0, 0, 100, 28));
        assert_eq!(medium.table.height, 20);
        assert_eq!(medium.preview.height, 8);

        let short = recurrence_series_areas(Rect::new(0, 0, 100, 20));
        assert_eq!(short.table.height, 20);
        assert_eq!(short.preview.height, 0);
    }

    #[test]
    fn occurrence_ref_uses_project_prefix_style() {
        let item = item();
        let line = occurrence_line(&item);

        assert_eq!(line_text(&line), " 2026-07-31 DCS-F5ZB");
        assert_eq!(line.spans[3].style.fg, Some(theme::project_color("docs")));
        assert_eq!(line.spans[4].style.fg, Some(FG_DIM));
        assert_eq!(line.spans[5].style.fg, Some(FG_MUTED));
    }

    #[test]
    fn recurring_preview_summarizes_series_and_current_task() {
        let lines = recurrence_preview_lines(&item(), 12);
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("RCR-6OGN  Publish weekly project update"));
        assert!(text.contains("project docs  state active  priority high"));
        assert!(text.contains("repeat Every Friday  available 09:00  due same day"));
        assert!(text.contains("current task 2026-07-31 DCS-F5ZB"));
        assert!(text.contains("Summarize progress and open decisions."));
    }
}
