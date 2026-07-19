use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use std::collections::HashSet;

use super::input::clipped_input_line;
use super::scroll::{clamp_scroll_start, scrollbar_thumb_position};
use super::task_display::{description_or_placeholder, labels_display};
use super::task_list::EPIC_MARKER;
use super::timestamps::local_timestamp_display;
use super::truncate::truncate_width;
use crate::query::TaskListItem;
use crate::task_render::{AttachmentMetadataJson, attachment_placeholder};
use crate::tui::app::WidgetState;
use crate::tui::detail_selection::{DetailTextSelection, TextCell, text_cell_at_column};
use crate::tui::markdown::{
    MarkdownBlock, MarkdownRenderContext, flatten_markdown_blocks, render_markdown,
    render_markdown_with_context,
};
use crate::tui::overlay::TextInputView;
use crate::tui::store::TuiStore;
use crate::tui::theme::{
    self, ACCENT, BG, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, INVERSE_FG, ORANGE, RED, YELLOW,
};
use crate::tui::widgets::{priority_short, status_chip, status_span};
use unicode_width::UnicodeWidthStr;

const DETAIL_DEPENDENCY_TREE_CAP: usize = 3;

#[derive(Debug, Clone, Copy)]
enum DependencyDirection {
    Blocker,
    Dependent,
}

impl DependencyDirection {
    fn marker(self) -> &'static str {
        match self {
            Self::Blocker => "←",
            Self::Dependent => "→",
        }
    }
}

#[derive(Debug)]
struct DetailContentLayout {
    body_area: Rect,
    content_area: Rect,
    metadata_area: Rect,
}

#[derive(Debug)]
struct DetailContentRenderModel {
    sticky_lines: Vec<Line<'static>>,
    lines: Vec<Line<'static>>,
    content_height: usize,
    body_start: usize,
    scrollbar_position: usize,
    image_placements: Vec<DetailBodyImagePlacement>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct DetailInlineImageContext {
    pub(crate) unavailable_hashes: HashSet<String>,
    pub(crate) focused_attachment_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailInlineImagePlacement {
    pub(crate) attachment_id: String,
    pub(crate) source_hash: String,
    pub(crate) x: u16,
    pub(crate) y: u16,
    pub(crate) width: u16,
    pub(crate) height: u16,
}

#[derive(Debug, Clone)]
struct DetailBodyImagePlacement {
    attachment_id: String,
    source_hash: String,
    line_index: usize,
    width: u16,
    height: u16,
}

#[derive(Debug, Clone)]
enum DetailBodyBlock {
    Line(Line<'static>),
    Image {
        placeholder: Line<'static>,
        attachment_id: String,
        source_hash: String,
        width: u16,
        height: u16,
    },
}

#[derive(Debug)]
struct SelectableLine {
    text: String,
    document_start: usize,
    body_index: Option<usize>,
}

#[derive(Debug)]
struct DetailSelectableDocument {
    text: String,
    title: SelectableLine,
    description: Vec<SelectableLine>,
}

pub(crate) struct DetailChildHit {
    pub(crate) task_id: crate::ids::TaskId,
}

pub(crate) struct DetailAttachmentHit {
    pub(crate) attachment_id: String,
}

pub(crate) struct DetailCopyHit {
    pub(crate) value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailMetadataTarget {
    Status,
    Priority,
}

#[allow(clippy::too_many_arguments)]
fn render_detail(
    frame: &mut Frame,
    item: &TaskListItem,
    scroll: u16,
    inline_title_editor: Option<&TextInputView>,
    hovered_child_task_id: Option<&str>,
    selection: Option<&DetailTextSelection>,
    widgets: &mut WidgetState,
    inline_images: Option<&DetailInlineImageContext>,
) {
    let layout = detail_content_layout(frame.area());
    frame.render_widget(Clear, layout.body_area);
    frame.render_widget(Block::new().style(Style::new().bg(BG)), layout.body_area);
    if layout.body_area.width == 0 || layout.body_area.height == 0 {
        return;
    }

    let selection = selection.filter(|selection| selection.terminal_width == frame.area().width);
    let model = build_detail_content_model(
        item,
        layout.content_area,
        scroll,
        inline_title_editor,
        hovered_child_task_id,
        selection,
        inline_images,
    );
    render_detail_content_from_model(frame, layout.content_area, &model, widgets);
    if layout.metadata_area.width > 0 {
        render_detail_metadata(frame, item, layout.metadata_area);
    }
}

fn detail_content_layout(frame_area: Rect) -> DetailContentLayout {
    let body = detail_body_area(frame_area);

    let [content_area, metadata_area] = if body.width >= 96 {
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(34)]).areas(body)
    } else {
        [body, Rect::default()]
    };
    let content_area = content_area.inner(detail_content_margin());
    DetailContentLayout {
        body_area: body,
        content_area,
        metadata_area,
    }
}

fn detail_body_area(frame_area: Rect) -> Rect {
    let [_, body, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(frame_area);
    body
}

fn keycap_style() -> Style {
    Style::new()
        .fg(FG)
        .bg(BG_PANEL)
        .add_modifier(Modifier::BOLD)
}

#[cfg(test)]
pub(crate) fn detail_scroll_cap(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
) -> u16 {
    detail_scroll_cap_with_images(item, terminal_width, terminal_height, None)
}

pub(crate) fn detail_scroll_cap_with_images(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    inline_images: Option<&DetailInlineImageContext>,
) -> u16 {
    let layout = detail_content_layout(Rect::new(0, 0, terminal_width, terminal_height));
    let model = build_detail_content_model(
        item,
        layout.content_area,
        0,
        None,
        None,
        None,
        inline_images,
    );
    let sticky_height = model
        .sticky_lines
        .len()
        .min(layout.content_area.height as usize);
    model.content_height.saturating_sub(
        layout
            .content_area
            .height
            .saturating_sub(sticky_height as u16) as usize,
    ) as u16
}

#[cfg(test)]
pub(crate) fn detail_section_scroll_target(
    item: &TaskListItem,
    scroll: u16,
    terminal_width: u16,
    terminal_height: u16,
    reverse: bool,
) -> u16 {
    detail_section_scroll_target_with_images(
        item,
        scroll,
        terminal_width,
        terminal_height,
        reverse,
        None,
    )
}

pub(crate) fn detail_attachment_scroll_target(
    item: &TaskListItem,
    attachment_id: &str,
    scroll: u16,
    terminal_width: u16,
    terminal_height: u16,
    inline_images: &DetailInlineImageContext,
) -> Option<u16> {
    let layout = detail_content_layout(Rect::new(0, 0, terminal_width, terminal_height));
    let model = build_detail_content_model(
        item,
        layout.content_area,
        0,
        None,
        None,
        None,
        Some(inline_images),
    );
    let sticky_height = model
        .sticky_lines
        .len()
        .min(layout.content_area.height as usize);
    let body_visible = (layout.content_area.height as usize).saturating_sub(sticky_height);
    let placement = model
        .image_placements
        .iter()
        .find(|placement| placement.attachment_id == attachment_id)?;
    let frame_start = placement.line_index.checked_sub(1)?;
    let frame_end = placement
        .line_index
        .saturating_add(placement.height as usize);
    let frame_height = frame_end.saturating_sub(frame_start).saturating_add(1);
    if frame_height > body_visible || body_visible == 0 {
        return None;
    }
    let cap = model.content_height.saturating_sub(body_visible);
    let scroll = (scroll as usize).min(cap);
    let target = if frame_start < scroll {
        frame_start
    } else if frame_end >= scroll.saturating_add(body_visible) {
        frame_end.saturating_add(1).saturating_sub(body_visible)
    } else {
        scroll
    };
    Some(target.min(cap) as u16)
}

pub(crate) fn detail_section_scroll_target_with_images(
    item: &TaskListItem,
    scroll: u16,
    terminal_width: u16,
    terminal_height: u16,
    reverse: bool,
    inline_images: Option<&DetailInlineImageContext>,
) -> u16 {
    let layout = detail_content_layout(Rect::new(0, 0, terminal_width, terminal_height));
    let model = build_detail_content_model(
        item,
        layout.content_area,
        scroll,
        None,
        None,
        None,
        inline_images,
    );
    let sticky_height = model
        .sticky_lines
        .len()
        .min(layout.content_area.height as usize);
    let visible = (layout.content_area.height as usize).saturating_sub(sticky_height);
    let scroll_cap = model.content_height.saturating_sub(visible);
    let mut targets =
        detail_section_body_indices(item, layout.content_area.width as usize, inline_images)
            .into_iter()
            .map(|index| index.min(scroll_cap) as u16)
            .collect::<Vec<_>>();
    targets.dedup();

    if reverse {
        targets
            .iter()
            .rev()
            .find(|&&target| target < model.body_start as u16)
            .copied()
            .or_else(|| targets.last().copied())
            .unwrap_or(0)
    } else {
        targets
            .iter()
            .find(|&&target| target > model.body_start as u16)
            .copied()
            .or_else(|| targets.first().copied())
            .unwrap_or(0)
    }
}

fn detail_content_margin() -> Margin {
    Margin {
        horizontal: 2,
        vertical: 1,
    }
}

fn build_detail_content_model(
    item: &TaskListItem,
    area: Rect,
    scroll: u16,
    inline_title_editor: Option<&TextInputView>,
    hovered_child_task_id: Option<&str>,
    selection: Option<&DetailTextSelection>,
    inline_images: Option<&DetailInlineImageContext>,
) -> DetailContentRenderModel {
    let mut sticky_lines = detail_header_options(item, area.width as usize, inline_title_editor);
    let (mut body_lines, image_placements) = detail_body_lines_with_images(
        item,
        area.width as usize,
        hovered_child_task_id,
        inline_images,
    );
    if inline_title_editor.is_none()
        && let Some(selection) = selection.filter(|selection| selection.task_id == item.task.id)
    {
        apply_detail_selection(
            item,
            area.width as usize,
            selection,
            inline_images,
            &mut sticky_lines,
            &mut body_lines,
        );
    }
    let content_height = body_lines.len().max(1);
    let sticky_height = sticky_lines.len().min(area.height as usize);
    let visible = (area.height as usize).saturating_sub(sticky_height);
    let start = clamp_scroll_start(scroll, content_height, visible.max(1));
    let lines = body_lines.into_iter().skip(start).collect();
    let scrollbar_position = if content_height > visible {
        scrollbar_thumb_position(start, content_height, visible.max(1))
    } else {
        0
    };
    DetailContentRenderModel {
        sticky_lines,
        lines,
        content_height,
        body_start: start,
        scrollbar_position,
        image_placements,
    }
}

fn visible_detail_image_rect(
    body_area: Rect,
    model: &DetailContentRenderModel,
    placement: &DetailBodyImagePlacement,
) -> Option<Rect> {
    let row = placement.line_index.checked_sub(model.body_start)?;
    let frame_start = row.checked_sub(1)?;
    let frame_end = row.saturating_add(placement.height as usize);
    if frame_end >= body_area.height as usize || frame_start >= body_area.height as usize {
        return None;
    }
    let width = placement.width.min(body_area.width.saturating_sub(4));
    if placement.height == 0 || width == 0 {
        return None;
    }
    Some(Rect::new(
        body_area.x.saturating_add(3),
        body_area.y.saturating_add(row as u16),
        width,
        placement.height,
    ))
}

fn render_detail_content_from_model(
    frame: &mut Frame,
    area: Rect,
    model: &DetailContentRenderModel,
    widgets: &mut WidgetState,
) {
    let visible = area.height as usize;
    let sticky_height = model.sticky_lines.len().min(visible);
    let [sticky_area, body_area] = Layout::vertical([
        Constraint::Length(sticky_height as u16),
        Constraint::Fill(1),
    ])
    .areas(area);
    frame.render_widget(
        Paragraph::new(Text::from(model.sticky_lines.clone())).style(Style::new().fg(FG).bg(BG)),
        sticky_area,
    );
    frame.render_widget(
        Paragraph::new(Text::from(model.lines.clone())).style(Style::new().fg(FG).bg(BG)),
        body_area,
    );
    let body_visible = body_area.height as usize;
    if model.content_height > body_visible {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(FG_DIM).bg(BG))
                .thumb_style(Style::new().fg(FG_MUTED)),
            body_area,
            &mut ScrollbarState::new(model.content_height)
                .position(model.scrollbar_position)
                .viewport_content_length(body_visible.max(1)),
        );
    }
    widgets
        .inline_image_placements
        .extend(model.image_placements.iter().filter_map(|placement| {
            let image = visible_detail_image_rect(body_area, model, placement)?;
            Some(DetailInlineImagePlacement {
                attachment_id: placement.attachment_id.clone(),
                source_hash: placement.source_hash.clone(),
                x: image.x,
                y: image.y,
                width: image.width,
                height: image.height,
            })
        }));
}

#[cfg(test)]
fn detail_content_lines(
    item: &TaskListItem,
    width: usize,
    inline_title_editor: Option<&TextInputView>,
) -> Vec<Line<'static>> {
    let mut lines = detail_header_options(item, width, inline_title_editor);
    lines.extend(detail_body_lines(item, width, None));
    lines
}

