use std::collections::BTreeSet;
use std::rc::Rc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Clear, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState};

use super::super::scroll::{clamp_scroll_start, scrollbar_thumb_position};
#[cfg(test)]
use super::attachments::attachment_is_locally_openable;
use super::attachments::{
    DetailBodyImagePlacement, DetailInlineImageContext, DetailInlineImagePlacement,
};
use super::body::{DetailBodyDocument, build_detail_body_document};
use super::metadata::render_detail_metadata;
use super::relationships::apply_link_row_style;
use super::relationships::{DetailEpicChild, detail_epic_children};
use super::text::{
    DetailSelectableDocument, apply_detail_selection_from_document, detail_header_options,
    detail_selectable_document_from_body,
};
use super::{
    DetailRevision, DetailSection, DetailTargetId, DetailTextSelection, TaskListItem, TextCell,
    TextInputView, WidgetState, detail_target_is_actionable,
};
use crate::tui::detail_selection::text_cell_at_column;
use crate::tui::theme::{self, BG};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DetailContentLayout {
    pub(super) body_area: Rect,
    pub(super) content_area: Rect,
    pub(super) metadata_area: Rect,
}

#[derive(Clone, Copy)]
pub(crate) struct DetailRenderContext<'a> {
    pub(crate) terminal_area: Rect,
    pub(crate) scroll: u16,
    pub(crate) detail_revision: DetailRevision,
    pub(crate) inline_title_editor: Option<&'a TextInputView>,
    pub(crate) active_target: Option<&'a DetailTargetId>,
    pub(crate) hovered_target: Option<&'a DetailTargetId>,
    pub(crate) expanded_sections: &'a BTreeSet<DetailSection>,
    pub(crate) selection: Option<&'a DetailTextSelection>,
    pub(crate) inline_images: Option<&'a DetailInlineImageContext>,
    pub(crate) pending_attachments:
        &'a [crate::tui::attachment_controller::PendingAttachmentView],
    pub(crate) removed_epic_child: Option<&'a crate::tui::app::RemovedEpicChild>,
}

impl DetailRenderContext<'_> {
    pub(super) fn content_layout(&self) -> DetailContentLayout {
        detail_content_layout(self.terminal_area)
    }
}

#[derive(Debug, Clone)]
pub(super) struct DetailContentRenderModel {
    pub(super) sticky_lines: Vec<Line<'static>>,
    pub(super) lines: Vec<Line<'static>>,
    pub(super) content_height: usize,
    pub(super) body_start: usize,
    pub(super) scrollbar_position: usize,
    pub(super) image_placements: Rc<Vec<DetailBodyImagePlacement>>,
    pub(super) interactive_rows: Rc<Vec<DetailInteractiveRow>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailInteractiveRow {
    pub(crate) target: DetailTargetId,
    pub(crate) line_index: usize,
    pub(crate) height: usize,
}

#[derive(Debug)]
pub(super) struct DetailBodyGeometry {
    pub(super) task_id: crate::ids::TaskId,
    pub(super) detail_revision: DetailRevision,
    pub(super) content_width: usize,
    pub(super) expanded_sections: BTreeSet<DetailSection>,
    pub(super) inline_images: Option<DetailInlineImageContext>,
    pub(super) pending_attachments: Vec<crate::tui::attachment_controller::PendingAttachmentView>,
    pub(super) removed_epic_child: Option<crate::tui::app::RemovedEpicChild>,
    pub(super) epic_children: Vec<DetailEpicChild>,
    pub(super) body: DetailBodyDocument,
    pub(super) selectable: DetailSelectableDocument,
}

#[derive(Debug)]
pub(crate) struct DetailDocument {
    pub(super) geometry: Rc<DetailBodyGeometry>,
    pub(super) layout: DetailContentLayout,
    pub(super) scroll: u16,
    pub(super) inline_title_editor: Option<(String, usize)>,
    pub(super) model: DetailContentRenderModel,
    #[cfg(test)]
    projection_id: usize,
}
impl DetailBodyGeometry {
    fn build(item: &TaskListItem, context: &DetailRenderContext<'_>) -> Self {
        let content_width = context.content_layout().content_area.width as usize;
        let inline_images = context.inline_images.cloned().map(|mut images| {
            images.focused_attachment_id = None;
            images
        });
        let epic_children = detail_epic_children(item, context.removed_epic_child);
        let body = build_detail_body_document(
            item,
            &epic_children,
            content_width,
            context.expanded_sections,
            inline_images.as_ref(),
            context.pending_attachments,
        );
        let selectable = detail_selectable_document_from_body(item, content_width, true, &body);
        Self {
            task_id: item.task.id.clone(),
            detail_revision: context.detail_revision,
            content_width,
            expanded_sections: context.expanded_sections.clone(),
            inline_images,
            pending_attachments: context.pending_attachments.to_vec(),
            removed_epic_child: context.removed_epic_child.cloned(),
            epic_children,
            body,
            selectable,
        }
    }
}

impl DetailDocument {
    pub(crate) fn build(item: &TaskListItem, context: &DetailRenderContext<'_>) -> Self {
        let geometry = Rc::new(DetailBodyGeometry::build(item, context));
        Self::from_geometry(geometry, item, context)
    }

