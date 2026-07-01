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
use crate::tui::overlay::{SearchPurpose, SearchResultItem};
use crate::tui::theme::{self, ACCENT, BG, FG, FG_DIM, FG_MUTED, SELECTED};
use crate::tui::widgets::{priority_icon, status_span};

const RESULT_ROWS: usize = 8;

#[derive(Clone, Copy)]
pub(in crate::tui::ui) struct SearchRenderStatus {
    pub(in crate::tui::ui) stale: bool,
    pub(in crate::tui::ui) no_matches_cached: bool,
}

pub(in crate::tui::ui) struct SearchRenderView<'a> {
    pub(in crate::tui::ui) input: &'a str,
    pub(in crate::tui::ui) cursor: usize,
    pub(in crate::tui::ui) results: &'a [SearchResultItem],
    pub(in crate::tui::ui) selected: usize,
    pub(in crate::tui::ui) total_matches: usize,
    pub(in crate::tui::ui) status: SearchRenderStatus,
    pub(in crate::tui::ui) purpose: &'a SearchPurpose,
}

pub(in crate::tui::ui) fn render_search(frame: &mut Frame, view: SearchRenderView<'_>) {
    let SearchRenderView {
        input,
        cursor,
        results,
        selected,
        total_matches,
        status,
        purpose,
    } = view;
    let width = frame.area().width.saturating_sub(8).clamp(72, 110);
    let result_rows = results.len().min(RESULT_ROWS) as u16;
    let height = if results.is_empty() {
        5
    } else {
        (result_rows * 2 + 6).min(frame.area().height.saturating_sub(2))
    };
    let mut dialog = Dialog::new(purpose.title(), width, height);
    if let Some(summary) = search_summary_line(
        input,
        results.len(),
        total_matches,
        status.stale,
        status.no_matches_cached,
    ) {
        dialog = dialog.right_title(summary);
    }
    let area = dialog.render_block_at(frame, search_dialog_area(frame.area(), width, height));

    if results.is_empty() {
        let [input_area, input_spacer_area, hint_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);
        frame.render_widget(
            Paragraph::new(search_input_line(input, cursor, purpose)),
            input_area,
        );
        frame.render_widget(Paragraph::new(""), input_spacer_area);
        frame.render_widget(Paragraph::new(search_hint_line(purpose)), hint_area);
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
    frame.render_widget(
        Paragraph::new(search_input_line(input, cursor, purpose)),
        input_area,
    );
    frame.render_widget(Paragraph::new(""), input_spacer_area);

    render_result_list(frame, body_area, input, results, selected, status.stale);

    frame.render_widget(Paragraph::new(""), hint_spacer_area);
    frame.render_widget(Paragraph::new(search_hint_line(purpose)), hint_area);
}

fn search_summary_line(
    input: &str,
    shown: usize,
    total: usize,
    stale: bool,
    no_matches_cached: bool,
) -> Option<Line<'static>> {
    if input.trim().is_empty() || (stale && !no_matches_cached) {
        return None;
    }
    let label = if total == 0 {
        "0 matches".to_string()
    } else {
        format!("{} of {}", format_count(shown), format_count(total))
    };
    Some(Line::from(vec![
        Span::styled(" ", Style::new().fg(FG_DIM)),
        Span::styled(label, Style::new().fg(FG_DIM)),
    ]))
}

fn format_count(count: usize) -> String {
    let digits = count.to_string();
    let mut formatted = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            formatted.push(',');
        }
        formatted.push(ch);
    }
    formatted.chars().rev().collect()
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

fn search_input_line(input: &str, cursor: usize, purpose: &SearchPurpose) -> Line<'static> {
    if input.is_empty() {
        let mut chars = purpose.placeholder().chars();
        let first = chars.next().unwrap_or_default().to_string();
        return Line::from(vec![
            cursor_cell(first),
            Span::styled(chars.collect::<String>(), Style::new().fg(FG_DIM)),
        ]);
    }
    input_line("", input, cursor)
}

fn render_result_list(
    frame: &mut Frame,
    area: Rect,
    input: &str,
    results: &[SearchResultItem],
    selected: usize,
    stale: bool,
) {
    let lines = results
        .iter()
        .enumerate()
        .take(RESULT_ROWS)
        .flat_map(|(index, result)| {
            [
                result_line(result, index == selected, input, area.width as usize, stale),
                result_meta_line(result, index == selected, area.width as usize, stale),
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
    stale: bool,
) -> Line<'static> {
    let style = row_style(selected, stale);
    let marker = if selected { "▸" } else { " " };
    let ref_width = 10;
    let title_width = width.saturating_sub(ref_width + 4).max(8);
    let title = truncate_chars(&result.title, title_width);
    let used_width = 2 + ref_width + 1 + title.chars().count();
    let mut spans = vec![Span::styled(format!("{marker} "), style)];
    spans.extend(result_ref_spans(result, ref_width, style));
    spans.push(Span::styled(" ", style));
    spans.extend(title_spans(&title, input, result.matched_field, style));
    spans.push(Span::styled(
        " ".repeat(width.saturating_sub(used_width)),
        style,
    ));
    Line::from(spans)
}

fn result_ref_spans(result: &SearchResultItem, width: usize, style: Style) -> Vec<Span<'static>> {
    let display_ref = truncate_chars(&result.display_ref, width);
    let bg = style.bg.unwrap_or(BG);
    if let Some((project, suffix)) = display_ref.split_once('-') {
        let used_width = project.chars().count() + 1 + suffix.chars().count();
        return vec![
            Span::styled(
                project.to_string(),
                Style::new()
                    .fg(theme::project_color(&result.project_key))
                    .bg(bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled("-", Style::new().fg(FG_DIM).bg(bg)),
            Span::styled(suffix.to_string(), style.fg(FG_MUTED)),
            Span::styled(" ".repeat(width.saturating_sub(used_width)), style),
        ];
    }
    vec![Span::styled(format!("{display_ref:<width$}"), style)]
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

fn result_meta_line(
    result: &SearchResultItem,
    selected: bool,
    width: usize,
    stale: bool,
) -> Line<'static> {
    let bg = row_bg(selected);
    let muted = if stale && !selected {
        Style::new().fg(FG_DIM).bg(bg).add_modifier(Modifier::DIM)
    } else {
        Style::new().fg(FG_DIM).bg(bg)
    };
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

fn row_style(selected: bool, stale: bool) -> Style {
    if selected {
        SELECTED
    } else if stale {
        Style::new().fg(FG_DIM).bg(BG).add_modifier(Modifier::DIM)
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

fn search_hint_line(purpose: &SearchPurpose) -> Line<'static> {
    let mut spans = vec![
        Span::styled(
            "↑/↓ ^N/^P",
            Style::new().fg(FG).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" select", Style::new().fg(FG_DIM)),
        Span::styled("  Enter", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(" {}", purpose.enter_hint()),
            Style::new().fg(FG_DIM),
        ),
    ];
    if let Some(tab_hint) = purpose.tab_hint() {
        spans.extend([
            Span::styled("  Tab", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" {tab_hint}"), Style::new().fg(FG_DIM)),
        ]);
    }
    spans.extend([
        Span::styled("  Esc", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" close", Style::new().fg(FG_DIM)),
    ]);
    Line::from(spans)
}