#[cfg(test)]
fn detail_body_lines(
    item: &TaskListItem,
    width: usize,
    hovered_child_task_id: Option<&str>,
) -> Vec<Line<'static>> {
    detail_body_lines_with_images(item, width, hovered_child_task_id, None).0
}

fn detail_body_lines_with_images(
    item: &TaskListItem,
    width: usize,
    hovered_child_task_id: Option<&str>,
    inline_images: Option<&DetailInlineImageContext>,
) -> (Vec<Line<'static>>, Vec<DetailBodyImagePlacement>) {
    let mut lines = detail_epic_child_lines(item, width, hovered_child_task_id);
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    let mut image_placements = Vec::new();
    extend_detail_body_blocks(
        &mut lines,
        &mut image_placements,
        &description_or_placeholder(&item.task.description),
        width,
        Style::new().fg(FG_MUTED),
        MarkdownRenderContext,
        inline_images,
    );
    extend_attachment_section(
        &mut lines,
        &mut image_placements,
        &item.attachments,
        width,
        inline_images,
    );
    lines.push(Line::from(""));
    lines.extend(detail_note_lines(item, width));
    lines.extend(detail_dependency_lines(item, width));
    (lines, image_placements)
}

fn detail_selectable_document(
    item: &TaskListItem,
    width: usize,
    inline_images: Option<&DetailInlineImageContext>,
) -> DetailSelectableDocument {
    let title = SelectableLine {
        text: item.task.title.clone(),
        document_start: 0,
        body_index: None,
    };
    let mut text = item.task.title.clone();
    let mut description = Vec::new();
    if !item.task.description.is_empty() {
        text.push('\n');
        let epic_lines = detail_epic_child_lines(item, width, None);
        let mut body_index = epic_lines.len() + usize::from(!epic_lines.is_empty());
        let content_width = width.saturating_sub(3).max(1);
        let blocks = detail_body_blocks(
            &item.task.description,
            content_width,
            MarkdownRenderContext,
            inline_images,
        );
        for (index, block) in blocks.into_iter().enumerate() {
            if index > 0 {
                text.push('\n');
            }
            let (line, rendered_height) = match block {
                DetailBodyBlock::Line(line) => (line, 1),
                DetailBodyBlock::Image {
                    placeholder,
                    height,
                    ..
                } => (placeholder, 1 + height as usize),
            };
            let line_text = line.to_string();
            let document_start = text.len();
            text.push_str(&line_text);
            description.push(SelectableLine {
                text: line_text,
                document_start,
                body_index: Some(body_index),
            });
            body_index += rendered_height;
        }
    }
    DetailSelectableDocument {
        text,
        title,
        description,
    }
}

fn apply_detail_selection(
    item: &TaskListItem,
    width: usize,
    selection: &DetailTextSelection,
    inline_images: Option<&DetailInlineImageContext>,
    sticky_lines: &mut [Line<'static>],
    body_lines: &mut [Line<'static>],
) {
    let document = detail_selectable_document(item, width, inline_images);
    let range = selection.range();
    if let Some(line) = sticky_lines.first_mut() {
        highlight_selectable_line(line, &document.title, &range, 0);
    }
    for selectable in &document.description {
        if let Some(line) = selectable
            .body_index
            .and_then(|index| body_lines.get_mut(index))
        {
            highlight_selectable_line(line, selectable, &range, 1);
        }
    }
}

fn highlight_selectable_line(
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
                span.style.fg(INVERSE_FG).bg(ACCENT),
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

fn detail_section_body_indices(
    item: &TaskListItem,
    width: usize,
    inline_images: Option<&DetailInlineImageContext>,
) -> Vec<usize> {
    let epic_lines = detail_epic_child_lines(item, width, None);
    let description_index = epic_lines.len() + usize::from(!epic_lines.is_empty());
    let notes_index = detail_description_lines(item, width, None, inline_images).len();
    let mut indices = if item.task.is_epic {
        vec![0, description_index, notes_index]
    } else {
        vec![description_index, notes_index]
    };
    if !item.depends_on.is_empty() || !item.blocks.is_empty() {
        indices.push(notes_index + detail_note_lines(item, width).len() + 1);
    }
    indices
}

fn detail_note_lines(item: &TaskListItem, width: usize) -> Vec<Line<'static>> {
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "NOTES",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" (", Style::new().fg(FG_DIM)),
        Span::styled("n", keycap_style()),
        Span::styled(" add)", Style::new().fg(FG_DIM)),
    ])];
    if item.notes.is_empty() {
        lines.push(Line::from(Span::styled("none", Style::new().fg(FG_MUTED))));
    } else {
        for note in &item.notes {
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled(
                    local_timestamp_display(&note.created_at),
                    Style::new().fg(FG_DIM),
                ),
                Span::styled("  you", Style::new().fg(ACCENT)),
            ]));
            lines.extend(quoted_block_lines(&note.body, width, Style::new().fg(FG)));
        }
    }
    lines
}

fn detail_description_lines(
    item: &TaskListItem,
    width: usize,
    hovered_child_task_id: Option<&str>,
    inline_images: Option<&DetailInlineImageContext>,
) -> Vec<Line<'static>> {
    let mut lines = detail_epic_child_lines(item, width, hovered_child_task_id);
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    lines.extend(quoted_block_lines_with_context(
        &description_or_placeholder(&item.task.description),
        width,
        Style::new().fg(FG_MUTED),
        MarkdownRenderContext,
    ));
    extend_attachment_section(
        &mut lines,
        &mut Vec::new(),
        &item.attachments,
        width,
        inline_images,
    );
    lines.push(Line::from(""));
    lines
}

fn detail_epic_child_lines(
    item: &TaskListItem,
    width: usize,
    hovered_child_task_id: Option<&str>,
) -> Vec<Line<'static>> {
    if !item.task.is_epic {
        return Vec::new();
    }

    let open = item
        .epic_children
        .iter()
        .filter(|link| link.unresolved)
        .count();
    let mut lines = vec![Line::from(vec![
        Span::styled(
            "CHILD TASKS",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" open={open} total={}", item.epic_children.len()),
            Style::new().fg(FG_DIM),
        ),
    ])];

    if item.epic_children.is_empty() {
        lines.push(Line::from(Span::styled("none", Style::new().fg(FG_MUTED))));
        return lines;
    }

    let links = item
        .epic_children
        .iter()
        .filter(|link| link.unresolved)
        .chain(item.epic_children.iter().filter(|link| !link.unresolved))
        .collect::<Vec<_>>();
    for (index, link) in links.iter().enumerate() {
        let is_last = index + 1 == links.len();
        let hovered = hovered_child_task_id == Some(link.task_id.as_str());
        lines.extend(epic_child_tree_item_lines(link, is_last, width, hovered));
    }

    lines
}

fn epic_child_tree_item_lines(
    link: &crate::query::TaskDependencyLink,
    is_last: bool,
    width: usize,
    hovered: bool,
) -> Vec<Line<'static>> {
    let tree_glyph = if is_last { "└─ " } else { "├─ " };
    let ref_style = if hovered {
        Style::new()
            .fg(ACCENT)
            .bg(BG_PANEL)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(ACCENT)
    };
    let title_style = if hovered {
        Style::new().fg(FG).bg(BG_PANEL)
    } else {
        Style::new().fg(FG)
    };
    let tree_style = if hovered {
        Style::new().fg(BORDER).bg(BG_PANEL)
    } else {
        Style::new().fg(BORDER)
    };
    let gap_style = if hovered {
        Style::new().fg(FG_DIM).bg(BG_PANEL)
    } else {
        Style::new().fg(FG_DIM)
    };
    let prefix = vec![
        Span::styled(tree_glyph, tree_style),
        Span::styled(link.display_ref.clone(), ref_style),
        Span::styled("  ", gap_style),
    ];
    dependency_node_lines_with_title_style(
        prefix,
        &link.title,
        &link.status,
        &link.priority,
        width,
        title_style,
    )
}