    fn from_geometry(
        geometry: Rc<DetailBodyGeometry>,
        item: &TaskListItem,
        context: &DetailRenderContext<'_>,
    ) -> Self {
        let layout = context.content_layout();
        let sticky_lines = detail_header_options(
            item,
            layout.content_area.width as usize,
            context.inline_title_editor,
        );
        let model = project_detail_content_model(
            sticky_lines,
            &geometry.body,
            layout.content_area.height as usize,
            context.scroll,
        );
        Self {
            geometry,
            layout,
            scroll: context.scroll,
            inline_title_editor: context
                .inline_title_editor
                .map(|editor| (editor.input.to_string(), editor.cursor)),
            model,
            #[cfg(test)]
            projection_id: super::next_detail_projection_id(),
        }
    }

    fn reproject(&self, item: &TaskListItem, context: &DetailRenderContext<'_>) -> Self {
        Self::from_geometry(Rc::clone(&self.geometry), item, context)
    }

    pub(crate) fn reuse_or_build(
        cached: Option<&Rc<Self>>,
        item: &TaskListItem,
        context: &DetailRenderContext<'_>,
    ) -> Rc<Self> {
        let Some(cached) = cached else {
            return Rc::new(Self::build(item, context));
        };
        if !cached.geometry_matches(item, context) {
            return Rc::new(Self::build(item, context));
        }
        if cached.view_matches(context) {
            Rc::clone(cached)
        } else {
            Rc::new(cached.reproject(item, context))
        }
    }

    fn geometry_matches(&self, item: &TaskListItem, context: &DetailRenderContext<'_>) -> bool {
        let layout = context.content_layout();
        let geometry = &self.geometry;
        geometry.task_id == item.task.id
            && geometry.detail_revision == context.detail_revision
            && geometry.content_width == layout.content_area.width as usize
            && geometry.expanded_sections == *context.expanded_sections
            && detail_inline_image_geometry_matches(
                geometry.inline_images.as_ref(),
                context.inline_images,
            )
            && geometry.pending_attachments == context.pending_attachments
            && geometry.removed_epic_child.as_ref() == context.removed_epic_child
    }

    fn view_matches(&self, context: &DetailRenderContext<'_>) -> bool {
        self.layout == context.content_layout()
            && self.scroll == context.scroll
            && self.inline_title_editor.as_ref()
                == context
                    .inline_title_editor
                    .map(|editor| (&editor.input, editor.cursor))
                    .map(|(input, cursor)| (input.clone(), cursor))
                    .as_ref()
    }

    pub(super) fn render(
        &self,
        frame: &mut Frame,
        item: &TaskListItem,
        context: &DetailRenderContext<'_>,
        widgets: &mut WidgetState,
    ) {
        frame.render_widget(Clear, self.layout.body_area);
        frame.render_widget(
            Block::new().style(Style::new().bg(BG)),
            self.layout.body_area,
        );
        if self.layout.body_area.width == 0 || self.layout.body_area.height == 0 {
            return;
        }
        let mut model = self.model.clone();
        if let Some(active_target) = context.active_target {
            apply_active_style(&mut model, active_target);
        }
        if context.hovered_target != context.active_target
            && let Some(hovered_target) = context.hovered_target
        {
            apply_hover_style(&mut model, hovered_target);
        }
        if context.inline_title_editor.is_none()
            && let Some(selection) = context.selection.filter(|selection| {
                selection.task_id == item.task.id
                    && selection.terminal_width == context.terminal_area.width
            })
        {
            apply_detail_selection_from_document(
                &self.geometry.selectable,
                selection,
                &mut model.sticky_lines,
                &mut model.lines,
                model.body_start,
            );
        }
        render_detail_content_from_model(frame, self.layout.content_area, model, widgets);
        if self.layout.metadata_area.width > 0 {
            render_detail_metadata(
                frame,
                item,
                &self.geometry.epic_children,
                self.layout.metadata_area,
            );
        }
    }

