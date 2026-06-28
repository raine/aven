use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::Paragraph;

use super::super::dialog::Dialog;
use super::super::input::{cursor_cell, input_line};
use super::super::task_display::labels_display;
use super::super::truncate::truncate_chars;
use crate::query::SearchMatchedField;
use crate::queue::{now_seconds, unix_seconds};
use crate::tui::overlay::SearchResultItem;
use crate::tui::theme::{self, ACCENT, BG, FG, FG_DIM, SELECTED};
use crate::tui::widgets::{priority_icon, status_span};

const RESULT_ROWS: usize = 8;
const SEARCH_PLACEHOLDER: &str = "Search tasks, notes, labels, and projects...";

pub(in crate::tui::ui) fn render_search(
    frame: &mut Frame,
    input: &str,
    cursor: usize,
    results: &[SearchResultItem],
    selected: usize,
) {
    let width = frame.area().width.saturating_sub(8).clamp(72, 110);
    let result_rows = results.len().min(RESULT_ROWS) as u16;
    let has_empty_input = input.trim().is_empty();
    let height = if results.is_empty() && has_empty_input {
        5
    } else if results.is_empty() {
        7
    } else {
        (result_rows * 2 + 6)
            .min(frame.area().height.saturating_sub(2))
            .max(10)
    };
    let area = Dialog::new("Search", width, height)
        .render_block_at(frame, search_dialog_area(frame.area(), width, height));

    if results.is_empty() && has_empty_input {
        let [input_area, input_spacer_area, hint_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(Paragraph::new(search_input_line(input, cursor)), input_area);
        frame.render_widget(Paragraph::new(""), input_spacer_area);
        frame.render_widget(Paragraph::new(search_hint_line()), hint_area);
        return;
    }

    let [
        input_area,
        input_spacer_area,
        body_area,
        hint_spacer_area,
        hint_area,
    ] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(search_input_line(input, cursor)), input_area);
    frame.render_widget(Paragraph::new(""), input_spacer_area);

    if results.is_empty() {
        render_empty_state(frame, body_area, input);
    } else {
        render_result_list(frame, body_area, input, results, selected);
    }

    frame.render_widget(Paragraph::new(""), hint_spacer_area);
    frame.render_widget(Paragraph::new(search_hint_line()), hint_area);
}

fn search_dialog_area(frame: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(frame.width);
    let height = height.min(frame.height);
    let x = frame.x + frame.width.saturating_sub(width) / 2;
    let top_anchor = frame.height / 4;
    let y = frame
        .y
        .saturating_add(top_anchor)
        .min(frame.y + frame.height.saturating_sub(height));
    Rect {
        x,
        y,
        width,
        height,
    }
}

fn search_input_line(input: &str, cursor: usize) -> Line<'static> {
    if input.is_empty() {
        let mut chars = SEARCH_PLACEHOLDER.chars();
        let first = chars.next().unwrap_or_default().to_string();
        return Line::from(vec![
            cursor_cell(first),
            Span::styled(chars.collect::<String>(), Style::new().fg(FG_DIM)),
        ]);
    }
    input_line("", input, cursor)
}

fn render_empty_state(frame: &mut Frame, area: Rect, input: &str) {
    if !input.trim().is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("No matching tasks", Style::new().fg(FG_DIM)))
                .style(Style::new().bg(BG)),
            area,
        );
    }
}

fn render_result_list(
    frame: &mut Frame,
    area: Rect,
    input: &str,
    results: &[SearchResultItem],
    selected: usize,
) {
    let lines = results
        .iter()
        .enumerate()
        .take(RESULT_ROWS)
        .flat_map(|(index, result)| {
            [
                result_line(result, index == selected, input, area.width as usize),
                result_meta_line(result, index == selected, area.width as usize),
            ]
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG)),
        area,
    );
}

fn result_line(
    result: &SearchResultItem,
    selected: bool,
    input: &str,
    width: usize,
) -> Line<'static> {
    let style = row_style(selected);
    let marker = if selected { "▸" } else { " " };
    let ref_width = 10;
    let title_width = width.saturating_sub(ref_width + 4).max(8);
    let title = truncate_chars(&result.title, title_width);
    let used_width = 2 + ref_width + 1 + title.chars().count();
    let mut spans = vec![
        Span::styled(format!("{marker} "), style),
        Span::styled(
            format!(
                "{:<ref_width$}",
                truncate_chars(&result.display_ref, ref_width)
            ),
            style.fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", style),
    ];
    spans.extend(title_spans(&title, input, result.matched_field, style));
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(used_width)),
        style,
    ));
    Line::from(spans)
}