fn detail_dependency_lines(item: &TaskListItem, width: usize) -> Vec<Line<'static>> {
    if item.depends_on.is_empty() && item.blocks.is_empty() {
        return Vec::new();
    }

    let mut lines = vec![Line::from("")];

    if !item.depends_on.is_empty() {
        lines.push(dependency_heading("WHY BLOCKED", &item.depends_on));
        lines.extend(dependency_branch_lines(
            &item.depends_on,
            DependencyDirection::Blocker,
            width,
        ));
    }

    if !item.blocks.is_empty() {
        lines.push(Line::from(""));
        lines.push(dependency_heading("WHAT THIS UNLOCKS", &item.blocks));
        lines.extend(dependency_branch_lines(
            &item.blocks,
            DependencyDirection::Dependent,
            width,
        ));
    }

    lines
}

fn dependency_branch_lines(
    links: &[crate::query::TaskDependencyLink],
    direction: DependencyDirection,
    width: usize,
) -> Vec<Line<'static>> {
    let visible = links.len().min(DETAIL_DEPENDENCY_TREE_CAP);
    let hidden = links.len().saturating_sub(visible);
    let rendered_len = visible + usize::from(hidden > 0);
    let mut lines = Vec::with_capacity(rendered_len);

    for (index, link) in links.iter().take(visible).enumerate() {
        let is_last = index + 1 == rendered_len;
        lines.extend(dependency_tree_item_lines(link, direction, is_last, width));
    }

    if hidden > 0 {
        lines.push(Line::from(vec![
            Span::styled("└─ ", Style::new().fg(BORDER)),
            Span::styled(format!("+{hidden} more"), Style::new().fg(FG_MUTED)),
        ]));
    }

    lines
}

fn dependency_heading(
    label: &'static str,
    links: &[crate::query::TaskDependencyLink],
) -> Line<'static> {
    let open = links.iter().filter(|link| link.unresolved).count();
    Line::from(vec![
        Span::styled(label, Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD)),
        Span::styled(
            format!(" open={open} total={}", links.len()),
            Style::new().fg(FG_DIM),
        ),
    ])
}

fn dependency_tree_item_lines(
    link: &crate::query::TaskDependencyLink,
    direction: DependencyDirection,
    is_last: bool,
    width: usize,
) -> Vec<Line<'static>> {
    let tree_glyph = if is_last { "└─ " } else { "├─ " };
    let prefix = vec![
        Span::styled(tree_glyph, Style::new().fg(BORDER)),
        Span::styled(direction.marker(), Style::new().fg(FG_DIM)),
        Span::styled(" ", Style::new().fg(FG_DIM)),
        Span::styled(link.display_ref.clone(), Style::new().fg(ACCENT)),
        Span::styled("  ", Style::new().fg(FG_DIM)),
    ];
    dependency_node_lines(prefix, &link.title, &link.status, &link.priority, width)
}

fn dependency_node_lines(
    spans: Vec<Span<'static>>,
    title: &str,
    status: &str,
    priority: &str,
    width: usize,
) -> Vec<Line<'static>> {
    dependency_node_lines_with_title_style(
        spans,
        title,
        status,
        priority,
        width,
        Style::new().fg(FG),
    )
}

fn dependency_node_lines_with_title_style(
    mut spans: Vec<Span<'static>>,
    title: &str,
    status: &str,
    priority: &str,
    width: usize,
    title_style: Style,
) -> Vec<Line<'static>> {
    let title_width = dependency_title_width(&spans, status, priority, width);
    if title_width > 0 {
        spans.push(Span::styled(
            truncate_width(title, title_width),
            title_style,
        ));
        spans.push(Span::styled("  ", Style::new().fg(FG_DIM)));
        spans.push(status_span(status));
        spans.push(Span::styled("  ", Style::new().fg(FG_DIM)));
        spans.push(Span::styled(
            priority_short(priority),
            theme::priority_style(priority).add_modifier(Modifier::BOLD),
        ));
        return vec![Line::from(spans)];
    }

    let continuation_prefix = dependency_continuation_prefix(&spans);
    let mut lines = vec![Line::from(spans)];
    lines.push(Line::from(vec![
        Span::styled(continuation_prefix.clone(), Style::new().fg(BORDER)),
        Span::styled(
            truncate_width(title, width.saturating_sub(continuation_prefix.width())),
            title_style,
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled(continuation_prefix.clone(), Style::new().fg(BORDER)),
        status_span(status),
        Span::styled("  ", Style::new().fg(FG_DIM)),
        Span::styled(
            priority_short(priority),
            theme::priority_style(priority).add_modifier(Modifier::BOLD),
        ),
    ]));
    lines
}

fn dependency_continuation_prefix(prefix: &[Span<'static>]) -> String {
    " ".repeat(prefix.iter().map(Span::width).sum::<usize>().min(4))
}

fn dependency_title_width(
    prefix: &[Span<'static>],
    status: &str,
    priority: &str,
    width: usize,
) -> usize {
    let prefix_width: usize = prefix.iter().map(Span::width).sum();
    let trailing_width = 4 + status_span(status).width() + priority_short(priority).width();
    width.saturating_sub(prefix_width + trailing_width)
}

fn detail_header_options(
    item: &TaskListItem,
    width: usize,
    inline_title_editor: Option<&TextInputView>,
) -> Vec<Line<'static>> {
    let mut summary_spans = vec![Span::styled(
        item.display_ref.clone(),
        Style::new().fg(FG_DIM),
    )];
    if item.task.is_epic {
        summary_spans.extend([
            Span::styled("  ", Style::new().fg(FG_DIM)),
            Span::styled(EPIC_MARKER, Style::new().fg(YELLOW)),
        ]);
    }
    summary_spans.extend([
        Span::styled("   ", Style::new().fg(FG_DIM)),
        status_span(item.task.status.as_str()),
        Span::styled("   ", Style::new().fg(FG_DIM)),
        Span::styled(
            priority_short(item.task.priority.as_str()),
            theme::priority_style(item.task.priority.as_str()).add_modifier(Modifier::BOLD),
        ),
    ]);
    let mut lines = vec![
        detail_title_line(item, width, inline_title_editor),
        Line::from(Span::styled("─".repeat(width), Style::new().fg(BORDER))),
        Line::from(summary_spans),
    ];
    if let Some(parent) = &item.epic_parent {
        lines.push(detail_epic_parent_line(parent, width));
    }
    lines.push(Line::from(""));
    lines
}

fn detail_epic_parent_line(
    parent: &crate::query::TaskDependencyLink,
    width: usize,
) -> Line<'static> {
    let mut spans = vec![
        Span::styled("part of ", Style::new().fg(FG_DIM)),
        Span::styled(EPIC_MARKER, Style::new().fg(YELLOW)),
        Span::styled(" ", Style::new().fg(FG_DIM)),
        Span::styled(
            parent.display_ref.clone(),
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" ", Style::new().fg(FG_DIM)),
    ];
    let used_width = spans.iter().map(Span::width).sum::<usize>();
    spans.push(Span::styled(
        truncate_width(&parent.title, width.saturating_sub(used_width)),
        Style::new().fg(FG_MUTED),
    ));
    Line::from(spans)
}

fn detail_title_line(
    item: &TaskListItem,
    width: usize,
    inline_title_editor: Option<&TextInputView>,
) -> Line<'static> {
    if let Some(editor) = inline_title_editor {
        let mut line = clipped_input_line(&editor.input, editor.cursor, width);
        for span in &mut line.spans {
            span.style = Style::new()
                .fg(FG)
                .add_modifier(Modifier::BOLD)
                .patch(span.style);
        }
        return line;
    }

    Line::from(vec![Span::styled(
        item.task.title.clone(),
        Style::new().fg(FG).add_modifier(Modifier::BOLD),
    )])
}

fn quoted_block_lines(body: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(3).max(1);
    render_markdown(body, content_width)
        .into_iter()
        .map(|line| {
            let mut spans = line_with_base_style(line, style).spans;
            spans.insert(0, Span::styled("│ ", Style::new().fg(BORDER)));
            Line::from(spans)
        })
        .collect()
}

fn quoted_block_lines_with_context(
    body: &str,
    width: usize,
    style: Style,
    context: MarkdownRenderContext,
) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(3).max(1);
    flatten_markdown_blocks(render_markdown_with_context(body, content_width, context))
        .into_iter()
        .map(|line| quoted_line(line, style))
        .collect()
}

fn extend_detail_body_blocks(
    lines: &mut Vec<Line<'static>>,
    placements: &mut Vec<DetailBodyImagePlacement>,
    body: &str,
    width: usize,
    style: Style,
    context: MarkdownRenderContext,
    inline_images: Option<&DetailInlineImageContext>,
) {
    let content_width = width.saturating_sub(3).max(1);
    for block in detail_body_blocks(body, content_width, context, inline_images) {
        match block {
            DetailBodyBlock::Line(line) => lines.push(quoted_line(line, style)),
            DetailBodyBlock::Image {
                placeholder,
                attachment_id,
                source_hash,
                width,
                height,
            } => {
                let line_index = lines.len().saturating_add(1);
                lines.push(quoted_line(placeholder, style));
                let blank_count = height as usize;
                for _ in 0..blank_count {
                    lines.push(Line::from(vec![Span::styled(
                        "│ ",
                        Style::new().fg(BORDER),
                    )]));
                }
                placements.push(DetailBodyImagePlacement {
                    attachment_id,
                    source_hash,
                    line_index,
                    width,
                    height,
                });
            }
        }
    }
}