    #[cfg(test)]
    pub(crate) fn matches_frame(
        &self,
        item: &TaskListItem,
        context: &DetailRenderContext<'_>,
    ) -> bool {
        self.geometry_matches(item, context) && self.view_matches(context)
    }

    pub(super) fn sticky_height(&self) -> usize {
        self.model
            .sticky_lines
            .len()
            .min(self.layout.content_area.height as usize)
    }

    fn body_visible(&self) -> usize {
        (self.layout.content_area.height as usize).saturating_sub(self.sticky_height())
    }

    pub(crate) fn scroll_cap(&self) -> u16 {
        self.model
            .content_height
            .saturating_sub(self.body_visible()) as u16
    }

    #[cfg(test)]
    pub(crate) fn interactive_rows(&self) -> &[DetailInteractiveRow] {
        self.geometry.body.interactive_rows.as_slice()
    }

    pub(crate) fn focus_targets(&self, item: &TaskListItem) -> Vec<DetailTargetId> {
        self.geometry
            .body
            .interactive_rows
            .iter()
            .map(|row| &row.target)
            .filter(|target| match target {
                DetailTargetId::Task {
                    section: DetailSection::EpicChildren,
                    task_id,
                } => self
                    .geometry
                    .epic_children
                    .iter()
                    .any(|child| child.link.task_id == *task_id),
                DetailTargetId::Expand {
                    section: DetailSection::EpicChildren,
                } => self.geometry.epic_children.len() > 5,
                _ => detail_target_is_actionable(item, target),
            })
            .cloned()
            .collect()
    }

    pub(crate) fn link_at_position(&self, column: u16, row: u16) -> Option<String> {
        let body_y = self
            .layout
            .content_area
            .y
            .saturating_add(self.sticky_height() as u16);
        if row < body_y
            || row
                >= self
                    .layout
                    .content_area
                    .y
                    .saturating_add(self.layout.content_area.height)
            || column < self.layout.content_area.x
        {
            return None;
        }
        let line_index = self
            .model
            .body_start
            .saturating_add(row.saturating_sub(body_y) as usize);
        let local_column = column.saturating_sub(self.layout.content_area.x) as usize;
        self.geometry
            .body
            .hyperlinks
            .iter()
            .find(|link| {
                link.line_index == line_index
                    && (link.start_column..link.end_column).contains(&local_column)
            })
            .map(|link| link.url.clone())
    }

    pub(crate) fn target_at_position(&self, column: u16, row: u16) -> Option<DetailTargetId> {
        if column < self.layout.content_area.x
            || column
                >= self
                    .layout
                    .content_area
                    .x
                    .saturating_add(self.layout.content_area.width)
        {
            return None;
        }
        let body_y = self
            .layout
            .content_area
            .y
            .saturating_add(self.sticky_height() as u16);
        if row < body_y
            || row
                >= self
                    .layout
                    .content_area
                    .y
                    .saturating_add(self.layout.content_area.height)
        {
            return None;
        }
        let body_index = self
            .model
            .body_start
            .saturating_add(row.saturating_sub(body_y) as usize);
        self.geometry
            .body
            .interactive_rows
            .iter()
            .find(|target| {
                (target.line_index..target.line_index.saturating_add(target.height))
                    .contains(&body_index)
            })
            .map(|target| target.target.clone())
    }

    #[cfg(test)]
    pub(crate) fn attachment_at_position(
        &self,
        item: &TaskListItem,
        column: u16,
        row: u16,
    ) -> Option<String> {
        let DetailTargetId::Attachment { attachment_id } = self.target_at_position(column, row)?
        else {
            return None;
        };
        item.attachments
            .iter()
            .find(|attachment| attachment.attachment_id == attachment_id)
            .filter(|attachment| attachment_is_locally_openable(attachment))
            .map(|_| attachment_id)
    }

    #[cfg(test)]
    pub(crate) fn child_task_at_position(
        &self,
        column: u16,
        row: u16,
    ) -> Option<crate::ids::TaskId> {
        match self.target_at_position(column, row)? {
            DetailTargetId::Task {
                section: DetailSection::EpicChildren,
                task_id,
            } => Some(task_id),
            _ => None,
        }
    }

