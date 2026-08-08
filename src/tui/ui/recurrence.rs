use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, TableState, Wrap};

use crate::query::{RecurrenceSeriesDetail, RecurrenceSeriesListItem};
use crate::tui::app::Focus;
use crate::tui::list_surface::ListSurface;
use crate::tui::store::TuiStore;
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BORDER, FG, FG_DIM, FG_MUTED, GREEN, ORANGE, RED, SELECTED,
    SELECTED_INACTIVE,
};

use super::dialog::{Dialog, dialog_hint_line};
use super::scroll::render_vertical_scrollbar;

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
    let areas = if store.recurrence_series.is_empty() {
        RecurrenceSeriesAreas {
            table: area,
            preview: Rect::default(),
        }
    } else {
        recurrence_series_areas(area)
    };
    let rows = Layout::vertical(vec![Constraint::Length(1); areas.table.height as usize])
        .split(areas.table);
    if rows.is_empty() {
        return;
    }
    render_header(frame, rows[0]);
    if store.recurrence_series.is_empty() {
        let body = Rect {
            y: areas.table.y.saturating_add(1),
            height: areas.table.height.saturating_sub(1),
            ..areas.table
        };
        super::empty_state::render_empty_state(
            frame,
            body,
            super::empty_state::recurrence_empty_state(store),
        );
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

fn recurrence_state_style(state: aven_core::recurrence::RecurrenceSeriesState) -> Style {
    let color = match state {
        aven_core::recurrence::RecurrenceSeriesState::Active => GREEN,
        aven_core::recurrence::RecurrenceSeriesState::Paused => ORANGE,
        aven_core::recurrence::RecurrenceSeriesState::Stopped => RED,
    };
    Style::new().fg(color).add_modifier(Modifier::BOLD)
}

fn recurrence_detail_field(label: &'static str, value: Span<'static>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<13}"), Style::new().fg(FG_DIM)),
        value,
    ])
}

fn recurrence_detail_actions(
    detail: &RecurrenceSeriesDetail,
    has_current: bool,
) -> Vec<(&'static str, &'static str)> {
    let mut actions = Vec::new();
    if has_current {
        actions.push(("Enter", "open task"));
    }
    match detail.series.state {
        aven_core::recurrence::RecurrenceSeriesState::Active => actions.push(("t r p", "pause")),
        aven_core::recurrence::RecurrenceSeriesState::Paused => actions.push(("t r r", "resume")),
        aven_core::recurrence::RecurrenceSeriesState::Stopped => {}
    }
    actions.push(("t r h", "history"));
    if detail.series.state != aven_core::recurrence::RecurrenceSeriesState::Stopped {
        actions.push(("t r s", "stop"));
    }
    actions.push(("Esc", "close"));
    actions
}