fn extend_attachment_section(
    lines: &mut Vec<Line<'static>>,
    placements: &mut Vec<DetailBodyImagePlacement>,
    attachments: &[AttachmentMetadataJson],
    width: usize,
    inline_images: Option<&DetailInlineImageContext>,
) {
    let live = attachments
        .iter()
        .filter(|attachment| !attachment.deleted)
        .collect::<Vec<_>>();
    if live.is_empty() {
        return;
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "ATTACHMENTS",
        Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
    )));
    let content_width = width.saturating_sub(3).max(1);
    for attachment in live {
        match attachment_detail_block(attachment, content_width, inline_images) {
            DetailBodyBlock::Line(line) => {
                lines.push(quoted_line(line, Style::new().fg(FG_MUTED)));
            }
            DetailBodyBlock::Image {
                placeholder,
                attachment_id,
                source_hash,
                width,
                height,
            } => {
                let focused = inline_images.is_some_and(|context| {
                    context.focused_attachment_id.as_deref() == Some(attachment_id.as_str())
                });
                let frame_style = Style::new().fg(if focused { ACCENT } else { BORDER });
                lines.push(quoted_line(placeholder, Style::new().fg(FG_MUTED)));
                lines.push(quoted_line(
                    Line::from(format!("┌{}┐", "─".repeat(width as usize))),
                    frame_style,
                ));
                let line_index = lines.len();
                for _ in 0..height {
                    lines.push(quoted_line(
                        Line::from(vec![
                            Span::styled("│", frame_style),
                            Span::raw(" ".repeat(width as usize)),
                            Span::styled("│", frame_style),
                        ]),
                        Style::new().fg(FG_MUTED),
                    ));
                }
                lines.push(quoted_line(
                    Line::from(format!("└{}┘", "─".repeat(width as usize))),
                    frame_style,
                ));
                placements.push(DetailBodyImagePlacement {
                    attachment_id,
                    source_hash,
                    line_index,
                    width,
                    height,
                });
            }
        }
    }
}

pub(crate) fn attachment_is_locally_previewable(
    attachment: &AttachmentMetadataJson,
    unavailable_hashes: &HashSet<String>,
) -> bool {
    !attachment.deleted
        && attachment.has_blob
        && attachment.bytes_state == crate::attachments::AttachmentBytesState::Present
        && attachment.media_type.starts_with("image/")
        && !unavailable_hashes.contains(&attachment.sha256)
        && matches!(
            (attachment.width, attachment.height),
            (Some(width), Some(height)) if width > 0 && height > 0
        )
}

fn attachment_detail_block(
    attachment: &AttachmentMetadataJson,
    content_width: usize,
    inline_images: Option<&DetailInlineImageContext>,
) -> DetailBodyBlock {
    let placeholder = Line::from(attachment_placeholder(attachment));
    let Some(inline_images) = inline_images else {
        return DetailBodyBlock::Line(placeholder);
    };
    if !attachment_is_locally_previewable(attachment, &inline_images.unavailable_hashes) {
        return DetailBodyBlock::Line(placeholder);
    }
    let (width, height) = image_preview_size(attachment, content_width.saturating_sub(4));
    DetailBodyBlock::Image {
        placeholder,
        attachment_id: attachment.attachment_id.clone(),
        source_hash: attachment.sha256.clone(),
        width,
        height,
    }
}

fn detail_body_blocks(
    body: &str,
    content_width: usize,
    context: MarkdownRenderContext,
    _inline_images: Option<&DetailInlineImageContext>,
) -> Vec<DetailBodyBlock> {
    render_markdown_with_context(body, content_width, context)
        .into_iter()
        .map(|block| match block {
            MarkdownBlock::Text(line) => DetailBodyBlock::Line(line),
        })
        .collect()
}

fn image_preview_size(attachment: &AttachmentMetadataJson, content_width: usize) -> (u16, u16) {
    const MAX_HEIGHT_ROWS: u16 = 12;
    const DEFAULT_HEIGHT_ROWS: u16 = 6;
    const CELL_HEIGHT_TO_WIDTH_RATIO: f64 = 2.0;

    let max_width = content_width.clamp(1, u16::MAX as usize) as u16;
    match (attachment.width, attachment.height) {
        (Some(width), Some(height)) if width > 0 && height > 0 => {
            let image_aspect = width as f64 / height as f64;
            let width_at_max_height =
                (MAX_HEIGHT_ROWS as f64 * image_aspect * CELL_HEIGHT_TO_WIDTH_RATIO)
                    .round()
                    .max(1.0) as u16;
            if width_at_max_height <= max_width {
                (width_at_max_height, MAX_HEIGHT_ROWS)
            } else {
                let height = ((max_width as f64 / image_aspect) / CELL_HEIGHT_TO_WIDTH_RATIO)
                    .round()
                    .clamp(3.0, MAX_HEIGHT_ROWS as f64) as u16;
                (max_width, height)
            }
        }
        _ => (max_width.min(80), DEFAULT_HEIGHT_ROWS),
    }
}

fn quoted_line(line: Line<'static>, style: Style) -> Line<'static> {
    let mut spans = line_with_base_style(line, style).spans;
    spans.insert(0, Span::styled("│ ", Style::new().fg(BORDER)));
    Line::from(spans)
}

fn line_with_base_style(mut line: Line<'static>, base: Style) -> Line<'static> {
    for span in &mut line.spans {
        span.style = base.patch(span.style);
    }
    line
}

fn render_detail_metadata(frame: &mut Frame, item: &TaskListItem, area: Rect) {
    let block = Block::new()
        .borders(Borders::LEFT)
        .border_style(Style::new().fg(BORDER))
        .padding(Padding::horizontal(1))
        .style(Style::new().bg(BG));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(Text::from(detail_metadata_lines(
            item,
            inner.width as usize,
        )))
        .style(Style::new().fg(FG).bg(BG)),
        inner,
    );
}

fn detail_metadata_lines(item: &TaskListItem, width: usize) -> Vec<Line<'static>> {
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
    ];
    let now_seconds = crate::queue::now_seconds();
    if let Some([relative, local]) = crate::tui::time::availability_summary_lines(
        item.task.available_at.as_deref().unwrap_or(""),
        item.queue.band == crate::queue::QueueBand::Available,
        now_seconds,
    ) {
        let value_style = Style::new().fg(ACCENT).add_modifier(Modifier::BOLD);
        lines.extend([
            Line::from(""),
            metadata_label("AVAILABILITY"),
            Line::from(Span::styled(truncate_width(&relative, width), value_style)),
            Line::from(Span::styled(truncate_width(&local, width), value_style)),
        ]);
    }
    if let Some([relative, date]) =
        crate::tui::time::due_summary_lines(item.task.due_on.as_deref().unwrap_or(""), now_seconds)
    {
        let color = if !item.task.status.is_open() {
            FG_MUTED
        } else {
            match crate::tui::time::due_state_at(
                item.task.due_on.as_deref().unwrap_or(""),
                now_seconds,
            ) {
                crate::due::DueState::Overdue(_) => RED,
                crate::due::DueState::Today => ORANGE,
                crate::due::DueState::Future(_) => ACCENT,
                crate::due::DueState::None => FG_MUTED,
            }
        };
        let value_style = Style::new().fg(color).add_modifier(Modifier::BOLD);
        lines.extend([
            Line::from(""),
            metadata_label("DUE"),
            Line::from(Span::styled(truncate_width(&relative, width), value_style)),
            Line::from(Span::styled(truncate_width(&date, width), value_style)),
        ]);
    }
    lines.extend(detail_epic_metadata_lines(item));
    if item.has_conflict {
        lines.extend([
            Line::from(""),
            metadata_label("CONFLICTS"),
            Line::from(Span::styled(
                "yes",
                Style::new().fg(ORANGE).add_modifier(Modifier::BOLD),
            )),
        ]);
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

fn detail_epic_metadata_lines(item: &TaskListItem) -> Vec<Line<'static>> {
    if !item.task.is_epic {
        return Vec::new();
    }

    let open = item
        .epic_children
        .iter()
        .filter(|link| link.unresolved)
        .count();
    let lines = vec![
        Line::from(""),
        metadata_label("CHILDREN"),
        Line::from(Span::styled(
            format!("open={open} total={}", item.epic_children.len()),
            Style::new().fg(FG_DIM),
        )),
    ];

    if item.epic_children.is_empty() {
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

fn metadata_label(label: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        label,
        Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
    ))
}

pub(crate) fn detail_text_cell_at_position(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    column: u16,
    row: u16,
    scroll: u16,
) -> Option<TextCell> {
    let layout = detail_content_layout(Rect::new(0, 0, terminal_width, terminal_height));
    if column < layout.body_area.x
        || column
            >= layout
                .content_area
                .x
                .saturating_add(layout.content_area.width)
        || row < layout.content_area.y
        || row
            >= layout
                .content_area
                .y
                .saturating_add(layout.content_area.height)
    {
        return None;
    }
    let model =
        build_detail_content_model(item, layout.content_area, scroll, None, None, None, None);
    let document = detail_selectable_document(item, layout.content_area.width as usize, None);

    let selectable = if row == layout.content_area.y {
        &document.title
    } else {
        let sticky_height = model
            .sticky_lines
            .len()
            .min(layout.content_area.height as usize) as u16;
        let body_y = layout.content_area.y.saturating_add(sticky_height);
        if row < body_y || row >= body_y.saturating_add(layout.content_area.height) {
            return None;
        }
        let body_index = model
            .body_start
            .saturating_add(row.saturating_sub(body_y) as usize);
        document
            .description
            .iter()
            .find(|line| line.body_index == Some(body_index))?
    };

    let text_x = layout
        .content_area
        .x
        .saturating_add(u16::from(selectable.body_index.is_some()) * 2);
    let cell_column = column.saturating_sub(text_x) as usize;
    let local = text_cell_at_column(&selectable.text, cell_column).or_else(|| {
        let edge_column = if column <= text_x {
            0
        } else {
            selectable.text.width().checked_sub(1)?
        };
        text_cell_at_column(&selectable.text, edge_column)
    })?;
    Some(TextCell {
        start: selectable.document_start + local.start,
        end: selectable.document_start + local.end,
    })
}

pub(crate) fn detail_selected_text(
    item: &TaskListItem,
    selection: &DetailTextSelection,
) -> Option<String> {
    if selection.task_id != item.task.id {
        return None;
    }
    let layout = detail_content_layout(Rect::new(0, 0, selection.terminal_width, 24));
    let document = detail_selectable_document(item, layout.content_area.width as usize, None);
    document.text.get(selection.range()).map(str::to_string)
}

