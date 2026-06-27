use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Paragraph};

use super::super::dialog::Dialog;
use super::super::input::{cursor_cell, input_line};
use super::super::task_display::{description_preview_text, labels_display};
use super::super::truncate::truncate_chars;
use crate::query::SearchMatchedField;
use crate::queue::{now_seconds, unix_seconds};
use crate::tui::overlay::SearchResultItem;
use crate::tui::theme::{
    self, ACCENT, BG, BG_ALT, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, GREEN, SELECTED,
};

const RESULT_ROWS: usize = 8;
const SEARCH_PLACEHOLDER: &str = "Search by ref, title, label, project, note, status, or priority";

pub(in crate::tui::ui) fn render_search(
    frame: &mut Frame,
    input: &str,
    cursor: usize,
    results: &[SearchResultItem],
    selected: usize,
) {
    let width = frame.area().width.saturating_sub(8).clamp(72, 110);
    let result_rows = (results.len().min(RESULT_ROWS) as u16).clamp(3, RESULT_ROWS as u16);
    let height = if results.is_empty() {
        4
    } else {
        (result_rows * 2 + 12)
            .min(frame.area().height.saturating_sub(2))
            .max(14)
    };
    let area = Dialog::new("Search", width, height)
        .render_block_at(frame, search_dialog_area(frame.area(), width, height));
    let [input_area, body_area, hint_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    frame.render_widget(Paragraph::new(search_input_line(input, cursor)), input_area);

    if results.is_empty() {
        render_empty_state(frame, body_area, input);
    } else if body_area.width < 96 {
        render_stacked_results(frame, body_area, results, selected);
    } else {
        let [list_area, divider_area, preview_area] = Layout::horizontal([
            Constraint::Percentage(48),
            Constraint::Length(1),
            Constraint::Percentage(52),
        ])
        .areas(body_area);
        render_result_list(frame, list_area, results, selected);
        render_divider(frame, divider_area);
        render_preview(frame, preview_area, results.get(selected));
    }

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
        return Line::from(vec![
            Span::raw("/"),
            cursor_cell(" "),
            Span::styled(SEARCH_PLACEHOLDER, Style::new().fg(FG_DIM)),
        ]);
    }
    input_line("/", input, cursor)
}

fn render_empty_state(frame: &mut Frame, area: Rect, input: &str) {
    if !input.trim().is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled("No matching tasks", Style::new().fg(FG_DIM)))
                .style(Style::new().bg(BG_ALT)),
            area,
        );
    }
}

fn render_stacked_results(
    frame: &mut Frame,
    area: Rect,
    results: &[SearchResultItem],
    selected: usize,
) {
    let preview_height = 7.min(area.height.saturating_sub(5));
    let [list_area, divider_area, preview_area] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Length(preview_height),
    ])
    .areas(area);
    let list_rows = (list_area.height / 2).max(1) as usize;
    let lines = results
        .iter()
        .enumerate()
        .take(list_rows)
        .flat_map(|(index, result)| {
            [
                result_line(result, index == selected, list_area.width as usize),
                result_meta_line(result, index == selected, list_area.width as usize),
            ]
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG_ALT)),
        list_area,
    );
    render_horizontal_divider(frame, divider_area);
    render_preview(frame, preview_area, results.get(selected));
}

fn render_result_list(
    frame: &mut Frame,
    area: Rect,
    results: &[SearchResultItem],
    selected: usize,
) {
    let lines = results
        .iter()
        .enumerate()
        .take(RESULT_ROWS)
        .flat_map(|(index, result)| {
            [
                result_line(result, index == selected, area.width as usize),
                result_meta_line(result, index == selected, area.width as usize),
            ]
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG_ALT)),
        area,
    );
}

fn render_divider(frame: &mut Frame, area: Rect) {
    let lines = (0..area.height)
        .map(|_| Line::from("│"))
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(BORDER).bg(BG_ALT)),
        area,
    );
}

fn render_horizontal_divider(frame: &mut Frame, area: Rect) {
    frame.render_widget(Block::new().style(Style::new().bg(BORDER)), area);
}