fn recurrence_detail_lines(detail: &RecurrenceSeriesDetail) -> Vec<Line<'static>> {
    let current = detail
        .current_occurrence
        .as_ref()
        .and_then(|occurrence| occurrence.task_id.as_ref())
        .and_then(|_| {
            Some((
                detail.summary.current_slot_on.clone()?,
                detail.summary.current_task_ref.clone()?,
            ))
        });
    let starts = detail.series.start_on.format("%b %-d").to_string();
    let available = detail
        .series
        .available_local_time
        .map(|time| time.format("%H:%M").to_string())
        .unwrap_or_else(|| "start of day".to_string());
    let due = match detail.series.due_policy {
        aven_core::recurrence::RecurrenceDuePolicy::SameDay => "same day",
        aven_core::recurrence::RecurrenceDuePolicy::None => "none",
    };

    let mut identity = display_ref_spans(&detail.summary.series_ref, ACCENT);
    identity.push(Span::styled("  ", Style::new().fg(FG_DIM)));
    identity.push(Span::styled(
        detail.series.state.as_str().to_string(),
        recurrence_state_style(detail.series.state),
    ));
    let mut lines = vec![
        Line::from(Span::styled(
            detail.series.title.clone(),
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        )),
        Line::from(identity),
        Line::from(""),
        Line::from(Span::styled(
            "SCHEDULE",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        )),
        recurrence_detail_field(
            "Repeat",
            Span::styled(
                crate::recurrence_input::natural_rule_label(detail.series.rule),
                Style::new().fg(FG),
            ),
        ),
        recurrence_detail_field(
            "Available",
            Span::styled(available, Style::new().fg(FG_MUTED)),
        ),
        recurrence_detail_field("Due", Span::styled(due, Style::new().fg(FG_MUTED))),
        recurrence_detail_field("Starts", Span::styled(starts, Style::new().fg(FG_MUTED))),
    ];
    if detail.series.priority.as_str() != "none" {
        lines.push(recurrence_detail_field(
            "Priority",
            Span::styled(
                detail.series.priority.as_str().to_string(),
                theme::priority_style(detail.series.priority.as_str()).add_modifier(Modifier::BOLD),
            ),
        ));
    }
    if !detail.labels.is_empty() {
        lines.push(recurrence_detail_field(
            "Labels",
            Span::styled(detail.labels.join(", "), Style::new().fg(ACCENT)),
        ));
    }
    if let Some((slot, task_ref)) = current.as_ref() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "CURRENT OCCURRENCE",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        )));
        let mut occurrence = vec![
            Span::styled(slot.clone(), Style::new().fg(FG_MUTED)),
            Span::raw("  "),
        ];
        occurrence.extend(display_ref_spans(task_ref, ACCENT));
        lines.push(Line::from(occurrence));
    }
    if !detail.series.description.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "DESCRIPTION",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        )));
        lines.extend(
            detail
                .series
                .description
                .lines()
                .map(|line| Line::from(Span::styled(line.to_string(), Style::new().fg(FG_MUTED)))),
        );
    }
    if !detail.lifecycle_conflicts.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "LIFECYCLE CONFLICTS",
            Style::new().fg(RED).add_modifier(Modifier::BOLD),
        )));
        lines.extend(detail.lifecycle_conflicts.iter().map(|conflict| {
            Line::from(vec![
                Span::styled(format!("{}  ", conflict.field), Style::new().fg(FG_DIM)),
                Span::styled(
                    format!("{} / {}", conflict.local_value, conflict.remote_value),
                    Style::new().fg(FG_MUTED),
                ),
            ])
        }));
    }
    lines
}

fn recurrence_detail_content_height(lines: &[Line<'_>], dialog_width: u16) -> usize {
    let content_width = dialog_width.saturating_sub(4).max(1) as usize;
    lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(content_width))
        .sum()
}