pub(crate) fn detail_attachment_at_position(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    column: u16,
    row: u16,
    scroll: u16,
    inline_images: &DetailInlineImageContext,
) -> Option<DetailAttachmentHit> {
    let layout = detail_content_layout(Rect::new(0, 0, terminal_width, terminal_height));
    let model = build_detail_content_model(
        item,
        layout.content_area,
        scroll,
        None,
        None,
        None,
        Some(inline_images),
    );
    let sticky_height = model
        .sticky_lines
        .len()
        .min(layout.content_area.height as usize) as u16;
    let body_area = Rect::new(
        layout.content_area.x,
        layout.content_area.y.saturating_add(sticky_height),
        layout.content_area.width,
        layout.content_area.height.saturating_sub(sticky_height),
    );
    for placement in &model.image_placements {
        if inline_images
            .unavailable_hashes
            .contains(&placement.source_hash)
        {
            continue;
        }
        let Some(image) = visible_detail_image_rect(body_area, &model, placement) else {
            continue;
        };
        let frame = Rect::new(
            image.x.saturating_sub(1),
            image.y.saturating_sub(1),
            image.width.saturating_add(2),
            image.height.saturating_add(2),
        );
        if column >= frame.x
            && column < frame.x.saturating_add(frame.width)
            && row >= frame.y
            && row < frame.y.saturating_add(frame.height)
            && row < body_area.y.saturating_add(body_area.height)
        {
            return Some(DetailAttachmentHit {
                attachment_id: placement.attachment_id.clone(),
            });
        }
    }
    None
}

pub(crate) fn detail_child_task_at_position(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    column: u16,
    row: u16,
    scroll: u16,
) -> Option<DetailChildHit> {
    let layout = detail_content_layout(Rect::new(0, 0, terminal_width, terminal_height));
    if column < layout.content_area.x
        || column
            >= layout
                .content_area
                .x
                .saturating_add(layout.content_area.width)
        || row < layout.content_area.y
        || row
            >= layout
                .content_area
                .y
                .saturating_add(layout.content_area.height)
    {
        return None;
    }
    let model =
        build_detail_content_model(item, layout.content_area, scroll, None, None, None, None);
    let sticky_height = model
        .sticky_lines
        .len()
        .min(layout.content_area.height as usize) as u16;
    let body_y = layout.content_area.y.saturating_add(sticky_height);
    if row < body_y {
        return None;
    }
    let body_row = row.saturating_sub(body_y) as usize;
    let body_index = model.body_start.saturating_add(body_row);
    let task_id = detail_epic_child_task_id_at_body_line(
        item,
        layout.content_area.width as usize,
        body_index,
    )?;
    Some(DetailChildHit { task_id })
}

fn detail_epic_child_task_id_at_body_line(
    item: &TaskListItem,
    width: usize,
    body_line_index: usize,
) -> Option<crate::ids::TaskId> {
    if !item.task.is_epic || body_line_index == 0 {
        return None;
    }
    let links = item
        .epic_children
        .iter()
        .filter(|link| link.unresolved)
        .chain(item.epic_children.iter().filter(|link| !link.unresolved))
        .collect::<Vec<_>>();
    let mut line_index = 1;
    for (index, link) in links.iter().enumerate() {
        let is_last = index + 1 == links.len();
        let line_count = epic_child_tree_item_lines(link, is_last, width, false).len();
        if (line_index..line_index + line_count).contains(&body_line_index) {
            return Some(link.task_id.clone());
        }
        line_index += line_count;
    }
    None
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
    let value = match line {
        15 => item.display_ref.clone(),
        18 => local_timestamp_display(&item.task.created_at),
        21 => local_timestamp_display(&item.task.updated_at),
        _ => return None,
    };
    let value_start = layout.metadata_area.x.saturating_add(2);
    let value_end = value_start.saturating_add(UnicodeWidthStr::width(value.as_str()) as u16);
    (column >= value_start && column < value_end).then_some(DetailCopyHit { value })
}

pub(crate) fn detail_metadata_target_at(
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
    let target = match line {
        6 => DetailMetadataTarget::Status,
        9 => DetailMetadataTarget::Priority,
        _ => return None,
    };
    Some((target, column, row))
}

fn metadata_content_row(metadata_area: Rect, body: Rect, column: u16, row: u16) -> Option<u16> {
    if column <= metadata_area.x
        || column >= metadata_area.x.saturating_add(metadata_area.width)
        || row < body.y
        || row >= body.y.saturating_add(body.height)
    {
        return None;
    }
    Some(row.saturating_sub(body.y))
}

pub(crate) fn render_attachment_preview(
    frame: &mut Frame,
    item: &TaskListItem,
    attachment_id: &str,
    widgets: &mut WidgetState,
    inline_images: Option<&DetailInlineImageContext>,
) {
    let area = detail_body_area(frame.area());
    frame.render_widget(Clear, area);
    let Some(attachment) = item.attachments.iter().find(|attachment| {
        attachment.attachment_id == attachment_id
            && !attachment.deleted
            && attachment.has_blob
            && attachment.bytes_state == crate::attachments::AttachmentBytesState::Present
            && attachment.media_type.starts_with("image/")
            && matches!(
                (attachment.width, attachment.height),
                (Some(width), Some(height)) if width > 0 && height > 0
            )
    }) else {
        frame.render_widget(
            Paragraph::new("attachment is unavailable")
                .alignment(ratatui::layout::Alignment::Center)
                .style(Style::new().fg(FG_MUTED).bg(BG)),
            area,
        );
        return;
    };
    let title = attachment
        .filename
        .as_deref()
        .or(attachment.alt_text.as_deref())
        .unwrap_or("Image preview");
    let block = Block::new()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(ACCENT))
        .title(format!(" {title} "))
        .style(Style::new().bg(BG));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let Some(inline_images) = inline_images else {
        frame.render_widget(
            Paragraph::new("preview unavailable")
                .alignment(ratatui::layout::Alignment::Center)
                .style(Style::new().fg(FG_MUTED).bg(BG)),
            inner,
        );
        return;
    };
    if inline_images
        .unavailable_hashes
        .contains(&attachment.sha256)
    {
        frame.render_widget(
            Paragraph::new("preview unavailable")
                .alignment(ratatui::layout::Alignment::Center)
                .style(Style::new().fg(FG_MUTED).bg(BG)),
            inner,
        );
        return;
    }
    let (width, height) = fitted_image_size(attachment, inner.width, inner.height);
    if width == 0 || height == 0 {
        return;
    }
    let x = inner
        .x
        .saturating_add(inner.width.saturating_sub(width) / 2);
    let y = inner
        .y
        .saturating_add(inner.height.saturating_sub(height) / 2);
    widgets
        .inline_image_placements
        .push(DetailInlineImagePlacement {
            attachment_id: attachment.attachment_id.clone(),
            source_hash: attachment.sha256.clone(),
            x,
            y,
            width,
            height,
        });
}