fn title_spans(
    title: &str,
    input: &str,
    matched_field: SearchMatchedField,
    style: Style,
) -> Vec<Span<'static>> {
    if matched_field != SearchMatchedField::Title {
        return vec![Span::styled(title.to_string(), style)];
    }
    let Some(ranges) = title_match_ranges(title, input) else {
        return vec![Span::styled(title.to_string(), style)];
    };
    let mut spans = Vec::new();
    let mut cursor = 0;
    for range in ranges {
        if range.start > cursor {
            spans.push(Span::styled(title[cursor..range.start].to_string(), style));
        }
        spans.push(Span::styled(
            title[range.clone()].to_string(),
            style.fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        cursor = range.end;
    }
    if cursor < title.len() {
        spans.push(Span::styled(title[cursor..].to_string(), style));
    }
    spans
}

fn title_match_ranges(title: &str, input: &str) -> Option<Vec<std::ops::Range<usize>>> {
    let normalized_title = title.to_ascii_lowercase();
    let query = input.trim().to_ascii_lowercase();
    if query.is_empty() {
        return None;
    }
    if let Some(index) = normalized_title.find(&query) {
        return Some(std::iter::once(index..index + query.len()).collect());
    }
    let mut ranges = query
        .split_whitespace()
        .filter_map(|token| {
            normalized_title
                .find(token)
                .map(|index| index..index + token.len())
        })
        .collect::<Vec<_>>();
    if ranges.is_empty() {
        return None;
    }
    ranges.sort_by_key(|range| range.start);
    Some(merge_ranges(ranges))
}

fn merge_ranges(ranges: Vec<std::ops::Range<usize>>) -> Vec<std::ops::Range<usize>> {
    let mut merged = Vec::<std::ops::Range<usize>>::new();
    for range in ranges {
        if let Some(last) = merged.last_mut()
            && range.start <= last.end
        {
            last.end = last.end.max(range.end);
            continue;
        }
        merged.push(range);
    }
    merged
}

fn result_meta_line(result: &SearchResultItem, selected: bool, width: usize) -> Line<'static> {
    let bg = row_bg(selected);
    let muted = Style::new().fg(FG_DIM).bg(bg);
    let labels = labels_display(&result.labels, ", ");
    let priority = result.priority.as_str();
    let priority_label = format!("{} {priority}", priority_icon(priority));
    let mut spans = vec![
        Span::styled("  ", muted),
        apply_bg(status_span(&result.status), bg),
        Span::styled(" · ", muted),
        Span::styled(
            priority_label,
            theme::priority_style(priority)
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" · ", muted),
        Span::styled(labels, muted),
        Span::styled(format!(" · age={}", task_age(result)), muted),
        Span::styled(format!(" · match={}", result.matched_field.as_str()), muted),
    ];
    if result.deleted {
        spans.push(Span::styled(" deleted", muted));
    }
    truncate_spans_to_width(&mut spans, width);
    let used_width = spans_width(&spans);
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(used_width)),
        muted,
    ));
    Line::from(spans)
}

fn apply_bg(mut span: Span<'static>, bg: Color) -> Span<'static> {
    span.style = span.style.bg(bg);
    span
}

fn truncate_spans_to_width(spans: &mut Vec<Span<'static>>, width: usize) {
    let mut used = 0;
    let mut index = 0;
    while index < spans.len() {
        let content_width = spans[index].content.chars().count();
        if used + content_width > width {
            let remaining = width.saturating_sub(used);
            spans[index].content = truncate_chars(&spans[index].content, remaining).into();
            spans.truncate(index + 1);
            return;
        }
        used += content_width;
        index += 1;
    }
}

fn spans_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| span.content.chars().count()).sum()
}

fn row_style(selected: bool) -> Style {
    if selected {
        SELECTED
    } else {
        Style::new().fg(FG).bg(BG)
    }
}

fn row_bg(selected: bool) -> Color {
    if selected {
        SELECTED.bg.unwrap_or(BG)
    } else {
        BG
    }
}

fn task_age(result: &SearchResultItem) -> String {
    unix_seconds(&result.created_at)
        .map(|created_at| compact_age(now_seconds().saturating_sub(created_at).max(0)))
        .unwrap_or_else(|| "?".to_string())
}

fn compact_age(age_seconds: i64) -> String {
    let minutes = age_seconds / 60;
    if minutes < 60 {
        return format!("{}m", minutes.max(0));
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h");
    }
    let days = hours / 24;
    if days < 14 {
        return format!("{days}d");
    }
    let weeks = days / 7;
    if weeks < 13 {
        return format!("{weeks}w");
    }
    format!("{}mo", days / 30)
}

fn search_hint_line() -> Line<'static> {
    Line::from(vec![
        Span::styled(
            "↑/↓ ^N/^P",
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" select", Style::new().fg(FG_DIM)),
        Span::styled("  Enter", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" open task", Style::new().fg(FG_DIM)),
        Span::styled("  Tab", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" open results", Style::new().fg(FG_DIM)),
        Span::styled("  Esc", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" close", Style::new().fg(FG_DIM)),
    ])
}
