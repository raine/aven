use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::super::input::clipped_input_line;
use super::super::task_display::linked_task_ref_spans;
use super::super::task_list::EPIC_MARKER;
use super::attachments::DetailInlineImageContext;
use super::body::{DetailBodyBlock, DetailBodyDocument};
use super::{DetailTextSelection, TaskListItem, TextInputView};
use crate::tui::markdown::{
    MarkdownBlock, MarkdownRenderContext, render_markdown_with_context_without_link_urls,
    render_markdown_without_link_urls,
};
use crate::tui::theme;
use crate::tui::widgets::{priority_short, status_span};

#[derive(Debug, Clone)]
pub(super) struct SelectableLine {
    pub(super) text: String,
    pub(super) document_start: usize,
    pub(super) body_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DetailHyperlink {
    pub(super) url: String,
    pub(super) line_index: usize,
    pub(super) start_column: usize,
    pub(super) end_column: usize,
}

#[derive(Debug, Clone)]
pub(super) struct DetailSelectableDocument {
    pub(super) text: String,
    pub(super) title: Vec<SelectableLine>,
    pub(super) description: Vec<SelectableLine>,
}

struct ParsedMarkdownLink {
    label: String,
    url: String,
}
pub(super) fn detail_selectable_document_from_body(
    item: &TaskListItem,
    width: usize,
    wrap_title: bool,
    body: &DetailBodyDocument,
) -> DetailSelectableDocument {
    let title = if wrap_title {
        title_line_ranges(&item.task.title, width)
            .into_iter()
            .map(|range| SelectableLine {
                text: item.task.title[range.clone()].to_string(),
                document_start: range.start,
                body_index: None,
            })
            .collect()
    } else {
        vec![SelectableLine {
            text: item.task.title.clone(),
            document_start: 0,
            body_index: None,
        }]
    };
    let mut text = item.task.title.clone();
    let mut description = body.selectable_description.clone();
    if !description.is_empty() {
        text.push('\n');
        let description_start = text.len();
        text.push_str(&body.selectable_text);
        for line in &mut description {
            line.document_start += description_start;
        }
    }
    DetailSelectableDocument {
        text,
        title,
        description,
    }
}

pub(super) fn apply_detail_selection_from_document(
    document: &DetailSelectableDocument,
    selection: &DetailTextSelection,
    sticky_lines: &mut [Line<'static>],
    body_lines: &mut [Line<'static>],
    body_start: usize,
) {
    let range = selection.range();
    for (line, selectable) in sticky_lines.iter_mut().zip(&document.title) {
        highlight_selectable_line(line, selectable, &range, 0);
    }
    let body_end = body_start.saturating_add(body_lines.len());
    let first = document
        .description
        .partition_point(|line| line.body_index.is_some_and(|index| index < body_start));
    let last = document.description[first..]
        .partition_point(|line| line.body_index.is_some_and(|index| index < body_end))
        + first;
    for selectable in &document.description[first..last] {
        if let Some(line) = selectable
            .body_index
            .and_then(|index| index.checked_sub(body_start))
            .and_then(|index| body_lines.get_mut(index))
        {
            highlight_selectable_line(line, selectable, &range, 1);
        }
    }
}

pub(super) fn highlight_selectable_line(
    line: &mut Line<'static>,
    selectable: &SelectableLine,
    selection: &std::ops::Range<usize>,
    skipped_spans: usize,
) {
    let line_start = selectable.document_start;
    let line_end = line_start + selectable.text.len();
    let start = selection.start.max(line_start).min(line_end) - line_start;
    let end = selection.end.max(line_start).min(line_end) - line_start;
    if start >= end {
        return;
    }

    let mut rebuilt = Vec::new();
    let mut offset = 0;
    for (index, span) in std::mem::take(&mut line.spans).into_iter().enumerate() {
        if index < skipped_spans {
            rebuilt.push(span);
            continue;
        }
        let content = span.content.as_ref();
        let span_start = offset;
        let span_end = offset + content.len();
        let selected_start = start.max(span_start).min(span_end) - span_start;
        let selected_end = end.max(span_start).min(span_end) - span_start;
        if selected_start > 0 {
            rebuilt.push(Span::styled(
                content[..selected_start].to_string(),
                span.style,
            ));
        }
        if selected_start < selected_end {
            rebuilt.push(Span::styled(
                content[selected_start..selected_end].to_string(),
                span.style.fg(theme::inverse_fg()).bg(theme::accent()),
            ));
        }
        if selected_end < content.len() {
            rebuilt.push(Span::styled(
                content[selected_end..].to_string(),
                span.style,
            ));
        }
        offset = span_end;
    }
    line.spans = rebuilt;
}

pub(super) fn detail_header_options(
    item: &TaskListItem,
    width: usize,
    inline_title_editor: Option<&TextInputView>,
) -> Vec<Line<'static>> {
    let mut summary_spans = vec![
        Span::styled(
            "● ",
            Style::new().fg(theme::project_color(&item.task.project_key)),
        ),
        Span::styled(
            item.task.project_key.clone(),
            Style::new().fg(theme::fg()).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" / ", Style::new().fg(theme::fg_dim())),
    ];
    summary_spans.extend(linked_task_ref_spans(
        &item.display_ref,
        &item.task.project_key,
    ));
    if item.task.is_epic {
        summary_spans.extend([
            Span::styled("  ", Style::new().fg(theme::fg_dim())),
            Span::styled(EPIC_MARKER, Style::new().fg(theme::yellow())),
        ]);
    }
    summary_spans.extend([
        Span::styled("   ", Style::new().fg(theme::fg_dim())),
        status_span(item.task.status.as_str()),
        Span::styled("   ", Style::new().fg(theme::fg_dim())),
        Span::styled(
            priority_short(item.task.priority.as_str()),
            theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
        ),
    ]);
    let mut lines = detail_title_lines(item, width, inline_title_editor);
    lines.extend([
        Line::from(Span::styled(
            "─".repeat(width),
            Style::new().fg(theme::border()),
        )),
        Line::from(summary_spans),
        Line::from(""),
    ]);
    lines
}

pub(super) fn detail_title_lines(
    item: &TaskListItem,
    width: usize,
    inline_title_editor: Option<&TextInputView>,
) -> Vec<Line<'static>> {
    if let Some(editor) = inline_title_editor {
        let mut line = clipped_input_line(&editor.input, editor.cursor, width);
        for span in &mut line.spans {
            span.style = Style::new()
                .fg(theme::fg())
                .add_modifier(Modifier::BOLD)
                .patch(span.style);
        }
        return vec![line];
    }

    title_line_ranges(&item.task.title, width)
        .into_iter()
        .map(|range| {
            Line::from(Span::styled(
                item.task.title[range].to_string(),
                Style::new().fg(theme::fg()).add_modifier(Modifier::BOLD),
            ))
        })
        .collect()
}

pub(super) fn title_line_ranges(title: &str, width: usize) -> Vec<std::ops::Range<usize>> {
    let width = width.max(1);
    let mut words = Vec::new();
    let mut word_start = None;
    for (index, character) in title.char_indices() {
        if character.is_whitespace() {
            if let Some(start) = word_start.take() {
                words.push(start..index);
            }
        } else if word_start.is_none() {
            word_start = Some(index);
        }
    }
    if let Some(start) = word_start {
        words.push(start..title.len());
    }
    if words.is_empty() {
        return std::iter::once(0..title.len()).collect();
    }

    let mut lines = Vec::new();
    let mut current: Option<std::ops::Range<usize>> = None;
    for word in words {
        if let Some(line) = &mut current {
            let candidate = line.start..word.end;
            if title[candidate.clone()].width() <= width {
                line.end = word.end;
                continue;
            }
            lines.push(line.clone());
            current = None;
        }

        let mut chunk_start = word.start;
        for (offset, character) in title[word.clone()].char_indices() {
            let index = word.start + offset;
            let end = index + character.len_utf8();
            if title[chunk_start..end].width() <= width {
                continue;
            }
            if chunk_start < index {
                lines.push(chunk_start..index);
                chunk_start = index;
            }
            if title[chunk_start..end].width() > width {
                lines.push(chunk_start..end);
                chunk_start = end;
            }
        }
        if chunk_start < word.end {
            current = Some(chunk_start..word.end);
        }
    }
    if let Some(line) = current {
        lines.push(line);
    }
    lines
}

fn markdown_links(markdown: &str) -> Vec<ParsedMarkdownLink> {
    let mut links = Vec::new();
    let mut current = None;
    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Link { dest_url, .. }) => {
                current = Some(ParsedMarkdownLink {
                    label: String::new(),
                    url: dest_url.to_string(),
                });
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some(link) = &mut current {
                    link.label.push_str(&text);
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if let Some(link) = &mut current {
                    link.label.push(' ');
                }
            }
            Event::End(TagEnd::Link) => {
                if let Some(link) = current.take() {
                    links.push(link);
                }
            }
            _ => {}
        }
    }
    links
}