    pub(crate) fn target_scroll_target(&self, target: &DetailTargetId, scroll: u16) -> Option<u16> {
        let visible = self.body_visible();
        let row = self
            .geometry
            .body
            .interactive_rows
            .iter()
            .find(|row| &row.target == target)?;
        if visible == 0 {
            return None;
        }
        let cap = self.model.content_height.saturating_sub(visible);
        let scroll = (scroll as usize).min(cap);
        let end = row.line_index.saturating_add(row.height.saturating_sub(1));
        let target_scroll = if row.line_index < scroll {
            row.line_index
        } else if end >= scroll.saturating_add(visible) {
            end.saturating_add(1).saturating_sub(visible)
        } else {
            scroll
        };
        Some(target_scroll.min(cap) as u16)
    }

    pub(crate) fn section_scroll_target(&self, reverse: bool) -> u16 {
        let scroll_cap = self
            .model
            .content_height
            .saturating_sub(self.body_visible());
        let mut targets = self
            .geometry
            .body
            .section_body_indices
            .iter()
            .map(|index| (*index).min(scroll_cap) as u16)
            .collect::<Vec<_>>();
        targets.dedup();
        if reverse {
            targets
                .iter()
                .rev()
                .find(|&&target| target < self.model.body_start as u16)
                .copied()
                .or_else(|| targets.last().copied())
                .unwrap_or(0)
        } else {
            targets
                .iter()
                .find(|&&target| target > self.model.body_start as u16)
                .copied()
                .or_else(|| targets.first().copied())
                .unwrap_or(0)
        }
    }

    pub(crate) fn text_cell_at_position(&self, column: u16, row: u16) -> Option<TextCell> {
        if column < self.layout.body_area.x
            || column
                >= self
                    .layout
                    .content_area
                    .x
                    .saturating_add(self.layout.content_area.width)
            || row < self.layout.content_area.y
            || row
                >= self
                    .layout
                    .content_area
                    .y
                    .saturating_add(self.layout.content_area.height)
        {
            return None;
        }
        let title_row = row.saturating_sub(self.layout.content_area.y) as usize;
        let selectable = if let Some(title) = self.geometry.selectable.title.get(title_row) {
            title
        } else {
            let body_y = self
                .layout
                .content_area
                .y
                .saturating_add(self.sticky_height() as u16);
            if row < body_y || row >= body_y.saturating_add(self.layout.content_area.height) {
                return None;
            }
            let body_index = self
                .model
                .body_start
                .saturating_add(row.saturating_sub(body_y) as usize);
            self.geometry
                .selectable
                .description
                .iter()
                .find(|line| line.body_index == Some(body_index))?
        };
        let text_x = self
            .layout
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

    pub(crate) fn selected_text(&self, selection: &DetailTextSelection) -> Option<String> {
        if selection.task_id != self.geometry.task_id {
            return None;
        }
        self.geometry
            .selectable
            .text
            .get(selection.range())
            .map(str::to_string)
    }

    #[cfg(test)]
    pub(crate) fn projection_id(&self) -> usize {
        self.projection_id
    }
}

pub(super) fn detail_content_layout(frame_area: Rect) -> DetailContentLayout {
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

pub(super) fn detail_body_area(frame_area: Rect) -> Rect {
    let [_, body, _] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Fill(1),
        Constraint::Length(2),
    ])
    .areas(frame_area);
    body
}

pub(super) fn detail_content_margin() -> Margin {
    Margin {
        horizontal: 2,
        vertical: 1,
    }
}

pub(super) fn project_detail_content_model(
    sticky_lines: Vec<Line<'static>>,
    body: &DetailBodyDocument,
    area_height: usize,
    scroll: u16,
) -> DetailContentRenderModel {
    let content_height = body.lines.len().max(1);
    let sticky_height = sticky_lines.len().min(area_height);
    let visible = area_height.saturating_sub(sticky_height);
    let start = clamp_scroll_start(scroll, content_height, visible.max(1));
    let end = start.saturating_add(visible).min(body.lines.len());
    let lines = body.lines[start.min(body.lines.len())..end].to_vec();
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
        image_placements: Rc::clone(&body.image_placements),
        interactive_rows: Rc::clone(&body.interactive_rows),
    }
}

pub(super) fn detail_inline_image_geometry_matches(
    cached: Option<&DetailInlineImageContext>,
    current: Option<&DetailInlineImageContext>,
) -> bool {
    match (cached, current) {
        (None, None) => true,
        (Some(cached), Some(current)) => {
            cached.previews_enabled == current.previews_enabled
                && cached.unavailable_hashes == current.unavailable_hashes
        }
        _ => false,
    }
}