fn recurrence_detail_height(lines: &[Line<'_>], dialog_width: u16, max_height: u16) -> u16 {
    (recurrence_detail_content_height(lines, dialog_width) as u16)
        .saturating_add(3)
        .min(max_height)
}

pub(super) fn render_recurrence_detail(
    frame: &mut Frame,
    detail: &RecurrenceSeriesDetail,
    scroll: u16,
) {
    let lines = recurrence_detail_lines(detail);
    let area = frame.area();
    let width = area.width.saturating_sub(4).min(76);
    let height = recurrence_detail_height(&lines, width, area.height.saturating_sub(2));
    let content_height = recurrence_detail_content_height(&lines, width);
    let content = Dialog::new("Recurring task", width, height).render_block(frame);
    let [body, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(content);
    frame.render_widget(
        Paragraph::new(Text::from(lines))
            .style(Style::new().fg(FG).bg(BG_ALT))
            .scroll((scroll, 0))
            .wrap(Wrap { trim: false }),
        body,
    );
    render_vertical_scrollbar(frame, body, content_height, scroll);
    let has_current = detail
        .current_occurrence
        .as_ref()
        .and_then(|occurrence| occurrence.task_id.as_ref())
        .is_some();
    frame.render_widget(
        Paragraph::new(dialog_hint_line(&recurrence_detail_actions(
            detail,
            has_current,
        )))
        .style(Style::new().fg(FG).bg(BG_ALT)),
        footer,
    );
}

#[cfg(test)]
mod tests {
    use chrono::{NaiveDate, NaiveTime, Weekday};

    use super::*;
    use crate::choices::{TaskPriority, TaskStatus};
    use crate::ids::{ProjectId, TaskId, WorkspaceId};
    use crate::query::{RecurrenceCounts, RecurrenceOccurrenceLink, RecurrenceSeriesSummary};
    use aven_core::recurrence::{
        RecurrenceDuePolicy, RecurrenceProjectionState, RecurrenceRule, RecurrenceSeriesId,
        RecurrenceSeriesState, TimeZoneId,
    };
    use aven_core::types::{RecurrenceOccurrence, RecurrenceSeries};

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

    fn detail() -> RecurrenceSeriesDetail {
        let item = item();
        let occurrence = item.current_occurrence.as_ref().unwrap();
        let series = item.series.clone();
        RecurrenceSeriesDetail {
            series: series.clone(),
            labels: vec!["release".to_string()],
            metadata: Vec::new(),
            summary: RecurrenceSeriesSummary {
                series: series.clone(),
                series_ref: item.series_ref,
                rule_label: item.rule_label,
                current_slot_on: Some(occurrence.slot_on.clone()),
                current_task_ref: Some(occurrence.task_ref.clone()),
                counts: RecurrenceCounts::default(),
            },
            current_occurrence: Some(RecurrenceOccurrence {
                workspace_id: series.workspace_id,
                series_id: series.id,
                slot_on: NaiveDate::from_ymd_opt(2026, 7, 31).unwrap(),
                task_id: Some(occurrence.task_id.clone()),
                outcome: None,
                resolved_at: None,
                outcome_change_id: None,
                projection_state: RecurrenceProjectionState::Projected,
                archived_at: None,
            }),
            lifecycle_conflicts: Vec::new(),
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

    #[test]
    fn recurring_detail_uses_dialog_hierarchy_and_standard_hints() {
        let lines = recurrence_detail_lines(&detail());
        let text = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");

        assert!(text.contains("RCR-6OGN  active"));
        assert!(text.contains("SCHEDULE"));
        assert!(text.contains("Repeat       Every Friday"));
        assert!(text.contains("CURRENT OCCURRENCE"));
        assert!(text.contains("2026-07-31  DCS-F5ZB"));
        assert!(text.contains("DESCRIPTION"));

        let state = lines[1]
            .spans
            .iter()
            .find(|span| span.content == "active")
            .unwrap();
        assert_eq!(state.style.fg, Some(GREEN));
        let detail = detail();
        let hint = dialog_hint_line(&recurrence_detail_actions(&detail, true));
        assert_eq!(
            line_text(&hint),
            "Enter open task  t r p pause  t r h history  t r s stop  Esc close"
        );
        for key in ["Enter", "t r p", "t r h", "t r s", "Esc"] {
            let span = hint.spans.iter().find(|span| span.content == key).unwrap();
            assert_eq!(span.style.fg, Some(FG));
            assert!(span.style.add_modifier.contains(Modifier::BOLD));
        }
    }

    #[test]
    fn recurring_detail_uses_the_standard_rounded_dialog() {
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_recurrence_detail(frame, &detail(), 0))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("╭─ Recurring task"));
        assert!(rendered.contains("SCHEDULE"));
        assert!(rendered.contains("CURRENT OCCURRENCE"));
    }

    #[test]
    fn recurring_detail_keeps_hints_visible_with_scrollable_content() {
        let mut detail = detail();
        detail.series.description = (1..=12)
            .map(|line| format!("Description line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let backend = ratatui::backend::TestBackend::new(100, 16);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| render_recurrence_detail(frame, &detail, 0))
            .unwrap();
        let buffer = terminal.backend().buffer();
        let rendered = (0..buffer.area.height)
            .map(|row| {
                (0..buffer.area.width)
                    .map(|column| buffer[(column, row)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Enter open task"));
        assert!(rendered.contains("Esc close"));
        assert!(rendered.contains("▲"));
    }
}