pub(super) fn markdown_hyperlinks(
    markdown: &str,
    lines: &[Line<'static>],
    line_offset: usize,
    column_offset: usize,
) -> Vec<DetailHyperlink> {
    let links = markdown_links(markdown);
    let mut placements = Vec::new();
    let mut link_index = 0;
    let mut rendered_width = 0usize;
    for (line_index, line) in lines.iter().enumerate() {
        let mut column = column_offset;
        for span in &line.spans {
            let width = span.content.width();
            if span.style.add_modifier.contains(Modifier::UNDERLINED)
                && let Some(link) = links.get(link_index)
            {
                if (link.url.starts_with("https://") || link.url.starts_with("http://"))
                    && width > 0
                {
                    placements.push(DetailHyperlink {
                        url: link.url.clone(),
                        line_index: line_offset.saturating_add(line_index),
                        start_column: column,
                        end_column: column.saturating_add(width),
                    });
                }
                rendered_width = rendered_width.saturating_add(width);
                if rendered_width >= link.label.width() {
                    link_index = link_index.saturating_add(1);
                    rendered_width = 0;
                }
            }
            column = column.saturating_add(width);
        }
    }
    placements
}

pub(super) fn plain_metadata_lines(value: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let width = width.max(1);
    value
        .split('\n')
        .flat_map(|line| {
            let mut lines = Vec::new();
            let mut text = String::new();
            let mut cells = 0;
            for c in line.chars() {
                let shown = if c.is_control() { '�' } else { c };
                let size = unicode_width::UnicodeWidthChar::width(shown).unwrap_or(0);
                if cells + size > width && cells > 0 {
                    lines.push(Line::styled(std::mem::take(&mut text), style));
                    cells = 0;
                }
                text.push(shown);
                cells += size;
            }
            lines.push(Line::styled(text, style));
            lines
        })
        .collect()
}

pub(super) fn quoted_block_lines(body: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(3).max(1);
    render_markdown_without_link_urls(body, content_width)
        .into_iter()
        .map(|line| {
            let mut spans = line_with_base_style(line, style).spans;
            spans.insert(0, Span::styled("│ ", Style::new().fg(theme::border())));
            Line::from(spans)
        })
        .collect()
}

pub(super) fn detail_body_blocks(
    body: &str,
    content_width: usize,
    context: MarkdownRenderContext,
    _inline_images: Option<&DetailInlineImageContext>,
) -> Vec<DetailBodyBlock> {
    render_markdown_with_context_without_link_urls(body, content_width, context)
        .into_iter()
        .map(|block| match block {
            MarkdownBlock::Text(line) => DetailBodyBlock::Line(line),
        })
        .collect()
}

pub(super) fn quoted_line(line: Line<'static>, style: Style) -> Line<'static> {
    let mut spans = line_with_base_style(line, style).spans;
    spans.insert(0, Span::styled("│ ", Style::new().fg(theme::border())));
    Line::from(spans)
}

pub(super) fn line_with_base_style(mut line: Line<'static>, base: Style) -> Line<'static> {
    for span in &mut line.spans {
        span.style = base.patch(span.style);
    }
    line
}

pub(super) fn keycap_style() -> Style {
    Style::new()
        .fg(theme::fg())
        .bg(theme::bg_panel())
        .add_modifier(Modifier::BOLD)
}