fn interactive_row_lines_mut<'a>(
    model: &'a mut DetailContentRenderModel,
    target: &DetailTargetId,
) -> Option<&'a mut [Line<'static>]> {
    let row = model
        .interactive_rows
        .iter()
        .find(|row| &row.target == target)?;
    let visible_start = model.body_start;
    let visible_end = visible_start.saturating_add(model.lines.len());
    let row_start = row.line_index.max(visible_start);
    let row_end = row.line_index.saturating_add(row.height).min(visible_end);
    if row_start >= row_end {
        return None;
    }
    Some(&mut model.lines[row_start - visible_start..row_end - visible_start])
}

pub(super) fn apply_active_style(model: &mut DetailContentRenderModel, target: &DetailTargetId) {
    let Some(lines) = interactive_row_lines_mut(model, target) else {
        return;
    };
    match target {
        DetailTargetId::CustomMetadata | DetailTargetId::Expand { .. } => {
            for line in lines {
                for (index, span) in line.spans.iter_mut().enumerate() {
                    span.style = span.style.bg(theme::bg_panel());
                    if index == 0 {
                        span.style = span.style.fg(theme::border());
                    } else {
                        span.style = span.style.fg(theme::accent()).add_modifier(Modifier::BOLD);
                    }
                }
            }
        }
        DetailTargetId::Note { .. } => {
            for line in lines {
                for span in &mut line.spans {
                    span.style = span.style.bg(theme::bg_panel());
                }
            }
        }
        DetailTargetId::Attachment { .. } => {
            for line in lines {
                for span in line.spans.iter_mut().skip(1) {
                    span.style = span.style.fg(theme::accent());
                }
            }
        }
        DetailTargetId::Task {
            section: DetailSection::EpicParent | DetailSection::EpicChildren,
            ..
        } => {
            for line in lines {
                for (index, span) in line.spans.iter_mut().enumerate() {
                    span.style = span.style.bg(theme::bg_panel());
                    match index {
                        0 => span.style = span.style.fg(theme::border()),
                        1 => {
                            span.style =
                                span.style.fg(theme::accent()).add_modifier(Modifier::BOLD);
                        }
                        2 => span.style = span.style.fg(theme::fg_dim()),
                        _ => {}
                    }
                }
            }
        }
        DetailTargetId::Task { .. } => apply_link_row_style(lines),
    }
}

pub(super) fn apply_hover_style(model: &mut DetailContentRenderModel, target: &DetailTargetId) {
    let Some(lines) = interactive_row_lines_mut(model, target) else {
        return;
    };
    for line in lines {
        for span in &mut line.spans {
            if matches!(target, DetailTargetId::Note { .. }) {
                span.style = span.style.bg(theme::bg_panel());
            } else {
                span.style = span.style.add_modifier(Modifier::UNDERLINED);
            }
        }
    }
}

pub(super) fn visible_detail_image_rect(
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

pub(super) fn render_detail_content_from_model(
    frame: &mut Frame,
    area: Rect,
    model: DetailContentRenderModel,
    widgets: &mut WidgetState,
) {
    let visible = area.height as usize;
    let sticky_height = model.sticky_lines.len().min(visible);
    let [sticky_area, body_area] = Layout::vertical([
        Constraint::Length(sticky_height as u16),
        Constraint::Fill(1),
    ])
    .areas(area);
    let images = model
        .image_placements
        .iter()
        .filter_map(|placement| {
            let image = visible_detail_image_rect(body_area, &model, placement)?;
            Some(DetailInlineImagePlacement {
                attachment_id: placement.attachment_id.clone(),
                source_hash: placement.source_hash.clone(),
                x: image.x,
                y: image.y,
                width: image.width,
                height: image.height,
            })
        })
        .collect::<Vec<_>>();
    frame.render_widget(
        Paragraph::new(Text::from(model.sticky_lines)).style(Style::new().fg(theme::fg()).bg(BG)),
        sticky_area,
    );
    frame.render_widget(
        Paragraph::new(Text::from(model.lines)).style(Style::new().fg(theme::fg()).bg(BG)),
        body_area,
    );
    let body_visible = body_area.height as usize;
    if model.content_height > body_visible {
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::new().fg(theme::fg_dim()).bg(BG))
                .thumb_style(Style::new().fg(theme::fg_muted())),
            body_area,
            &mut ScrollbarState::new(model.content_height)
                .position(model.scrollbar_position)
                .viewport_content_length(body_visible.max(1)),
        );
    }
    widgets.inline_image_placements.extend(images);
}