fn render_preview(frame: &mut Frame, area: Rect, result: Option<&SearchResultItem>) {
    let Some(result) = result else {
        return;
    };
    frame.render_widget(Block::new().style(Style::new().bg(BG)), area);
    let inner = Rect {
        x: area.x.saturating_add(1),
        width: area.width.saturating_sub(1),
        ..area
    };
    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                truncate_chars(&result.display_ref, 16),
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  ", Style::new().fg(FG_DIM)),
            Span::styled(
                truncate_chars(&result.title, inner.width.saturating_sub(18) as usize),
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(vec![
            status_span(&result.status, BG),
            Span::styled("  ", Style::new().fg(FG_DIM).bg(BG)),
            priority_span(&result.priority, BG),
            Span::styled("  ", Style::new().fg(FG_DIM).bg(BG)),
            Span::styled(
                truncate_chars(&result.project_key, 24),
                Style::new()
                    .fg(theme::project_color(&result.project_key))
                    .add_modifier(Modifier::BOLD),
            ),
            deleted_span(result.deleted),
        ]),
        Line::from(vec![
            Span::styled("match ", Style::new().fg(FG_DIM).bg(BG)),
            Span::styled(
                result.matched_field.as_str(),
                Style::new().fg(GREEN).bg(BG).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("  age {}", task_age(result)),
                Style::new().fg(FG_DIM).bg(BG),
            ),
        ]),
        Line::from(vec![
            Span::styled("labels ", Style::new().fg(FG_DIM)),
            Span::styled(
                truncate_chars(
                    &labels_display(&result.labels, ", "),
                    inner.width.saturating_sub(8) as usize,
                ),
                Style::new().fg(FG_MUTED).bg(BG),
            ),
        ]),
        Line::from(""),
    ];
    let preview = result
        .snippet
        .as_deref()
        .unwrap_or(result.description.as_str());
    if !preview.trim().is_empty() {
        lines.push(Line::from(Span::styled(
            preview_label(result.matched_field),
            Style::new().fg(FG_DIM).bg(BG),
        )));
    }
    lines.extend(
        wrapped_preview_lines(
            &description_preview_text(preview),
            inner.width as usize,
            inner.height.saturating_sub(lines.len() as u16) as usize,
        )
        .into_iter()
        .map(|line| Line::from(Span::styled(line, Style::new().fg(FG_MUTED).bg(BG)))),
    );
    frame.render_widget(
        Paragraph::new(Text::from(lines)).style(Style::new().fg(FG).bg(BG)),
        inner,
    );
}

fn result_line(result: &SearchResultItem, selected: bool, width: usize) -> Line<'static> {
    let style = row_style(selected);
    let marker = if selected { "▸" } else { " " };
    let ref_width = 10;
    let title_width = width.saturating_sub(ref_width + 4).max(8);
    Line::from(vec![
        Span::styled(format!("{marker} "), style),
        Span::styled(
            format!(
                "{:<ref_width$}",
                truncate_chars(&result.display_ref, ref_width)
            ),
            style.fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", style),
        Span::styled(truncate_chars(&result.title, title_width), style),
    ])
}

fn result_meta_line(result: &SearchResultItem, selected: bool, width: usize) -> Line<'static> {
    let style = row_style(selected).fg(FG_DIM);
    let labels = labels_display(&result.labels, ", ");
    let deleted = if result.deleted { " deleted" } else { "" };
    let meta = format!(
        "  {} · {} · {} · age={} · match={}{}",
        result.status,
        result.priority,
        labels,
        task_age(result),
        result.matched_field.as_str(),
        deleted
    );
    Line::from(Span::styled(truncate_chars(&meta, width), style))
}

fn row_style(selected: bool) -> Style {
    if selected {
        SELECTED
    } else {
        Style::new().fg(FG).bg(BG_ALT)
    }
}

fn status_span(status: &str, bg: ratatui::style::Color) -> Span<'static> {
    Span::styled(
        status.to_string(),
        theme::status_style(status)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    )
}

fn priority_span(priority: &str, bg: ratatui::style::Color) -> Span<'static> {
    Span::styled(
        priority.to_string(),
        theme::priority_style(priority)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    )
}

fn deleted_span(deleted: bool) -> Span<'static> {
    if deleted {
        Span::styled("  deleted", Style::new().fg(FG_DIM).bg(BG_PANEL))
    } else {
        Span::raw("")
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

fn preview_label(field: SearchMatchedField) -> &'static str {
    match field {
        SearchMatchedField::Description => "description match",
        SearchMatchedField::Note => "note match",
        _ => "description",
    }
}

fn wrapped_preview_lines(value: &str, width: usize, max_lines: usize) -> Vec<String> {
    if max_lines == 0 {
        return Vec::new();
    }
    let width = width.max(16);
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in value.split_whitespace() {
        if current.chars().count() + word.chars().count() + 1 > width && !current.is_empty() {
            lines.push(current);
            current = String::new();
            if lines.len() == max_lines {
                break;
            }
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if lines.len() < max_lines && !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push("(no description)".to_string());
    }
    lines
}

fn search_hint_line() -> Line<'static> {
    Line::from(vec![
        Span::styled("↑/↓", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" preview", Style::new().fg(FG_DIM)),
        Span::styled("  Enter", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" open results", Style::new().fg(FG_DIM)),
        Span::styled("  Esc", Style::new().fg(FG).add_modifier(Modifier::BOLD)),
        Span::styled(" close", Style::new().fg(FG_DIM)),
    ])
}