fn fitted_image_size(
    attachment: &AttachmentMetadataJson,
    max_width: u16,
    max_height: u16,
) -> (u16, u16) {
    const CELL_HEIGHT_TO_WIDTH_RATIO: f64 = 2.0;
    let (Some(pixel_width), Some(pixel_height)) = (attachment.width, attachment.height) else {
        return (max_width, max_height);
    };
    if pixel_width <= 0 || pixel_height <= 0 || max_width == 0 || max_height == 0 {
        return (0, 0);
    }
    let aspect = pixel_width as f64 / pixel_height as f64;
    let width_at_max_height =
        (max_height as f64 * aspect * CELL_HEIGHT_TO_WIDTH_RATIO).round() as u16;
    if width_at_max_height <= max_width {
        (width_at_max_height.max(1), max_height)
    } else {
        let height = ((max_width as f64 / aspect) / CELL_HEIGHT_TO_WIDTH_RATIO).round() as u16;
        (max_width, height.max(1).min(max_height))
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_detail_underlay(
    frame: &mut Frame,
    store: &TuiStore,
    widgets: &mut WidgetState,
    scroll: u16,
    inline_title_editor: Option<&TextInputView>,
    hovered_child_task_id: Option<&str>,
    selection: Option<&DetailTextSelection>,
    inline_images: Option<&DetailInlineImageContext>,
) {
    if let Some(task) = store.selected_task(widgets.table.selected()) {
        render_detail(
            frame,
            task,
            scroll,
            inline_title_editor,
            hovered_child_task_id,
            selection,
            widgets,
            inline_images,
        );
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::widgets::{ListState, TableState};

    use super::*;
    use crate::choices::{TaskPriority, TaskStatus};

    #[test]
    fn detail_content_includes_notes() {
        let item = detail_test_item();
        let rendered = detail_content_lines(&item, 60, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Fix token refresh race"));
        assert!(rendered.contains("Confirmed race in useTokenRefresh.ts"));
        assert!(!rendered.contains("2026-06-20T12:00:00Z"));
    }

    #[test]
    fn detail_header_marks_epics_with_star() {
        let mut item = detail_test_item();
        item.task.is_epic = true;

        let lines = detail_header_options(&item, 60, None);
        let marker = lines[2]
            .spans
            .iter()
            .find(|span| span.content == EPIC_MARKER)
            .expect("epic marker");

        assert_eq!(marker.style.fg, Some(YELLOW));
    }

    #[test]
    fn detail_header_shows_epic_parent_context() {
        let mut item = detail_test_item();
        item.epic_parent = Some(crate::query::TaskDependencyLink {
            task_id: crate::test_support::task_id("epic-task-id"),
            display_ref: "APP-EPIC".to_string(),
            title: "Ship authentication reliability".to_string(),
            status: "active".to_string(),
            priority: "high".to_string(),
            unresolved: true,
        });

        let lines = detail_header_options(&item, 60, None);

        assert_eq!(
            lines[3].to_string(),
            format!("part of {EPIC_MARKER} APP-EPIC Ship authentication reliability")
        );
        assert_eq!(lines[4].to_string(), "");
    }

    #[test]
    fn detail_epic_parent_context_truncates_to_header_width() {
        let mut item = detail_test_item();
        item.epic_parent = Some(crate::query::TaskDependencyLink {
            task_id: crate::test_support::task_id("epic-task-id"),
            display_ref: "APP-EPIC".to_string(),
            title: "A long epic title that must fit the sticky header".to_string(),
            status: "active".to_string(),
            priority: "high".to_string(),
            unresolved: true,
        });

        let line = detail_header_options(&item, 32, None).remove(3);

        assert!(line.width() <= 32);
        assert!(line.to_string().ends_with('…'));
    }

    #[test]
    fn detail_content_renders_dependency_tree() {
        let item = detail_test_item();
        let rendered = detail_content_lines(&item, 80, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("WHY BLOCKED open=1 total=1"));
        assert!(rendered.contains("└─ ← APP-7KQ1"));
        assert!(rendered.contains("Ship auth service"));
        assert!(rendered.contains("WHAT THIS UNLOCKS open=1 total=1"));
        assert!(rendered.contains("└─ → APP-7KQ2"));
        assert!(rendered.contains("Write rollout notes"));
    }

    #[test]
    fn detail_dependency_tree_caps_long_blockers() {
        let mut item = detail_test_item();
        item.depends_on = (0..5)
            .map(|index| crate::query::TaskDependencyLink {
                task_id: crate::test_support::task_id(&format!("blocker-id-{index}")),
                display_ref: format!("APP-B{index}"),
                title: format!("blocker {index}"),
                status: "todo".to_string(),
                priority: "medium".to_string(),
                unresolved: true,
            })
            .collect();
        item.blocks.clear();

        let rendered = detail_content_lines(&item, 80, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("├─ ← APP-B0"));
        assert!(rendered.contains("├─ ← APP-B2"));
        assert!(rendered.contains("└─ +2 more"));
        assert!(!rendered.contains("APP-B3"));
    }

    #[test]
    fn detail_dependency_tree_caps_long_dependents() {
        let mut item = detail_test_item();
        item.blocks = (0..5)
            .map(|index| crate::query::TaskDependencyLink {
                task_id: crate::test_support::task_id(&format!("dependent-id-{index}")),
                display_ref: format!("APP-D{index}"),
                title: format!("dependent {index}"),
                status: "inbox".to_string(),
                priority: "low".to_string(),
                unresolved: true,
            })
            .collect();
        item.depends_on.clear();

        let rendered = detail_content_lines(&item, 80, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("├─ → APP-D0"));
        assert!(rendered.contains("├─ → APP-D2"));
        assert!(rendered.contains("└─ +2 more"));
        assert!(!rendered.contains("APP-D3"));
    }

    #[test]
    fn detail_dependency_tree_truncates_titles_in_narrow_width() {
        let mut item = detail_test_item();
        item.depends_on[0].title =
            "A very long title that should fit the dependency row".to_string();

        let rendered = detail_content_lines(&item, 40, None);

        let blocker_line = rendered
            .iter()
            .find(|line| line.to_string().contains("APP-7KQ1"))
            .expect("blocker line rendered");

        assert!(
            blocker_line.width() <= 40,
            "line width {} exceeded 40",
            blocker_line.width()
        );
        assert!(blocker_line.to_string().contains('…'));
    }

    #[test]
    fn detail_dependency_tree_stacks_when_narrow() {
        let mut item = detail_test_item();
        item.depends_on[0].title =
            "A very long title that should stack in the dependency tree".to_string();

        let rendered = detail_dependency_lines(&item, 20);
        let rendered_text = rendered
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered_text.contains("└─ ← APP-7KQ1"));
        assert!(rendered_text.contains("A very long tit…"));
        assert!(rendered_text.contains("□ todo  ● high"));
        for line in rendered.into_iter().filter(|line| {
            let text = line.to_string();
            text.contains("APP-") || text.contains("todo")
        }) {
            assert!(
                line.width() <= 20,
                "line width {} exceeded 20: {line:?}",
                line.width()
            );
        }
    }

    #[test]
    fn detail_dependency_tree_is_omitted_without_links() {
        let mut item = detail_test_item();
        item.depends_on.clear();
        item.blocks.clear();

        let rendered = detail_content_lines(&item, 60, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("WHY BLOCKED"));
        assert!(!rendered.contains("WHAT THIS UNLOCKS"));
    }

    #[test]
    fn detail_text_mapping_handles_wide_title_characters() {
        let mut item = detail_test_item();
        item.task.title = "A界B".to_string();

        let first_wide_cell = detail_text_cell_at_position(&item, 80, 24, 3, 3, 0).unwrap();
        let second_wide_cell = detail_text_cell_at_position(&item, 80, 24, 4, 3, 0).unwrap();
        let selection = DetailTextSelection::new(item.task.id.clone(), 80, first_wide_cell);

        assert_eq!(first_wide_cell, second_wide_cell);
        assert_eq!(
            detail_selected_text(&item, &selection).as_deref(),
            Some("界")
        );
        let trailing_space = detail_text_cell_at_position(&item, 80, 24, 70, 3, 0).unwrap();
        let trailing_selection = DetailTextSelection::new(item.task.id.clone(), 80, trailing_space);
        assert_eq!(
            detail_selected_text(&item, &trailing_selection).as_deref(),
            Some("B")
        );

        item.task.title = "x".repeat(200);
        assert_eq!(detail_text_cell_at_position(&item, 120, 30, 88, 3, 0), None);
    }

    #[test]
    fn detail_text_mapping_uses_scrolled_wrapped_description_lines() {
        let mut item = detail_test_item();
        item.task.description = "first paragraph with enough words to wrap across terminal lines and keep going for another line".to_string();
        let layout = detail_content_layout(Rect::new(0, 0, 70, 12));
        let document = detail_selectable_document(&item, layout.content_area.width as usize, None);
        assert!(document.description.len() > 1);
        let expected = document.description[1]
            .text
            .chars()
            .next()
            .unwrap()
            .to_string();
        let body_y = layout.content_area.y + 4;

        let cell =
            detail_text_cell_at_position(&item, 70, 12, layout.content_area.x + 2, body_y, 1)
                .unwrap();
        let selection = DetailTextSelection::new(item.task.id.clone(), 70, cell);

        assert_eq!(
            detail_selected_text(&item, &selection).as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn detail_selection_copies_rendered_markdown_text() {
        let mut item = detail_test_item();
        item.task.description = "**bold** and `code`".to_string();
        let layout = detail_content_layout(Rect::new(0, 0, 80, 24));
        let document = detail_selectable_document(&item, layout.content_area.width as usize, None);
        let description = document.description.first().unwrap();
        let selection = DetailTextSelection {
            task_id: item.task.id.clone(),
            terminal_width: 80,
            anchor: TextCell {
                start: description.document_start,
                end: description.document_start + 1,
            },
            focus: TextCell {
                start: description.document_start + description.text.len() - 1,
                end: description.document_start + description.text.len(),
            },
        };

        assert_eq!(description.text, "bold and code");
        assert_eq!(
            detail_selected_text(&item, &selection).as_deref(),
            Some("bold and code")
        );
    }

    #[test]
    fn detail_selection_highlights_the_selected_range() {
        let item = detail_test_item();
        let selection =
            DetailTextSelection::new(item.task.id.clone(), 80, TextCell { start: 0, end: 3 });

        let model = build_detail_content_model(
            &item,
            Rect::new(2, 3, 76, 18),
            0,
            None,
            None,
            Some(&selection),
            None,
        );
        let selected = model.sticky_lines[0]
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "Fix")
            .unwrap();

        assert_eq!(selected.style.bg, Some(ACCENT));
        assert_eq!(selected.style.fg, Some(INVERSE_FG));
    }

    #[test]
    fn detail_content_renders_markdown_description_and_notes() {
        let mut item = detail_test_item();
        item.task.description = "## Context\n- **One** item".to_string();
        item.notes[0].body = "Use `aven show` after edits".to_string();

        let rendered = detail_content_lines(&item, 60, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Context"));
        assert!(rendered.contains("- One item"));
        assert!(rendered.contains("aven show"));
        assert!(!rendered.contains("`aven show`"));
    }

    #[test]
    fn detail_description_lines_keep_quote_rail() {
        let mut item = detail_test_item();
        item.task.description = "## Context\nsecond line".to_string();
        let lines = detail_content_lines(&item, 60, None);
        let description_lines: Vec<_> = lines
            .into_iter()
            .filter(|line| {
                let text = line.to_string();
                text.contains("Context") || text.contains("second")
            })
            .collect();
        assert!(!description_lines.is_empty());
        for line in description_lines {
            assert!(
                line.spans
                    .first()
                    .is_some_and(|span| span.content.as_ref() == "│ "),
                "missing quote rail: {line:?}"
            );
        }
    }

    #[test]
    fn detail_note_lines_keep_quote_rail() {
        let mut item = detail_test_item();
        item.notes[0].body = "Use `aven` here".to_string();
        let lines = detail_content_lines(&item, 60, None);
        let note_lines: Vec<_> = lines
            .into_iter()
            .filter(|line| line.to_string().contains("aven"))
            .collect();
        assert_eq!(note_lines.len(), 1);
        let line = &note_lines[0];
        assert!(
            line.spans
                .first()
                .is_some_and(|span| span.content.as_ref() == "│ "),
            "missing quote rail: {line:?}"
        );
        let code_span = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "aven")
            .expect("missing rendered inline code span");
        assert_eq!(
            code_span.style.fg,
            Some(crate::tui::theme::BLUE),
            "inline code foreground style was not preserved"
        );
        assert!(
            !line.to_string().contains('`'),
            "inline code markers leaked into rendered note text"
        );
    }

    #[test]
    fn detail_content_lists_epic_children() {
        let item = detail_test_epic_item();

        let lines = detail_content_lines(&item, 80, None)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let rendered = lines.join("\n");
        let child_heading_index = lines
            .iter()
            .position(|line| line.contains("CHILD TASKS"))
            .unwrap();

        assert_eq!(lines[child_heading_index.saturating_sub(1)], "");
        assert!(lines[child_heading_index.saturating_sub(2)].contains("active"));

        assert!(rendered.contains("CHILD TASKS open=1 total=2"));
        assert!(rendered.contains("├─ APP-CHLD"));
        assert!(rendered.contains("Build the first child task"));
        assert!(rendered.contains("└─ APP-DONE"));
        assert!(rendered.contains("Finished child task"));
        assert!(
            rendered.find("CHILD TASKS").unwrap()
                < rendered.find("Two token refresh requests").unwrap()
        );
    }

    #[test]
    fn detail_child_hit_maps_child_rows() {
        let item = detail_test_epic_item();

        let hit = detail_child_task_at_position(&item, 120, 30, 4, 8, 0).unwrap();

        assert_eq!(
            hit.task_id.as_str(),
            crate::test_support::task_id("child-task-id").as_str()
        );
    }

    #[test]
    fn hovered_detail_child_uses_link_style() {
        let item = detail_test_epic_item();
        let hovered = crate::test_support::task_id("child-task-id");
        let lines = detail_body_lines(&item, 80, Some(hovered.as_str()));
        let line = lines
            .iter()
            .find(|line| line.to_string().contains("APP-CHLD"))
            .unwrap();
        let ref_span = line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "APP-CHLD")
            .unwrap();

        assert_eq!(ref_span.style.bg, Some(BG_PANEL));
    }

    #[test]
    fn detail_metadata_includes_operational_fields() {
        let item = detail_test_item();
        let rendered = detail_metadata_lines(&item, 31)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("PROJECT\n● app"));
        assert!(rendered.contains("STATUS\n● active"));
        assert!(rendered.contains("PRIORITY\n▲ urgent"));
        assert!(rendered.contains("LABELS\nbug, mobile"));
        assert!(rendered.contains("CONFLICTS\nyes"));
    }

    #[test]
    fn detail_metadata_splits_availability_across_bounded_lines() {
        let mut item = detail_test_item();
        item.task.available_at = Some("2999-07-17T12:30:00Z".to_string());

        let lines = detail_metadata_lines(&item, 20);
        let availability = lines
            .iter()
            .position(|line| line.to_string() == "AVAILABILITY")
            .unwrap();
        let values = &lines[availability + 1..availability + 3];

        assert!(values.iter().all(|line| !line.to_string().is_empty()));
        assert!(values.iter().all(|line| line.width() <= 20));
    }

    #[test]
    fn detail_metadata_marks_epics_and_counts_children() {
        let mut item = detail_test_item();
        item.task.is_epic = true;
        item.epic_children = vec![
            crate::query::TaskDependencyLink {
                task_id: crate::test_support::task_id("child-task-id"),
                display_ref: "APP-CHLD".to_string(),
                title: "Build the first child task".to_string(),
                status: "todo".to_string(),
                priority: "medium".to_string(),
                unresolved: true,
            },
            crate::query::TaskDependencyLink {
                task_id: crate::test_support::task_id("done-child-task-id"),
                display_ref: "APP-DONE".to_string(),
                title: "Finished child task".to_string(),
                status: "done".to_string(),
                priority: "none".to_string(),
                unresolved: false,
            },
        ];

        let rendered = detail_metadata_lines(&item, 31)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains(" EPIC "));
        assert!(rendered.contains("CHILDREN\nopen=1 total=2"));
        assert!(!rendered.contains("APP-CHLD"));
        assert!(!rendered.contains("Build the first child task"));
        assert!(!rendered.contains("APP-DONE"));
    }

    #[test]
    fn detail_metadata_marks_epics_without_children() {
        let mut item = detail_test_item();
        item.task.is_epic = true;

        let rendered = detail_metadata_lines(&item, 31)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains(" EPIC "));
        assert!(rendered.contains("CHILDREN\nopen=0 total=0\nnone"));
    }

    #[test]
    fn detail_content_layout_matches_wide_and_narrow_metadata_rules() {
        let wide = detail_content_layout(Rect::new(0, 0, 120, 30));

        assert_eq!(wide.body_area, Rect::new(0, 2, 120, 26));
        assert!(wide.metadata_area.width > 0);
        assert_eq!(wide.content_area.y, 3);

        let narrow = detail_content_layout(Rect::new(0, 0, 80, 30));

        assert_eq!(narrow.metadata_area, Rect::default());
        assert_eq!(narrow.content_area.x, 2);
    }

    #[test]
    fn detail_copy_targets_map_displayed_values() {
        let item = detail_test_item();
        let expected = [
            (2, 5, item.display_ref.clone()),
            (88, 17, item.display_ref.clone()),
            (88, 20, local_timestamp_display(&item.task.created_at)),
            (88, 23, local_timestamp_display(&item.task.updated_at)),
        ];

        for (column, row, value) in expected {
            assert_eq!(
                detail_copy_target_at(&item, 120, 30, column, row).map(|hit| hit.value),
                Some(value)
            );
        }
        assert!(detail_copy_target_at(&item, 120, 30, 87, 17).is_none());
        assert!(detail_copy_target_at(&item, 120, 30, 88, 16).is_none());
        assert!(detail_copy_target_at(&item, 80, 30, 70, 20).is_none());
    }

    #[test]
    fn detail_metadata_target_maps_status_and_priority_values() {
        assert_eq!(
            detail_metadata_target_at(120, 30, 88, 8),
            Some((DetailMetadataTarget::Status, 88, 8))
        );
        assert_eq!(
            detail_metadata_target_at(120, 30, 88, 11),
            Some((DetailMetadataTarget::Priority, 88, 11))
        );
        assert_eq!(detail_metadata_target_at(120, 30, 88, 7), None);
        assert_eq!(detail_metadata_target_at(80, 30, 70, 9), None);
    }

    #[test]
    fn detail_section_targets_cycle_through_notes_and_dependencies() {
        let mut item = detail_test_item();
        item.task.description = (0..20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let layout = detail_content_layout(Rect::new(0, 0, 80, 10));
        let indices = detail_section_body_indices(&item, layout.content_area.width as usize, None);

        let notes = detail_section_scroll_target(&item, 0, 80, 10, false);
        let dependencies = detail_section_scroll_target(&item, notes, 80, 10, false);

        assert_eq!(notes, indices[1] as u16);
        assert_eq!(dependencies, indices[2] as u16);
        assert_eq!(
            detail_section_scroll_target(&item, dependencies, 80, 10, false),
            0
        );
        assert_eq!(
            detail_section_scroll_target(&item, 0, 80, 10, true),
            dependencies
        );
    }

    #[test]
    fn detail_content_model_prepares_render_lines_and_scrollbar() {
        let mut item = detail_test_item();
        item.task.description = (0..20)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let model =
            build_detail_content_model(&item, Rect::new(0, 0, 60, 5), 4, None, None, None, None);

        assert_eq!(
            model.content_height,
            detail_body_lines(&item, 60, None).len()
        );
        assert_eq!(
            model.sticky_lines.len(),
            detail_header_options(&item, 60, None).len()
        );
        assert_eq!(model.sticky_lines[0].to_string(), "Fix token refresh race");
        assert_eq!(model.lines.len(), model.content_height.saturating_sub(4));
        assert!(model.scrollbar_position > 0);
    }

    #[test]
    fn detail_empty_description_renders_attachment_section() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];

        let rendered = detail_content_lines(&item, 80, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("ATTACHMENTS\n│ [image: attachment]"));
    }

    #[test]
    fn detail_attachment_section_renders_live_rows_once_in_order() {
        let mut item = detail_test_item();
        item.attachments = vec![
            attachment_metadata("ATTACHMENT000001", false, true),
            attachment_metadata("ATTACHMENT000002", false, false),
            attachment_metadata("ATTACHMENT000003", false, false),
            attachment_metadata("ATTACHMENT000004", true, true),
        ];
        item.attachments[0].alt_text = Some("First".to_string());
        item.attachments[1].alt_text = Some("Second".to_string());
        item.attachments[2].alt_text = Some("Third".to_string());
        item.attachments[2].bytes_state = crate::attachments::AttachmentBytesState::Unavailable;
        item.attachments[3].alt_text = Some("Deleted".to_string());

        let rendered = detail_content_lines(&item, 80, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(rendered.matches("ATTACHMENTS").count(), 1);
        assert_eq!(rendered.matches("[image: attachment]").count(), 1);
        assert_eq!(rendered.matches("[image: pending download]").count(), 1);
        assert_eq!(rendered.matches("[image: unavailable bytes]").count(), 1);
        assert!(
            rendered.find("[image: attachment]").unwrap()
                < rendered.find("[image: pending download]").unwrap()
        );
        assert!(!rendered.contains("Deleted"));
    }

    #[test]
    fn detail_reserves_rows_for_previewable_attachment() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        let context = DetailInlineImageContext::default();

        let (lines, placements) = detail_body_lines_with_images(&item, 80, None, Some(&context));

        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].height, 12);
        assert!(
            lines
                .iter()
                .any(|line| line.to_string() == "│ [image: attachment]")
        );
        assert_eq!(
            lines[placements[0].line_index - 1].to_string(),
            format!("│ ┌{}┐", "─".repeat(placements[0].width as usize))
        );
        assert_eq!(
            lines[placements[0].line_index + placements[0].height as usize].to_string(),
            format!("│ └{}┘", "─".repeat(placements[0].width as usize))
        );
    }

    #[test]
    fn focused_preview_changes_border_style_without_moving_image() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        let unfocused = DetailInlineImageContext::default();
        let focused = DetailInlineImageContext {
            focused_attachment_id: Some("ATTACHMENT000001".to_string()),
            ..DetailInlineImageContext::default()
        };

        let (unfocused_lines, unfocused_placements) =
            detail_body_lines_with_images(&item, 80, None, Some(&unfocused));
        let (focused_lines, focused_placements) =
            detail_body_lines_with_images(&item, 80, None, Some(&focused));

        assert_eq!(unfocused_placements.len(), 1);
        assert_eq!(
            (
                unfocused_placements[0].line_index,
                unfocused_placements[0].width,
                unfocused_placements[0].height,
            ),
            (
                focused_placements[0].line_index,
                focused_placements[0].width,
                focused_placements[0].height,
            )
        );
        let border_index = unfocused_placements[0].line_index - 1;
        assert_eq!(
            unfocused_lines[border_index].to_string(),
            focused_lines[border_index].to_string()
        );
        assert_ne!(
            unfocused_lines[border_index].spans[1].style,
            focused_lines[border_index].spans[1].style
        );
    }

    #[test]
    fn detail_preview_preserves_image_aspect_within_max_rows() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        item.attachments[0].width = Some(646);
        item.attachments[0].height = Some(302);
        let context = DetailInlineImageContext::default();

        let (_lines, placements) = detail_body_lines_with_images(&item, 200, None, Some(&context));

        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].height, 12);
        assert_eq!(placements[0].width, 51);
    }

    #[test]
    fn detail_suppressed_preview_keeps_textual_attachment_row() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        let context = DetailInlineImageContext {
            unavailable_hashes: [item.attachments[0].sha256.clone()].into_iter().collect(),
            ..DetailInlineImageContext::default()
        };

        let (lines, placements) = detail_body_lines_with_images(&item, 80, None, Some(&context));

        assert!(placements.is_empty());
        assert!(
            lines
                .iter()
                .any(|line| line.to_string() == "│ [image: attachment]")
        );
    }

    #[test]
    fn detail_falls_back_to_single_placeholder_when_previews_disabled() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];

        let (lines, placements) = detail_body_lines_with_images(&item, 80, None, None);

        assert!(placements.is_empty());
        assert_eq!(
            lines
                .iter()
                .filter(|line| line.to_string().contains("[image: attachment]"))
                .count(),
            1
        );
        assert_eq!(
            lines.iter().filter(|line| line.to_string() == "│ ").count(),
            0
        );
    }

    #[test]
    fn detail_placements_only_include_visible_preview_rows() {
        let mut item = detail_test_item();
        item.task.description = "intro".to_string();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        let context = DetailInlineImageContext::default();

        let model = build_detail_content_model(
            &item,
            Rect::new(0, 0, 80, 10),
            0,
            None,
            None,
            None,
            Some(&context),
        );

        assert_eq!(model.image_placements.len(), 1);
        assert!(model.image_placements[0].line_index < model.content_height);
    }

    #[test]
    fn detail_omits_preview_when_frame_is_clipped_by_viewport() {
        let model = DetailContentRenderModel {
            sticky_lines: Vec::new(),
            lines: vec![Line::from(""); 5],
            content_height: 12,
            body_start: 0,
            scrollbar_position: 0,
            image_placements: vec![DetailBodyImagePlacement {
                attachment_id: "ATTACHMENT000001".to_string(),
                source_hash: "0".repeat(64),
                line_index: 3,
                width: 30,
                height: 12,
            }],
        };
        let mut widgets = WidgetState {
            sidebar: ListState::default(),
            table: TableState::default(),
            marked_task_ids: BTreeSet::new(),
            inline_image_placements: Vec::new(),
        };
        let backend = TestBackend::new(20, 5);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                render_detail_content_from_model(
                    frame,
                    Rect::new(0, 0, 20, 5),
                    &model,
                    &mut widgets,
                );
            })
            .unwrap();

        assert!(widgets.inline_image_placements.is_empty());
    }

    #[test]
    fn detail_attachment_hit_tracks_scroll_and_excludes_suppressed_preview() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        let terminal = Rect::new(0, 0, 100, 40);
        let layout = detail_content_layout(terminal);
        let context = DetailInlineImageContext::default();
        let scroll = 2;
        let model = build_detail_content_model(
            &item,
            layout.content_area,
            scroll,
            None,
            None,
            None,
            Some(&context),
        );
        let placement = &model.image_placements[0];
        let body_y = layout.content_area.y.saturating_add(
            model
                .sticky_lines
                .len()
                .min(layout.content_area.height as usize) as u16,
        );
        let column = layout.content_area.x.saturating_add(2);
        let row = body_y.saturating_add(
            placement
                .line_index
                .saturating_sub(model.body_start)
                .saturating_sub(1) as u16,
        );

        let hit = detail_attachment_at_position(
            &item,
            terminal.width,
            terminal.height,
            column,
            row,
            scroll,
            &context,
        );
        assert_eq!(
            hit.map(|hit| hit.attachment_id),
            Some("ATTACHMENT000001".to_string())
        );
        let suppressed_context = DetailInlineImageContext {
            unavailable_hashes: [item.attachments[0].sha256.clone()].into_iter().collect(),
            ..DetailInlineImageContext::default()
        };
        assert!(
            detail_attachment_at_position(
                &item,
                terminal.width,
                terminal.height,
                column,
                row,
                scroll,
                &suppressed_context,
            )
            .is_none()
        );
    }

    #[test]
    fn duplicate_hash_hit_uses_the_visible_attachment_frame() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![
            attachment_metadata("FIRSTATTACHMENT", false, true),
            attachment_metadata("SECONDATTACHMENT", false, true),
        ];
        assert_eq!(item.attachments[0].sha256, item.attachments[1].sha256);
        let context = DetailInlineImageContext::default();
        let terminal = Rect::new(0, 0, 80, 30);
        let layout = detail_content_layout(terminal);
        let model = build_detail_content_model(
            &item,
            layout.content_area,
            0,
            None,
            None,
            None,
            Some(&context),
        );
        let sticky_height = model
            .sticky_lines
            .len()
            .min(layout.content_area.height as usize) as u16;
        let body_area = Rect::new(
            layout.content_area.x,
            layout.content_area.y.saturating_add(sticky_height),
            layout.content_area.width,
            layout.content_area.height.saturating_sub(sticky_height),
        );
        let cap = detail_scroll_cap_with_images(&item, 80, 30, Some(&context));
        let (scroll, image) = (0..=cap)
            .find_map(|scroll| {
                let model = build_detail_content_model(
                    &item,
                    layout.content_area,
                    scroll,
                    None,
                    None,
                    None,
                    Some(&context),
                );
                let first =
                    visible_detail_image_rect(body_area, &model, &model.image_placements[0]);
                let second =
                    visible_detail_image_rect(body_area, &model, &model.image_placements[1]);
                (first.is_none() && second.is_some()).then(|| (scroll, second.unwrap()))
            })
            .expect("only the second duplicate frame visible");
        let hit = detail_attachment_at_position(
            &item,
            80,
            30,
            image.x.saturating_sub(1),
            image.y.saturating_sub(1),
            scroll,
            &context,
        )
        .expect("visible duplicate hit");

        assert_eq!(hit.attachment_id, "SECONDATTACHMENT");
    }

    #[test]
    fn framed_preview_overflow_expands_scroll_and_reaches_hit_target() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        let context = DetailInlineImageContext::default();
        let width = 80;
        let height = 26;
        let text_only_cap = detail_scroll_cap(&item, width, height);
        let preview_cap = detail_scroll_cap_with_images(&item, width, height, Some(&context));

        assert!(preview_cap > text_only_cap);
        let target =
            detail_attachment_scroll_target(&item, "ATTACHMENT000001", 0, width, height, &context)
                .expect("attachment scroll target");
        assert!(target > text_only_cap);
        let reached = (0..height).any(|row| {
            (0..width).any(|column| {
                detail_attachment_at_position(&item, width, height, column, row, target, &context)
                    .is_some()
            })
        });
        assert!(reached);
    }

    #[test]
    fn large_attachment_preview_stays_inside_its_border() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        let context = DetailInlineImageContext::default();
        let mut widgets = WidgetState {
            sidebar: ListState::default(),
            table: TableState::default(),
            marked_task_ids: BTreeSet::new(),
            inline_image_placements: Vec::new(),
        };
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                render_detail(
                    frame,
                    &item,
                    0,
                    None,
                    None,
                    None,
                    &mut widgets,
                    Some(&context),
                );
            })
            .unwrap();
        let thumbnail = widgets.inline_image_placements[0].clone();
        widgets.inline_image_placements.clear();

        terminal
            .draw(|frame| {
                render_attachment_preview(
                    frame,
                    &item,
                    "ATTACHMENT000001",
                    &mut widgets,
                    Some(&context),
                );
            })
            .unwrap();

        assert_eq!(widgets.inline_image_placements.len(), 1);
        let placement = &widgets.inline_image_placements[0];
        assert_eq!(placement.source_hash, thumbnail.source_hash);
        assert_ne!(
            (placement.x, placement.y, placement.width, placement.height),
            (thumbnail.x, thumbnail.y, thumbnail.width, thumbnail.height)
        );
        let area = detail_body_area(Rect::new(0, 0, 100, 30));
        assert!(placement.x > area.x);
        assert!(placement.y > area.y);
        assert!(placement.x.saturating_add(placement.width) < area.x + area.width);
        assert!(placement.y.saturating_add(placement.height) < area.y + area.height);
    }

    fn attachment_metadata(
        attachment_id: &str,
        deleted: bool,
        has_blob: bool,
    ) -> crate::task_render::AttachmentMetadataJson {
        crate::task_render::AttachmentMetadataJson {
            attachment_id: attachment_id.to_string(),
            task_id: "7KQ9A1X".to_string(),
            sha256: "0".repeat(64),
            media_type: "image/png".to_string(),
            byte_size: 4,
            filename: Some("chart.png".to_string()),
            alt_text: Some("Chart".to_string()),
            width: Some(640),
            height: Some(480),
            created_at: "2026-06-20T12:00:00Z".to_string(),
            deleted,
            deleted_at: deleted.then(|| "2026-06-20T12:00:00Z".to_string()),
            bytes_state: if has_blob {
                crate::attachments::AttachmentBytesState::Present
            } else {
                crate::attachments::AttachmentBytesState::PendingDownload
            },
            has_blob,
        }
    }

    fn detail_test_epic_item() -> TaskListItem {
        let mut item = detail_test_item();
        item.task.is_epic = true;
        item.epic_children = vec![
            crate::query::TaskDependencyLink {
                task_id: crate::test_support::task_id("child-task-id"),
                display_ref: "APP-CHLD".to_string(),
                title: "Build the first child task".to_string(),
                status: "todo".to_string(),
                priority: "medium".to_string(),
                unresolved: true,
            },
            crate::query::TaskDependencyLink {
                task_id: crate::test_support::task_id("done-child-task-id"),
                display_ref: "APP-DONE".to_string(),
                title: "Finished child task".to_string(),
                status: "done".to_string(),
                priority: "none".to_string(),
                unresolved: false,
            },
        ];
        item
    }

    fn detail_test_item() -> TaskListItem {
        TaskListItem {
            task: crate::types::Task {
                id: crate::test_support::task_id("7KQ9A1X"),
                workspace_id: "0000000000000001".parse().unwrap(),
                title: "Fix token refresh race".to_string(),
                description: "Two token refresh requests fire together.".to_string(),
                project_id: "0000000000000001".parse().unwrap(),
                project_key: "app".to_string(),
                project_prefix: "APP".to_string(),
                status: TaskStatus::Active,
                priority: TaskPriority::Urgent,
                created_at: "2026-06-19T12:00:00Z".to_string(),
                updated_at: "2026-06-20T12:00:00Z".to_string(),
                queue_activity_at: "2026-06-20T12:00:00Z".to_string(),
                available_at: None,
                due_on: None,
                deleted: false,
                is_epic: false,
            },
            display_ref: "APP-7KQ9A1X".to_string(),
            labels: vec!["bug".to_string(), "mobile".to_string()],
            notes: vec![crate::query::TaskNote {
                body: "Confirmed race in useTokenRefresh.ts".to_string(),
                created_at: "2026-06-20T12:00:00Z".to_string(),
            }],
            has_conflict: true,
            unresolved_blocker_count: 0,
            dependent_count: 0,
            depends_on: vec![crate::query::TaskDependencyLink {
                task_id: crate::test_support::task_id("blocker-task-id"),
                display_ref: "APP-7KQ1".to_string(),
                title: "Ship auth service".to_string(),
                status: "todo".to_string(),
                priority: "high".to_string(),
                unresolved: true,
            }],
            blocks: vec![crate::query::TaskDependencyLink {
                task_id: crate::test_support::task_id("dependent-task-id"),
                display_ref: "APP-7KQ2".to_string(),
                title: "Write rollout notes".to_string(),
                status: "inbox".to_string(),
                priority: "none".to_string(),
                unresolved: true,
            }],
            epic_children: Vec::new(),
            epic_parent: None,
            attachments: Vec::new(),
            queue: Default::default(),
        }
    }
}
