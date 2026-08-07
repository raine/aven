use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, Padding, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use std::collections::{BTreeSet, HashSet};

use super::input::clipped_input_line;
use super::scroll::{clamp_scroll_start, scrollbar_thumb_position};
use super::task_display::{description_or_placeholder, labels_display};
use super::task_list::EPIC_MARKER;
use super::timestamps::local_timestamp_display;
use super::truncate::truncate_width;
use crate::query::TaskListItem;
use crate::task_render::{AttachmentMetadataJson, attachment_state_placeholder, human_file_size};
use crate::tui::app::{DetailSection, DetailTargetId, WidgetState};
use crate::tui::detail_selection::{DetailTextSelection, TextCell, text_cell_at_column};
use crate::tui::markdown::{
    MarkdownBlock, MarkdownRenderContext, render_markdown_with_context_without_link_urls,
    render_markdown_without_link_urls,
};
use crate::tui::overlay::TextInputView;
use crate::tui::store::TuiStore;
use crate::tui::theme::{
    self, ACCENT, BG, BG_PANEL, BORDER, FG, FG_DIM, FG_MUTED, INVERSE_FG, ORANGE, RED, YELLOW,
};
use crate::tui::widgets::{priority_short, status_chip, status_span};
use unicode_width::UnicodeWidthStr;

const DETAIL_DEPENDENCY_TREE_CAP: usize = 3;

pub(crate) fn detail_target_is_actionable(item: &TaskListItem, target: &DetailTargetId) -> bool {
    match target {
        DetailTargetId::Task { section, task_id } => match section {
            DetailSection::EpicParent => item
                .epic_parent
                .as_ref()
                .is_some_and(|link| &link.task_id == task_id),
            DetailSection::EpicChildren => item
                .epic_children
                .iter()
                .any(|link| &link.task_id == task_id),
            DetailSection::DependsOn => item.depends_on.iter().any(|link| &link.task_id == task_id),
            DetailSection::Blocks => item.blocks.iter().any(|link| &link.task_id == task_id),
            DetailSection::Attachments | DetailSection::Notes => false,
        },
        DetailTargetId::Note { note_id } => item.notes.iter().any(|note| note.id == *note_id),
        DetailTargetId::Attachment { attachment_id } => item
            .attachments
            .iter()
            .find(|attachment| attachment.attachment_id == *attachment_id)
            .is_some_and(attachment_is_locally_openable),
        DetailTargetId::Expand { section } => match section {
            DetailSection::EpicChildren => item.epic_children.len() > 5,
            DetailSection::DependsOn => item.depends_on.len() > DETAIL_DEPENDENCY_TREE_CAP,
            DetailSection::Blocks => item.blocks.len() > DETAIL_DEPENDENCY_TREE_CAP,
            DetailSection::EpicParent | DetailSection::Attachments | DetailSection::Notes => false,
        },
    }
}

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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailContentLayout {
    body_area: Rect,
    content_area: Rect,
    metadata_area: Rect,
}

#[derive(Clone, Copy)]
pub(crate) struct DetailRenderContext<'a> {
    pub(crate) terminal_area: Rect,
    pub(crate) scroll: u16,
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
    fn content_layout(&self) -> DetailContentLayout {
        detail_content_layout(self.terminal_area)
    }
}

#[derive(Debug, Clone)]
struct DetailContentRenderModel {
    sticky_lines: Vec<Line<'static>>,
    lines: Vec<Line<'static>>,
    content_height: usize,
    body_start: usize,
    scrollbar_position: usize,
    image_placements: Vec<DetailBodyImagePlacement>,
    interactive_rows: Vec<DetailInteractiveRow>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailInteractiveRow {
    pub(crate) target: DetailTargetId,
    pub(crate) line_index: usize,
    pub(crate) height: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DetailInlineImageContext {
    pub(crate) previews_enabled: bool,
    pub(crate) unavailable_hashes: HashSet<String>,
    pub(crate) focused_attachment_id: Option<String>,
}

impl Default for DetailInlineImageContext {
    fn default() -> Self {
        Self {
            previews_enabled: true,
            unavailable_hashes: HashSet::new(),
            focused_attachment_id: None,
        }
    }
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
struct DetailBodyAttachmentPlacement {
    attachment_id: String,
    line_index: usize,
    height: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EpicChildState {
    Live,
    Removed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailEpicChild {
    link: crate::query::TaskDependencyLink,
    state: EpicChildState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct EpicChildCounts {
    open: usize,
    total: usize,
}

#[derive(Debug, Clone)]
struct DetailBodyDocument {
    lines: Vec<Line<'static>>,
    image_placements: Vec<DetailBodyImagePlacement>,
    interactive_rows: Vec<DetailInteractiveRow>,
    hyperlinks: Vec<DetailHyperlink>,
    selectable_description: Vec<SelectableLine>,
    selectable_text: String,
    section_body_indices: Vec<usize>,
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

#[derive(Debug, Clone)]
struct SelectableLine {
    text: String,
    document_start: usize,
    body_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailHyperlink {
    url: String,
    line_index: usize,
    start_column: usize,
    end_column: usize,
}

#[derive(Debug, Clone)]
struct DetailSelectableDocument {
    text: String,
    title: Vec<SelectableLine>,
    description: Vec<SelectableLine>,
}

#[derive(Debug)]
pub(crate) struct DetailDocument {
    source: TaskListItem,
    epic_children: Vec<DetailEpicChild>,
    layout: DetailContentLayout,
    scroll: u16,
    expanded_sections: BTreeSet<DetailSection>,
    inline_images: Option<DetailInlineImageContext>,
    pending_attachments: Vec<crate::tui::attachment_controller::PendingAttachmentView>,
    inline_title_editor: Option<(String, usize)>,
    model: DetailContentRenderModel,
    hyperlinks: Vec<DetailHyperlink>,
    selectable: DetailSelectableDocument,
    section_body_indices: Vec<usize>,
    #[cfg(test)]
    projection_id: usize,
}

#[cfg(test)]
pub(crate) struct DetailChildHit {
    pub(crate) task_id: crate::ids::TaskId,
}

#[cfg(test)]
pub(crate) struct DetailAttachmentHit {
    pub(crate) attachment_id: String,
}

pub(crate) struct DetailCopyHit {
    pub(crate) value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetailMetadataTarget {
    Project,
    Status,
    Priority,
    Labels,
    Availability,
    Due,
}

fn detail_epic_children(
    item: &TaskListItem,
    removed: Option<&crate::tui::app::RemovedEpicChild>,
) -> Vec<DetailEpicChild> {
    let mut children = item
        .epic_children
        .iter()
        .cloned()
        .map(|link| DetailEpicChild {
            link,
            state: EpicChildState::Live,
        })
        .collect::<Vec<_>>();
    if let Some(removed) = removed
        && removed.epic_id == item.task.id
        && !children
            .iter()
            .any(|child| child.link.task_id == removed.child.task_id)
    {
        let position = removed.original_position.min(children.len());
        children.insert(
            position,
            DetailEpicChild {
                link: removed.child.clone(),
                state: EpicChildState::Removed,
            },
        );
    }
    children
}

impl DetailDocument {
    pub(crate) fn build(item: &TaskListItem, context: &DetailRenderContext<'_>) -> Self {
        let layout = context.content_layout();
        let inline_images = context.inline_images.cloned().map(|mut images| {
            images.focused_attachment_id = None;
            images
        });
        let epic_children = detail_epic_children(item, context.removed_epic_child);
        let sticky_lines = detail_header_options(
            item,
            layout.content_area.width as usize,
            context.inline_title_editor,
        );
        let body = build_detail_body_document(
            item,
            &epic_children,
            layout.content_area.width as usize,
            context.expanded_sections,
            inline_images.as_ref(),
            context.pending_attachments,
        );
        let model = project_detail_content_model(
            sticky_lines,
            &body,
            layout.content_area.height as usize,
            context.scroll,
        );
        let selectable = detail_selectable_document_from_body(
            item,
            layout.content_area.width as usize,
            context.inline_title_editor.is_none(),
            &body,
        );
        Self {
            source: item.clone(),
            epic_children,
            layout,
            scroll: context.scroll,
            expanded_sections: context.expanded_sections.clone(),
            inline_images,
            pending_attachments: context.pending_attachments.to_vec(),
            inline_title_editor: context
                .inline_title_editor
                .map(|editor| (editor.input.clone(), editor.cursor)),
            model,
            hyperlinks: body.hyperlinks,
            selectable,
            section_body_indices: body.section_body_indices,
            #[cfg(test)]
            projection_id: next_detail_projection_id(),
        }
    }

    fn render(
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
                &self.selectable,
                selection,
                &mut model.sticky_lines,
                &mut model.lines,
                model.body_start,
            );
        }
        render_detail_content_from_model(frame, self.layout.content_area, &model, widgets);
        if self.layout.metadata_area.width > 0 {
            render_detail_metadata(frame, item, &self.epic_children, self.layout.metadata_area);
        }
    }

    pub(crate) fn matches_frame(
        &self,
        item: &TaskListItem,
        context: &DetailRenderContext<'_>,
    ) -> bool {
        self.source == *item
            && self.epic_children == detail_epic_children(item, context.removed_epic_child)
            && self.layout == context.content_layout()
            && self.scroll == context.scroll
            && self.expanded_sections == *context.expanded_sections
            && detail_inline_image_geometry_matches(
                self.inline_images.as_ref(),
                context.inline_images,
            )
            && self.pending_attachments == context.pending_attachments
            && self.inline_title_editor.as_ref()
                == context
                    .inline_title_editor
                    .map(|editor| (&editor.input, editor.cursor))
                    .map(|(input, cursor)| (input.clone(), cursor))
                    .as_ref()
    }

    fn sticky_height(&self) -> usize {
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
        &self.model.interactive_rows
    }

    pub(crate) fn focus_targets(&self, item: &TaskListItem) -> Vec<DetailTargetId> {
        self.model
            .interactive_rows
            .iter()
            .map(|row| &row.target)
            .filter(|target| match target {
                DetailTargetId::Task {
                    section: DetailSection::EpicChildren,
                    task_id,
                } => self
                    .epic_children
                    .iter()
                    .any(|child| &child.link.task_id == task_id),
                DetailTargetId::Expand {
                    section: DetailSection::EpicChildren,
                } => self.epic_children.len() > 5,
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
        self.hyperlinks
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
        self.model
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
            .model
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
        let selectable = if let Some(title) = self.selectable.title.get(title_row) {
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
            self.selectable
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
        if selection.task_id != self.source.task.id {
            return None;
        }
        self.selectable
            .text
            .get(selection.range())
            .map(str::to_string)
    }

    #[cfg(test)]
    pub(crate) fn projection_id(&self) -> usize {
        self.projection_id
    }
}

#[cfg(test)]
fn next_detail_projection_id() -> usize {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

fn render_detail(
    frame: &mut Frame,
    item: &TaskListItem,
    context: &DetailRenderContext<'_>,
    widgets: &mut WidgetState,
) {
    let document = widgets
        .detail_document
        .as_ref()
        .filter(|document| document.matches_frame(item, context))
        .cloned()
        .unwrap_or_else(|| std::rc::Rc::new(DetailDocument::build(item, context)));
    document.render(frame, item, context, widgets);
    widgets.detail_document = Some(document);
}

fn detail_query_context<'a>(
    terminal_width: u16,
    terminal_height: u16,
    scroll: u16,
    expanded_sections: &'a BTreeSet<DetailSection>,
    inline_images: Option<&'a DetailInlineImageContext>,
) -> DetailRenderContext<'a> {
    DetailRenderContext {
        terminal_area: Rect::new(0, 0, terminal_width, terminal_height),
        scroll,
        inline_title_editor: None,
        active_target: None,
        hovered_target: None,
        expanded_sections,
        selection: None,
        inline_images,
        pending_attachments: &[],
        removed_epic_child: None,
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
    let expanded_sections = BTreeSet::new();
    DetailDocument::build(
        item,
        &detail_query_context(
            terminal_width,
            terminal_height,
            0,
            &expanded_sections,
            inline_images,
        ),
    )
    .scroll_cap()
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

#[cfg(test)]
pub(crate) fn detail_interactive_rows(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    inline_images: Option<&DetailInlineImageContext>,
    expanded_sections: &BTreeSet<DetailSection>,
) -> Vec<DetailInteractiveRow> {
    DetailDocument::build(
        item,
        &detail_query_context(
            terminal_width,
            terminal_height,
            0,
            expanded_sections,
            inline_images,
        ),
    )
    .interactive_rows()
    .to_vec()
}

#[cfg(test)]
pub(crate) fn detail_attachment_scroll_target(
    item: &TaskListItem,
    attachment_id: &str,
    scroll: u16,
    terminal_width: u16,
    terminal_height: u16,
    inline_images: &DetailInlineImageContext,
) -> Option<u16> {
    let expanded_sections = BTreeSet::new();
    let document = DetailDocument::build(
        item,
        &detail_query_context(
            terminal_width,
            terminal_height,
            scroll,
            &expanded_sections,
            Some(inline_images),
        ),
    );
    document.target_scroll_target(
        &DetailTargetId::Attachment {
            attachment_id: attachment_id.to_string(),
        },
        scroll,
    )
}

#[cfg(test)]
pub(crate) fn detail_section_scroll_target_with_images(
    item: &TaskListItem,
    scroll: u16,
    terminal_width: u16,
    terminal_height: u16,
    reverse: bool,
    inline_images: Option<&DetailInlineImageContext>,
) -> u16 {
    let expanded_sections = BTreeSet::new();
    DetailDocument::build(
        item,
        &detail_query_context(
            terminal_width,
            terminal_height,
            scroll,
            &expanded_sections,
            inline_images,
        ),
    )
    .section_scroll_target(reverse)
}

fn detail_content_margin() -> Margin {
    Margin {
        horizontal: 2,
        vertical: 1,
    }
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn build_detail_content_model(
    item: &TaskListItem,
    area: Rect,
    scroll: u16,
    inline_title_editor: Option<&TextInputView>,
    active_target: Option<&DetailTargetId>,
    expanded_sections: &BTreeSet<DetailSection>,
    selection: Option<&DetailTextSelection>,
    inline_images: Option<&DetailInlineImageContext>,
) -> DetailContentRenderModel {
    build_detail_content_model_with_pending(
        item,
        area,
        scroll,
        inline_title_editor,
        active_target,
        expanded_sections,
        selection,
        inline_images,
        &[],
    )
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
fn build_detail_content_model_with_pending(
    item: &TaskListItem,
    area: Rect,
    scroll: u16,
    inline_title_editor: Option<&TextInputView>,
    active_target: Option<&DetailTargetId>,
    expanded_sections: &BTreeSet<DetailSection>,
    selection: Option<&DetailTextSelection>,
    inline_images: Option<&DetailInlineImageContext>,
    pending_attachments: &[crate::tui::attachment_controller::PendingAttachmentView],
) -> DetailContentRenderModel {
    let epic_children = detail_epic_children(item, None);
    let body = build_detail_body_document(
        item,
        &epic_children,
        area.width as usize,
        expanded_sections,
        inline_images,
        pending_attachments,
    );
    let mut model = project_detail_content_model(
        detail_header_options(item, area.width as usize, inline_title_editor),
        &body,
        area.height as usize,
        scroll,
    );
    if let Some(active_target) = active_target {
        apply_active_style(&mut model, active_target);
    }
    if inline_title_editor.is_none()
        && let Some(selection) = selection.filter(|selection| selection.task_id == item.task.id)
    {
        let selectable =
            detail_selectable_document_from_body(item, area.width as usize, true, &body);
        apply_detail_selection_from_document(
            &selectable,
            selection,
            &mut model.sticky_lines,
            &mut model.lines,
            model.body_start,
        );
    }
    model
}

fn project_detail_content_model(
    sticky_lines: Vec<Line<'static>>,
    body: &DetailBodyDocument,
    area_height: usize,
    scroll: u16,
) -> DetailContentRenderModel {
    let content_height = body.lines.len().max(1);
    let sticky_height = sticky_lines.len().min(area_height);
    let visible = area_height.saturating_sub(sticky_height);
    let start = clamp_scroll_start(scroll, content_height, visible.max(1));
    let lines = body.lines.iter().skip(start).cloned().collect();
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
        image_placements: body.image_placements.clone(),
        interactive_rows: body.interactive_rows.clone(),
    }
}

fn detail_inline_image_geometry_matches(
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
    let start = row.line_index.saturating_sub(model.body_start);
    let end = start.saturating_add(row.height).min(model.lines.len());
    let lines_len = model.lines.len();
    Some(&mut model.lines[start.min(lines_len)..end])
}

fn apply_active_style(model: &mut DetailContentRenderModel, target: &DetailTargetId) {
    let Some(lines) = interactive_row_lines_mut(model, target) else {
        return;
    };
    match target {
        DetailTargetId::Expand { .. } => {
            for line in lines {
                for (index, span) in line.spans.iter_mut().enumerate() {
                    span.style = span.style.bg(BG_PANEL);
                    if index == 0 {
                        span.style = span.style.fg(BORDER);
                    } else {
                        span.style = span.style.fg(ACCENT).add_modifier(Modifier::BOLD);
                    }
                }
            }
        }
        DetailTargetId::Note { .. } => {
            for line in lines {
                for span in &mut line.spans {
                    span.style = span.style.bg(BG_PANEL);
                }
            }
        }
        DetailTargetId::Attachment { .. } => {
            for line in lines {
                for span in line.spans.iter_mut().skip(1) {
                    span.style = span.style.fg(ACCENT);
                }
            }
        }
        DetailTargetId::Task {
            section: DetailSection::EpicParent | DetailSection::EpicChildren,
            ..
        } => {
            for line in lines {
                for (index, span) in line.spans.iter_mut().enumerate() {
                    span.style = span.style.bg(BG_PANEL);
                    match index {
                        0 => span.style = span.style.fg(BORDER),
                        1 => {
                            span.style = span.style.fg(ACCENT).add_modifier(Modifier::BOLD);
                        }
                        2 => span.style = span.style.fg(FG_DIM),
                        _ => {}
                    }
                }
            }
        }
        DetailTargetId::Task { .. } => apply_link_row_style(lines),
    }
}

fn apply_hover_style(model: &mut DetailContentRenderModel, target: &DetailTargetId) {
    let Some(lines) = interactive_row_lines_mut(model, target) else {
        return;
    };
    for line in lines {
        for span in &mut line.spans {
            if matches!(target, DetailTargetId::Note { .. }) {
                span.style = span.style.bg(BG_PANEL);
            } else {
                span.style = span.style.add_modifier(Modifier::UNDERLINED);
            }
        }
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

#[cfg(test)]
fn detail_body_lines_with_images(
    item: &TaskListItem,
    width: usize,
    hovered_child_task_id: Option<&str>,
    inline_images: Option<&DetailInlineImageContext>,
) -> (
    Vec<Line<'static>>,
    Vec<DetailBodyImagePlacement>,
    Vec<DetailBodyAttachmentPlacement>,
    Vec<DetailInteractiveRow>,
) {
    let target = hovered_child_task_id.map(|task_id| DetailTargetId::Task {
        section: DetailSection::EpicChildren,
        task_id: crate::ids::TaskId::try_from(task_id.to_string()).expect("valid test task ID"),
    });
    detail_body_lines_with_pending_images(
        item,
        width,
        target.as_ref(),
        &BTreeSet::new(),
        inline_images,
        &[],
    )
}

#[cfg(test)]
fn detail_body_lines_with_pending_images(
    item: &TaskListItem,
    width: usize,
    active_target: Option<&DetailTargetId>,
    expanded_sections: &BTreeSet<DetailSection>,
    inline_images: Option<&DetailInlineImageContext>,
    pending_attachments: &[crate::tui::attachment_controller::PendingAttachmentView],
) -> (
    Vec<Line<'static>>,
    Vec<DetailBodyImagePlacement>,
    Vec<DetailBodyAttachmentPlacement>,
    Vec<DetailInteractiveRow>,
) {
    let epic_children = detail_epic_children(item, None);
    let body = build_detail_body_document(
        item,
        &epic_children,
        width,
        expanded_sections,
        inline_images,
        pending_attachments,
    );
    let mut model = project_detail_content_model(Vec::new(), &body, usize::MAX, 0);
    if let Some(active_target) = active_target {
        apply_active_style(&mut model, active_target);
    }
    let attachment_placements = body
        .interactive_rows
        .iter()
        .filter_map(|row| match &row.target {
            DetailTargetId::Attachment { attachment_id } => Some(DetailBodyAttachmentPlacement {
                attachment_id: attachment_id.clone(),
                line_index: row.line_index,
                height: row.height,
            }),
            _ => None,
        })
        .collect();
    (
        model.lines,
        body.image_placements,
        attachment_placements,
        body.interactive_rows,
    )
}

fn build_detail_body_document(
    item: &TaskListItem,
    epic_children: &[DetailEpicChild],
    width: usize,
    expanded_sections: &BTreeSet<DetailSection>,
    inline_images: Option<&DetailInlineImageContext>,
    pending_attachments: &[crate::tui::attachment_controller::PendingAttachmentView],
) -> DetailBodyDocument {
    let mut lines = Vec::new();
    let mut interactive_rows = Vec::new();
    let mut section_body_indices = vec![0];
    extend_epic_parent_section(&mut lines, &mut interactive_rows, item, width, None);
    extend_epic_children_section(
        &mut lines,
        &mut interactive_rows,
        item,
        epic_children,
        width,
        None,
        expanded_sections.contains(&DetailSection::EpicChildren),
    );
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    section_body_indices.push(lines.len());

    let mut image_placements = Vec::new();
    let mut hyperlinks = Vec::new();
    let mut selectable_description = Vec::new();
    let mut selectable_text = String::new();
    let description = description_or_placeholder(&item.task.description);
    let content_width = width.saturating_sub(3).max(1);
    let blocks = detail_body_blocks(
        &description,
        content_width,
        MarkdownRenderContext,
        inline_images,
    );
    let rendered_description = blocks
        .iter()
        .map(|block| match block {
            DetailBodyBlock::Line(line) => line.clone(),
            DetailBodyBlock::Image { placeholder, .. } => placeholder.clone(),
        })
        .collect::<Vec<_>>();
    hyperlinks.extend(markdown_hyperlinks(
        &description,
        &rendered_description,
        lines.len(),
        2,
    ));
    for (index, block) in blocks.into_iter().enumerate() {
        let selectable_line = match &block {
            DetailBodyBlock::Line(line) => line.to_string(),
            DetailBodyBlock::Image { placeholder, .. } => placeholder.to_string(),
        };
        if !item.task.description.is_empty() {
            if index > 0 {
                selectable_text.push('\n');
            }
            let document_start = selectable_text.len();
            selectable_text.push_str(&selectable_line);
            selectable_description.push(SelectableLine {
                text: selectable_line,
                document_start,
                body_index: Some(lines.len()),
            });
        }
        match block {
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
                let line_index = lines.len().saturating_add(1);
                lines.push(quoted_line(placeholder, Style::new().fg(FG_MUTED)));
                for _ in 0..height {
                    lines.push(Line::from(vec![Span::styled(
                        "│ ",
                        Style::new().fg(BORDER),
                    )]));
                }
                image_placements.push(DetailBodyImagePlacement {
                    attachment_id,
                    source_hash,
                    line_index,
                    width,
                    height,
                });
            }
        }
    }

    let mut attachment_placements = Vec::new();
    extend_attachment_section(
        &mut lines,
        &mut image_placements,
        &mut attachment_placements,
        &item.attachments,
        width,
        inline_images,
    );
    for placement in attachment_placements {
        interactive_rows.push(DetailInteractiveRow {
            target: DetailTargetId::Attachment {
                attachment_id: placement.attachment_id,
            },
            line_index: placement.line_index,
            height: placement.height,
        });
    }
    extend_pending_attachment_section(
        &mut lines,
        &item.task.id,
        pending_attachments,
        item.attachments
            .iter()
            .any(|attachment| !attachment.deleted),
    );
    lines.push(Line::from(""));
    section_body_indices.push(lines.len());
    extend_detail_note_section(
        &mut lines,
        &mut interactive_rows,
        &mut hyperlinks,
        item,
        width,
    );
    if !item.depends_on.is_empty() || !item.blocks.is_empty() {
        let dependency_start = lines.len();
        extend_dependency_sections(
            &mut lines,
            &mut interactive_rows,
            item,
            width,
            None,
            expanded_sections,
        );
        section_body_indices.push(dependency_start.saturating_add(1));
    }
    section_body_indices.sort_unstable();
    section_body_indices.dedup();

    DetailBodyDocument {
        lines,
        image_placements,
        interactive_rows,
        hyperlinks,
        selectable_description,
        selectable_text,
        section_body_indices,
    }
}

#[cfg(test)]
fn detail_selectable_document(
    item: &TaskListItem,
    width: usize,
    inline_images: Option<&DetailInlineImageContext>,
) -> DetailSelectableDocument {
    let epic_children = detail_epic_children(item, None);
    let body = build_detail_body_document(
        item,
        &epic_children,
        width,
        &BTreeSet::new(),
        inline_images,
        &[],
    );
    detail_selectable_document_from_body(item, width, true, &body)
}

fn detail_selectable_document_from_body(
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

fn apply_detail_selection_from_document(
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
    for selectable in &document.description {
        if let Some(line) = selectable
            .body_index
            .and_then(|index| index.checked_sub(body_start))
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

#[cfg(test)]
fn detail_section_body_indices(
    item: &TaskListItem,
    width: usize,
    inline_images: Option<&DetailInlineImageContext>,
) -> Vec<usize> {
    build_detail_body_document(
        item,
        &detail_epic_children(item, None),
        width,
        &BTreeSet::new(),
        inline_images,
        &[],
    )
    .section_body_indices
}

fn extend_detail_note_section(
    lines: &mut Vec<Line<'static>>,
    rows: &mut Vec<DetailInteractiveRow>,
    hyperlinks: &mut Vec<DetailHyperlink>,
    item: &TaskListItem,
    width: usize,
) {
    let mut header = vec![
        Span::styled(
            "NOTES",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" (", Style::new().fg(FG_DIM)),
        Span::styled("n", keycap_style()),
        Span::styled(" add", Style::new().fg(FG_DIM)),
    ];
    if !item.notes.is_empty() && width >= 42 {
        header.extend([
            Span::styled(" · ", Style::new().fg(FG_DIM)),
            Span::styled("e", keycap_style()),
            Span::styled(" edit · ", Style::new().fg(FG_DIM)),
            Span::styled("D", keycap_style()),
            Span::styled(" delete", Style::new().fg(FG_DIM)),
        ]);
    }
    header.push(Span::styled(")", Style::new().fg(FG_DIM)));
    lines.push(Line::from(header));
    if item.notes.is_empty() {
        lines.push(Line::from(Span::styled("none", Style::new().fg(FG_MUTED))));
    } else {
        for note in &item.notes {
            lines.push(Line::from(""));
            let mut rendered = vec![Line::from(vec![
                Span::styled(
                    local_timestamp_display(&note.created_at),
                    Style::new().fg(FG_DIM),
                ),
                Span::styled("  you", Style::new().fg(ACCENT)),
            ])];
            let note_lines = quoted_block_lines(&note.body, width, Style::new().fg(FG));
            let unquoted_note_lines =
                render_markdown_without_link_urls(&note.body, width.saturating_sub(3).max(1));
            hyperlinks.extend(markdown_hyperlinks(
                &note.body,
                &unquoted_note_lines,
                lines.len().saturating_add(1),
                2,
            ));
            rendered.extend(note_lines);
            push_interactive_lines(
                lines,
                rows,
                DetailTargetId::Note {
                    note_id: note.id.clone(),
                },
                rendered,
            );
        }
    }
}

fn push_interactive_lines(
    lines: &mut Vec<Line<'static>>,
    rows: &mut Vec<DetailInteractiveRow>,
    target: DetailTargetId,
    rendered: Vec<Line<'static>>,
) {
    let line_index = lines.len();
    let height = rendered.len();
    lines.extend(rendered);
    rows.push(DetailInteractiveRow {
        target,
        line_index,
        height,
    });
}

fn extend_epic_parent_section(
    lines: &mut Vec<Line<'static>>,
    rows: &mut Vec<DetailInteractiveRow>,
    item: &TaskListItem,
    width: usize,
    active_target: Option<&DetailTargetId>,
) {
    let Some(parent) = &item.epic_parent else {
        return;
    };
    lines.push(Line::from(Span::styled(
        "EPIC PARENT",
        Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
    )));
    let target = DetailTargetId::Task {
        section: DetailSection::EpicParent,
        task_id: parent.task_id.clone(),
    };
    let active = active_target == Some(&target);
    let rendered = epic_child_tree_item_lines(parent, EpicChildState::Live, true, width, active);
    push_interactive_lines(lines, rows, target, rendered);
}

fn epic_child_counts(children: &[DetailEpicChild]) -> EpicChildCounts {
    EpicChildCounts {
        open: children
            .iter()
            .filter(|child| child.state == EpicChildState::Live && child.link.unresolved)
            .count(),
        total: children
            .iter()
            .filter(|child| child.state == EpicChildState::Live)
            .count(),
    }
}

fn ordered_epic_children(children: &[DetailEpicChild]) -> Vec<&DetailEpicChild> {
    children
        .iter()
        .filter(|child| child.link.unresolved)
        .chain(children.iter().filter(|child| !child.link.unresolved))
        .collect()
}

fn extend_epic_children_section(
    lines: &mut Vec<Line<'static>>,
    rows: &mut Vec<DetailInteractiveRow>,
    item: &TaskListItem,
    children: &[DetailEpicChild],
    width: usize,
    active_target: Option<&DetailTargetId>,
    expanded: bool,
) {
    if !item.task.is_epic {
        return;
    }
    if !lines.is_empty() {
        lines.push(Line::from(""));
    }
    let counts = epic_child_counts(children);
    lines.push(Line::from(vec![
        Span::styled(
            "CHILD TASKS",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" open={} total={}", counts.open, counts.total),
            Style::new().fg(FG_DIM),
        ),
    ]));
    let links = ordered_epic_children(children);
    if links.is_empty() {
        lines.push(Line::from(Span::styled("none", Style::new().fg(FG_MUTED))));
        return;
    }
    let visible = if expanded {
        links.len()
    } else {
        links.len().min(5)
    };
    let has_disclosure = links.len() > 5;
    for (index, child) in links.iter().take(visible).enumerate() {
        let target = DetailTargetId::Task {
            section: DetailSection::EpicChildren,
            task_id: child.link.task_id.clone(),
        };
        let is_last = index + 1 == visible && !has_disclosure;
        let rendered = epic_child_tree_item_lines(
            &child.link,
            child.state,
            is_last,
            width,
            active_target == Some(&target),
        );
        push_interactive_lines(lines, rows, target, rendered);
    }
    if has_disclosure {
        let target = DetailTargetId::Expand {
            section: DetailSection::EpicChildren,
        };
        let label = if expanded {
            "Show less".to_string()
        } else {
            format!("Show {} more", links.len() - visible)
        };
        push_disclosure_row(lines, rows, target, &label, active_target);
    }
}

fn push_disclosure_row(
    lines: &mut Vec<Line<'static>>,
    rows: &mut Vec<DetailInteractiveRow>,
    target: DetailTargetId,
    label: &str,
    active_target: Option<&DetailTargetId>,
) {
    let active = active_target == Some(&target);
    push_interactive_lines(lines, rows, target, vec![disclosure_line(label, active)]);
}

fn disclosure_line(label: &str, active: bool) -> Line<'static> {
    let style = if active {
        Style::new()
            .fg(ACCENT)
            .bg(BG_PANEL)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_MUTED)
    };
    let tree_style = if active {
        Style::new().fg(BORDER).bg(BG_PANEL)
    } else {
        Style::new().fg(BORDER)
    };
    Line::from(vec![
        Span::styled("└─ ", tree_style),
        Span::styled(label.to_string(), style),
    ])
}

fn epic_child_tree_item_lines(
    link: &crate::query::TaskDependencyLink,
    state: EpicChildState,
    is_last: bool,
    width: usize,
    hovered: bool,
) -> Vec<Line<'static>> {
    let tree_glyph = if is_last { "└─ " } else { "├─ " };
    let removed = state == EpicChildState::Removed;
    let ref_style = if hovered {
        Style::new()
            .fg(ACCENT)
            .bg(BG_PANEL)
            .add_modifier(Modifier::BOLD)
    } else if removed {
        Style::new().fg(FG_MUTED).add_modifier(Modifier::DIM)
    } else {
        Style::new().fg(ACCENT)
    };
    let title_style = if hovered {
        Style::new().fg(FG).bg(BG_PANEL)
    } else if removed {
        Style::new().fg(FG_MUTED).add_modifier(Modifier::DIM)
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
    let title = if removed {
        format!("{}  [removed]", link.title)
    } else {
        link.title.clone()
    };
    dependency_node_lines_with_title_style(
        prefix,
        &title,
        &link.status,
        &link.priority,
        width,
        title_style,
    )
}

fn extend_dependency_sections(
    lines: &mut Vec<Line<'static>>,
    rows: &mut Vec<DetailInteractiveRow>,
    item: &TaskListItem,
    width: usize,
    active_target: Option<&DetailTargetId>,
    expanded_sections: &BTreeSet<DetailSection>,
) {
    extend_dependency_section(
        lines,
        rows,
        "WHY BLOCKED",
        &item.depends_on,
        DependencyDirection::Blocker,
        DetailSection::DependsOn,
        width,
        active_target,
        expanded_sections.contains(&DetailSection::DependsOn),
    );
    extend_dependency_section(
        lines,
        rows,
        "WHAT THIS UNLOCKS",
        &item.blocks,
        DependencyDirection::Dependent,
        DetailSection::Blocks,
        width,
        active_target,
        expanded_sections.contains(&DetailSection::Blocks),
    );
}

#[allow(clippy::too_many_arguments)]
fn extend_dependency_section(
    lines: &mut Vec<Line<'static>>,
    rows: &mut Vec<DetailInteractiveRow>,
    label: &'static str,
    links: &[crate::query::TaskDependencyLink],
    direction: DependencyDirection,
    section: DetailSection,
    width: usize,
    active_target: Option<&DetailTargetId>,
    expanded: bool,
) {
    if links.is_empty() {
        return;
    }
    lines.push(Line::from(""));
    lines.push(dependency_heading(label, links));
    let visible = if expanded {
        links.len()
    } else {
        links.len().min(DETAIL_DEPENDENCY_TREE_CAP)
    };
    let has_disclosure = links.len() > DETAIL_DEPENDENCY_TREE_CAP;
    for (index, link) in links.iter().take(visible).enumerate() {
        let target = DetailTargetId::Task {
            section,
            task_id: link.task_id.clone(),
        };
        let mut rendered = dependency_tree_item_lines(
            link,
            direction,
            index + 1 == visible && !has_disclosure,
            width,
        );
        if active_target == Some(&target) {
            apply_link_row_style(&mut rendered);
        }
        push_interactive_lines(lines, rows, target, rendered);
    }
    if has_disclosure {
        let target = DetailTargetId::Expand { section };
        let label = if expanded {
            "Show less".to_string()
        } else {
            format!("Show {} more", links.len() - visible)
        };
        push_disclosure_row(lines, rows, target, &label, active_target);
    }
}

fn apply_link_row_style(lines: &mut [Line<'static>]) {
    for line in lines {
        for span in &mut line.spans {
            span.style = span.style.bg(BG_PANEL);
        }
    }
}

#[cfg(test)]
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

#[cfg(test)]
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
    let mut lines = detail_title_lines(item, width, inline_title_editor);
    lines.extend([
        Line::from(Span::styled("─".repeat(width), Style::new().fg(BORDER))),
        Line::from(summary_spans),
        Line::from(""),
    ]);
    lines
}

fn detail_title_lines(
    item: &TaskListItem,
    width: usize,
    inline_title_editor: Option<&TextInputView>,
) -> Vec<Line<'static>> {
    if let Some(editor) = inline_title_editor {
        let mut line = clipped_input_line(&editor.input, editor.cursor, width);
        for span in &mut line.spans {
            span.style = Style::new()
                .fg(FG)
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
                Style::new().fg(FG).add_modifier(Modifier::BOLD),
            ))
        })
        .collect()
}

fn title_line_ranges(title: &str, width: usize) -> Vec<std::ops::Range<usize>> {
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

struct ParsedMarkdownLink {
    label: String,
    url: String,
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

fn markdown_hyperlinks(
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

fn quoted_block_lines(body: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    let content_width = width.saturating_sub(3).max(1);
    render_markdown_without_link_urls(body, content_width)
        .into_iter()
        .map(|line| {
            let mut spans = line_with_base_style(line, style).spans;
            spans.insert(0, Span::styled("│ ", Style::new().fg(BORDER)));
            Line::from(spans)
        })
        .collect()
}

fn extend_pending_attachment_section(
    lines: &mut Vec<Line<'static>>,
    task_id: &crate::ids::TaskId,
    pending_attachments: &[crate::tui::attachment_controller::PendingAttachmentView],
    has_live_attachments: bool,
) {
    let pending = pending_attachments
        .iter()
        .filter(|attachment| &attachment.task_id == task_id)
        .collect::<Vec<_>>();
    if pending.is_empty() {
        return;
    }
    if !has_live_attachments {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "ATTACHMENTS",
            Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
        )));
    }
    for attachment in pending {
        let (label, color) = match attachment.status {
            crate::tui::attachment_controller::PendingAttachmentStatus::Preparing => {
                ("[image: preparing]", FG_MUTED)
            }
            crate::tui::attachment_controller::PendingAttachmentStatus::Failed => {
                ("[image: failed]", crate::tui::theme::RED)
            }
        };
        lines.push(quoted_line(Line::from(label), Style::new().fg(color)));
    }
}

fn extend_attachment_section(
    lines: &mut Vec<Line<'static>>,
    placements: &mut Vec<DetailBodyImagePlacement>,
    attachment_placements: &mut Vec<DetailBodyAttachmentPlacement>,
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
                let focused = inline_images.is_some_and(|context| {
                    context.focused_attachment_id.as_deref()
                        == Some(attachment.attachment_id.as_str())
                });
                attachment_placements.push(DetailBodyAttachmentPlacement {
                    attachment_id: attachment.attachment_id.clone(),
                    line_index: lines.len(),
                    height: 1,
                });
                lines.push(quoted_line(
                    line,
                    Style::new().fg(if focused { ACCENT } else { FG_MUTED }),
                ));
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
                attachment_placements.push(DetailBodyAttachmentPlacement {
                    attachment_id: attachment_id.clone(),
                    line_index: lines.len(),
                    height: height as usize + 3,
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

pub(crate) fn attachment_is_locally_openable(attachment: &AttachmentMetadataJson) -> bool {
    !attachment.deleted
        && attachment.has_blob
        && attachment.bytes_state == crate::attachments::AttachmentBytesState::Present
        && matches!(
            attachment.media_type.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp"
        )
}

pub(crate) fn attachment_is_locally_previewable(
    attachment: &AttachmentMetadataJson,
    unavailable_hashes: &HashSet<String>,
) -> bool {
    attachment_is_locally_openable(attachment)
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
    let focused = inline_images.is_some_and(|context| {
        context.focused_attachment_id.as_deref() == Some(attachment.attachment_id.as_str())
    });
    let placeholder = attachment_detail_line(attachment, content_width, focused);
    let Some(inline_images) = inline_images else {
        return DetailBodyBlock::Line(placeholder);
    };
    if !inline_images.previews_enabled
        || !attachment_is_locally_previewable(attachment, &inline_images.unavailable_hashes)
    {
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

fn attachment_detail_line(
    attachment: &AttachmentMetadataJson,
    content_width: usize,
    focused: bool,
) -> Line<'static> {
    let state_style = Style::new().fg(if focused { ACCENT } else { FG_MUTED });
    let filename_style = Style::new().fg(if focused { ACCENT } else { FG });
    let separator_style = Style::new().fg(if focused { ACCENT } else { FG_DIM });
    let metadata_style = Style::new().fg(if focused { ACCENT } else { FG_MUTED });
    let mut spans = vec![Span::styled(
        attachment_state_placeholder(attachment),
        state_style,
    )];
    if let Some(filename) = attachment.filename.as_deref() {
        spans.push(Span::styled(format!(" {filename}"), filename_style));
    }
    if let (Some(width), Some(height)) = (attachment.width, attachment.height) {
        spans.push(Span::styled(" · ", separator_style));
        spans.push(Span::styled(format!("{width}×{height}"), metadata_style));
    }
    spans.push(Span::styled(" · ", separator_style));
    spans.push(Span::styled(
        human_file_size(attachment.byte_size),
        metadata_style,
    ));
    truncate_styled_line(Line::from(spans), content_width)
}

fn truncate_styled_line(line: Line<'static>, max_width: usize) -> Line<'static> {
    use unicode_width::UnicodeWidthChar;

    if UnicodeWidthStr::width(line.to_string().as_str()) <= max_width {
        return line;
    }
    if max_width == 0 {
        return Line::default();
    }

    let target_width = max_width - 1;
    let mut used_width = 0;
    let mut truncated = Vec::new();
    let mut ellipsis_style = Style::default();
    'spans: for span in line.spans {
        let mut content = String::new();
        ellipsis_style = span.style;
        for character in span.content.chars() {
            let character_width = UnicodeWidthChar::width(character).unwrap_or(0);
            if used_width + character_width > target_width {
                if !content.is_empty() {
                    truncated.push(Span::styled(content, span.style));
                }
                break 'spans;
            }
            content.push(character);
            used_width += character_width;
        }
        if !content.is_empty() {
            truncated.push(Span::styled(content, span.style));
        }
    }
    truncated.push(Span::styled("…", ellipsis_style));
    Line::from(truncated)
}

fn detail_body_blocks(
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

fn render_detail_metadata(
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

#[cfg(test)]
fn detail_metadata_lines(item: &TaskListItem, width: usize) -> Vec<Line<'static>> {
    detail_metadata_lines_with_children(item, &detail_epic_children(item, None), width)
}

fn detail_metadata_lines_with_children(
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
    )
    .unwrap_or_else(|| ["none".to_string(), String::new()]);
    let availability_style = if item.task.available_at.is_some() {
        Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(FG_MUTED)
    };
    lines.extend([
        Line::from(""),
        metadata_label("AVAILABILITY"),
        Line::from(Span::styled(
            truncate_width(&availability[0], width),
            availability_style,
        )),
        Line::from(Span::styled(
            truncate_width(&availability[1], width),
            availability_style,
        )),
    ]);
    let due =
        crate::tui::time::due_summary_lines(item.task.due_on.as_deref().unwrap_or(""), now_seconds)
            .unwrap_or_else(|| ["none".to_string(), String::new()]);
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
    lines.extend([
        Line::from(""),
        metadata_label("DUE"),
        Line::from(Span::styled(truncate_width(&due[0], width), due_style)),
        Line::from(Span::styled(truncate_width(&due[1], width), due_style)),
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

fn metadata_label(label: &'static str) -> Line<'static> {
    Line::from(Span::styled(
        label,
        Style::new().fg(FG_DIM).add_modifier(Modifier::BOLD),
    ))
}

#[cfg(test)]
pub(crate) fn detail_text_cell_at_position(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    column: u16,
    row: u16,
    scroll: u16,
) -> Option<TextCell> {
    let expanded_sections = BTreeSet::new();
    DetailDocument::build(
        item,
        &detail_query_context(
            terminal_width,
            terminal_height,
            scroll,
            &expanded_sections,
            None,
        ),
    )
    .text_cell_at_position(column, row)
}

pub(crate) fn detail_selected_text(
    item: &TaskListItem,
    selection: &DetailTextSelection,
) -> Option<String> {
    let expanded_sections = BTreeSet::new();
    DetailDocument::build(
        item,
        &detail_query_context(selection.terminal_width, 24, 0, &expanded_sections, None),
    )
    .selected_text(selection)
}

#[cfg(test)]
pub(crate) fn detail_attachment_at_position(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    column: u16,
    row: u16,
    scroll: u16,
    inline_images: &DetailInlineImageContext,
) -> Option<DetailAttachmentHit> {
    let expanded_sections = BTreeSet::new();
    DetailDocument::build(
        item,
        &detail_query_context(
            terminal_width,
            terminal_height,
            scroll,
            &expanded_sections,
            Some(inline_images),
        ),
    )
    .attachment_at_position(item, column, row)
    .map(|attachment_id| DetailAttachmentHit { attachment_id })
}

#[cfg(test)]
pub(crate) fn detail_child_task_at_position(
    item: &TaskListItem,
    terminal_width: u16,
    terminal_height: u16,
    column: u16,
    row: u16,
    scroll: u16,
) -> Option<DetailChildHit> {
    let expanded_sections = BTreeSet::new();
    DetailDocument::build(
        item,
        &detail_query_context(
            terminal_width,
            terminal_height,
            scroll,
            &expanded_sections,
            None,
        ),
    )
    .child_task_at_position(column, row)
    .map(|task_id| DetailChildHit { task_id })
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
        23 => item.display_ref.clone(),
        26 => local_timestamp_display(&item.task.created_at),
        29 => local_timestamp_display(&item.task.updated_at),
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
        3 => DetailMetadataTarget::Project,
        6 => DetailMetadataTarget::Status,
        9 => DetailMetadataTarget::Priority,
        12 => DetailMetadataTarget::Labels,
        15 | 16 => DetailMetadataTarget::Availability,
        19 | 20 => DetailMetadataTarget::Due,
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

fn render_attachment_preview_message(frame: &mut Frame, area: Rect, message: &'static str) {
    frame.render_widget(
        Paragraph::new(message)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::new().fg(FG_MUTED).bg(BG)),
        area,
    );
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
        render_attachment_preview_message(frame, area, "attachment is unavailable");
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
        render_attachment_preview_message(frame, inner, "preview unavailable");
        return;
    };
    if inline_images
        .unavailable_hashes
        .contains(&attachment.sha256)
    {
        render_attachment_preview_message(frame, inner, "preview unavailable");
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
    selected_task: Option<usize>,
    scroll: u16,
    inline_title_editor: Option<&TextInputView>,
    active_target: Option<&DetailTargetId>,
    hovered_target: Option<&DetailTargetId>,
    expanded_sections: &BTreeSet<DetailSection>,
    selection: Option<&DetailTextSelection>,
    inline_images: Option<&DetailInlineImageContext>,
    pending_attachments: &[crate::tui::attachment_controller::PendingAttachmentView],
    removed_epic_child: Option<&crate::tui::app::RemovedEpicChild>,
) {
    if let Some(task) = store.selected_task(selected_task) {
        let context = DetailRenderContext {
            terminal_area: frame.area(),
            scroll,
            inline_title_editor,
            active_target,
            hovered_target,
            expanded_sections,
            selection,
            inline_images,
            pending_attachments,
            removed_epic_child,
        };
        render_detail(frame, task, &context, widgets);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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
    fn detail_header_wraps_the_complete_title_before_metadata() {
        let mut item = detail_test_item();
        item.task.title = "Implement wrapped task titles in the detail pane".to_string();

        let lines = detail_header_options(&item, 18, None);
        let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();

        assert_eq!(
            &rendered[..3],
            &["Implement wrapped", "task titles in the", "detail pane"]
        );
        assert_eq!(rendered[3], "─".repeat(18));
        assert!(rendered[4].contains(&item.display_ref));
        assert!(!rendered.join("\n").contains('…'));
    }

    #[test]
    fn detail_header_breaks_words_that_exceed_narrow_widths() {
        let mut item = detail_test_item();
        item.task.title = "abcdefghij".to_string();

        let lines = detail_title_lines(&item, 4, None);
        let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();

        assert_eq!(rendered, ["abcd", "efgh", "ij"]);
        assert_eq!(rendered.concat(), item.task.title);
        assert!(lines.iter().all(|line| line.width() <= 4));
    }

    #[test]
    fn detail_header_wraps_by_unicode_display_width() {
        let mut item = detail_test_item();
        item.task.title = "ab界cd 界界界".to_string();

        let lines = detail_title_lines(&item, 4, None);
        let rendered = lines.iter().map(Line::to_string).collect::<Vec<_>>();

        assert_eq!(rendered, ["ab界", "cd", "界界", "界"]);
        assert!(lines.iter().all(|line| line.width() <= 4));

        item.task.title = "👩‍💻x".to_string();
        let emoji = detail_title_lines(&item, 2, None)
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>();
        assert_eq!(emoji, ["👩‍💻", "x"]);
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
    fn detail_body_shows_epic_parent_relationship() {
        let mut item = detail_test_item();
        item.epic_parent = Some(crate::query::TaskDependencyLink {
            task_id: crate::test_support::task_id("epic-task-id"),
            display_ref: "APP-EPIC".to_string(),
            title: "Ship authentication reliability".to_string(),
            status: "active".to_string(),
            priority: "high".to_string(),
            unresolved: true,
        });

        let lines = detail_body_lines(&item, 60, None);

        assert_eq!(lines[0].to_string(), "EPIC PARENT");
        assert!(lines[1].to_string().contains("APP-EPIC"));
        assert!(
            lines
                .iter()
                .map(Line::to_string)
                .collect::<Vec<_>>()
                .join(" ")
                .contains("Ship authentication")
        );
    }

    #[test]
    fn detail_epic_parent_relationship_wraps_to_width() {
        let mut item = detail_test_item();
        item.epic_parent = Some(crate::query::TaskDependencyLink {
            task_id: crate::test_support::task_id("epic-task-id"),
            display_ref: "APP-EPIC".to_string(),
            title: "A long epic title that must fit the sticky header".to_string(),
            status: "active".to_string(),
            priority: "high".to_string(),
            unresolved: true,
        });

        let lines = detail_body_lines(&item, 32, None);

        assert!(lines.iter().all(|line| line.width() <= 32));
        assert!(lines.len() > 2);
    }

    #[test]
    fn detail_document_projections_share_semantic_body_geometry() {
        let mut item = detail_test_epic_item();
        item.epic_parent = Some(crate::query::TaskDependencyLink {
            task_id: crate::test_support::task_id("epic-parent-id"),
            display_ref: "APP-EPIC".to_string(),
            title: "Parent epic".to_string(),
            status: "active".to_string(),
            priority: "high".to_string(),
            unresolved: true,
        });
        item.task.description =
            "A wrapped description with enough words to occupy several rows.".to_string();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        let images = DetailInlineImageContext::default();
        let children = detail_epic_children(&item, None);
        let body =
            build_detail_body_document(&item, &children, 42, &BTreeSet::new(), Some(&images), &[]);
        let selectable = detail_selectable_document_from_body(&item, 42, true, &body);
        let model = project_detail_content_model(Vec::new(), &body, usize::MAX, 0);

        assert_eq!(model.content_height, body.lines.len());
        assert_eq!(model.interactive_rows, body.interactive_rows);
        assert_eq!(model.image_placements.len(), body.image_placements.len());
        for line in &selectable.description {
            let body_index = line.body_index.expect("description body index");
            assert_eq!(
                body.lines[body_index].to_string(),
                format!("│ {}", line.text)
            );
        }
        for row in &body.interactive_rows {
            assert!(row.line_index < body.lines.len());
            assert!(row.line_index + row.height <= body.lines.len());
        }
        let section_lines = body
            .section_body_indices
            .iter()
            .map(|index| body.lines[*index].to_string())
            .collect::<Vec<_>>();
        assert_eq!(section_lines[0], "EPIC PARENT");
        assert!(section_lines[1].starts_with("│ A wrapped description"));
        assert_eq!(section_lines[2], "NOTES (n add · e edit · D delete)");
        assert_eq!(section_lines[3], "WHY BLOCKED open=1 total=1");
    }

    #[test]
    fn detail_projection_orders_every_relationship_section() {
        let mut item = detail_test_item();
        item.task.is_epic = true;
        item.epic_parent = Some(crate::query::TaskDependencyLink {
            task_id: crate::test_support::task_id("epic-parent-id"),
            display_ref: "APP-EPIC".to_string(),
            title: "Parent epic".to_string(),
            status: "active".to_string(),
            priority: "high".to_string(),
            unresolved: true,
        });
        item.epic_children = vec![crate::query::TaskDependencyLink {
            task_id: crate::test_support::task_id("epic-child-id"),
            display_ref: "APP-CHILD".to_string(),
            title: "Child task".to_string(),
            status: "todo".to_string(),
            priority: "medium".to_string(),
            unresolved: true,
        }];
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];

        let sections = detail_interactive_rows(
            &item,
            100,
            40,
            Some(&DetailInlineImageContext::default()),
            &BTreeSet::new(),
        )
        .into_iter()
        .map(|row| row.target.section())
        .fold(Vec::new(), |mut sections, section| {
            if sections.last() != Some(&section) {
                sections.push(section);
            }
            sections
        });

        assert_eq!(
            sections,
            vec![
                DetailSection::EpicParent,
                DetailSection::EpicChildren,
                DetailSection::Attachments,
                DetailSection::Notes,
                DetailSection::DependsOn,
                DetailSection::Blocks,
            ]
        );
    }

    #[test]
    fn detail_projection_expands_long_dependency_sections() {
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

        let collapsed = detail_interactive_rows(&item, 80, 24, None, &BTreeSet::new());
        assert_eq!(
            collapsed
                .iter()
                .filter(|row| row.target.section() == DetailSection::DependsOn)
                .count(),
            4
        );
        assert!(matches!(
            collapsed.last().map(|row| &row.target),
            Some(DetailTargetId::Expand {
                section: DetailSection::DependsOn
            })
        ));

        let expanded = detail_interactive_rows(
            &item,
            80,
            24,
            None,
            &[DetailSection::DependsOn].into_iter().collect(),
        );
        assert_eq!(
            expanded
                .iter()
                .filter(|row| row.target.section() == DetailSection::DependsOn)
                .count(),
            6
        );
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
        assert!(rendered.contains("└─ Show 2 more"));
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
        assert!(rendered.contains("└─ Show 2 more"));
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
    fn detail_selection_maps_and_highlights_across_wrapped_title_lines() {
        let mut item = detail_test_item();
        item.task.title = "first wrapped second line".to_string();
        let expanded_sections = BTreeSet::new();
        let context = detail_query_context(20, 24, 0, &expanded_sections, None);
        let document = DetailDocument::build(&item, &context);
        let layout = detail_content_layout(context.terminal_area);
        let first = document
            .text_cell_at_position(layout.content_area.x, layout.content_area.y)
            .unwrap();
        let focus = document
            .text_cell_at_position(
                layout.content_area.x + "second line".width() as u16 - 1,
                layout.content_area.y + 1,
            )
            .unwrap();
        let mut selection = DetailTextSelection::new(item.task.id.clone(), 20, first);
        selection.focus = focus;

        assert_eq!(document.sticky_height(), 5);
        assert_eq!(
            document.selected_text(&selection).as_deref(),
            Some(item.task.title.as_str())
        );

        let model = build_detail_content_model(
            &item,
            layout.content_area,
            0,
            None,
            None,
            &expanded_sections,
            Some(&selection),
            None,
        );
        for line in &model.sticky_lines[..2] {
            assert!(line.spans.iter().any(|span| span.style.bg == Some(ACCENT)));
        }
    }

    #[test]
    fn inline_title_editing_stays_on_one_clipped_line() {
        let mut item = detail_test_item();
        item.task.title = "a committed title long enough to wrap".to_string();
        let editor = TextInputView {
            kind: crate::tui::overlay::TextInputKind::EditTitle,
            title: "Edit title".to_string(),
            prompt: String::new(),
            input: "an edited title that remains horizontal".to_string(),
            cursor: 30,
        };

        let lines = detail_header_options(&item, 10, Some(&editor));

        assert_eq!(lines.len(), 4);
        assert!(lines[0].width() <= 10);
        assert_eq!(lines[1].to_string(), "─".repeat(10));
        assert_ne!(lines[0].to_string(), editor.input);
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
            &BTreeSet::new(),
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
    fn detail_markdown_links_hide_destinations_and_have_mouse_targets() {
        let mut item = detail_test_item();
        item.task.description =
            "See the [Aven guide](https://aven.raine.dev/guide/) for details.".to_string();
        item.notes[0].body = "Review the [task docs](https://aven.raine.dev/tasks/).".to_string();
        let expanded_sections = BTreeSet::new();
        let context = detail_query_context(100, 30, 0, &expanded_sections, None);
        let document = DetailDocument::build(&item, &context);
        let rendered = document
            .model
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");
        let link = document.hyperlinks.first().expect("description link");
        let row = document
            .layout
            .content_area
            .y
            .saturating_add(document.sticky_height() as u16)
            .saturating_add(link.line_index.saturating_sub(document.model.body_start) as u16);
        let column = document
            .layout
            .content_area
            .x
            .saturating_add(link.start_column as u16);

        assert!(rendered.contains("See the Aven guide for details."));
        assert!(rendered.contains("Review the task docs."));
        assert!(!rendered.contains("https://"));
        assert_eq!(
            document
                .hyperlinks
                .iter()
                .map(|link| link.url.as_str())
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([
                "https://aven.raine.dev/guide/",
                "https://aven.raine.dev/tasks/",
            ])
        );
        assert_eq!(
            document.link_at_position(column, row).as_deref(),
            Some("https://aven.raine.dev/guide/")
        );
        assert!(
            document
                .link_at_position(column.saturating_sub(1), row)
                .is_none()
        );
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
    fn removed_epic_child_is_labeled_and_excluded_from_counts() {
        let mut item = detail_test_epic_item();
        let child = item.epic_children.remove(0);
        item.epic_children.clear();
        let removed = crate::tui::app::RemovedEpicChild {
            epic_id: item.task.id.clone(),
            child,
            original_position: 0,
        };
        let children = detail_epic_children(&item, Some(&removed));
        let body = build_detail_body_document(&item, &children, 80, &BTreeSet::new(), None, &[]);
        let rendered = body
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("CHILD TASKS open=0 total=0"));
        assert!(rendered.contains("Build the first child task  [removed]"));
        assert_eq!(children[0].state, EpicChildState::Removed);
    }

    #[test]
    fn legitimate_removed_suffix_remains_a_live_epic_child_title() {
        let mut item = detail_test_epic_item();
        item.epic_children.truncate(1);
        item.epic_children[0].title = "Investigate literal [removed]".to_string();

        let children = detail_epic_children(&item, None);
        let body = build_detail_body_document(&item, &children, 80, &BTreeSet::new(), None, &[]);
        let child_line = body
            .lines
            .iter()
            .find(|line| line.to_string().contains("APP-CHLD"))
            .expect("child row");
        let child_ref = child_line
            .spans
            .iter()
            .find(|span| span.content.as_ref() == "APP-CHLD")
            .expect("child ref");
        let rendered = body
            .lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("CHILD TASKS open=1 total=1"));
        assert!(rendered.contains("Investigate literal [removed]"));
        assert_eq!(children[0].state, EpicChildState::Live);
        assert_eq!(child_ref.style.fg, Some(ACCENT));
        assert!(!child_ref.style.add_modifier.contains(Modifier::DIM));
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
        assert!(rendered.contains("AVAILABILITY\nnone"));
        assert!(rendered.contains("DUE\nnone"));
        assert!(rendered.contains("CONFLICTS\nyes"));
    }

    #[test]
    fn detail_metadata_includes_recurrence_state_and_history() {
        let mut item = detail_test_item();
        let series_id: aven_core::recurrence::RecurrenceSeriesId =
            "7KQ9A1X4MV2P8D6R".parse().unwrap();
        item.recurrence = Some(crate::query::TaskRecurrenceSummary {
            series_id: series_id.clone(),
            series_ref: "RCR-A1".to_string(),
            slot_on: "2026-07-20".to_string(),
            rule_label: "weekdays at 09:00".to_string(),
            timezone: "Europe/Helsinki".to_string(),
            lifecycle: aven_core::recurrence::RecurrenceSeriesState::Paused,
            outcome: Some(aven_core::recurrence::RecurrenceOutcome::Skipped),
            projection_state: aven_core::recurrence::RecurrenceProjectionState::Archived,
        });
        item.recurrence_group = Some(crate::query::RecurrenceTaskGroup {
            series_id,
            series_ref: "RCR-A1".to_string(),
            counts: crate::query::RecurrenceCounts {
                series_ref: "RCR-A1".to_string(),
                completed: 8,
                skipped: 3,
                missed: 2,
                ..crate::query::RecurrenceCounts::default()
            },
        });

        let rendered = detail_metadata_lines(&item, 31)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("RECURRENCE\n↻ RCR-A1"));
        assert!(rendered.contains("schedule weekdays at 09:00"));
        assert!(rendered.contains("slot 2026-07-20"));
        assert!(rendered.contains("zone Europe/Helsinki"));
        assert!(rendered.contains("lifecycle paused"));
        assert!(rendered.contains("outcome skipped"));
        assert!(rendered.contains("projection archived"));
        assert!(rendered.contains("history t r h"));
        assert!(rendered.contains("SERIES HISTORY\ncompleted 8\nskipped 3\nmissed 2"));
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
        let item = detail_test_epic_item();

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
    fn detail_metadata_uses_shared_epic_rollup_semantics() {
        let mut item = detail_test_epic_item();
        item.epic_rollup = Some(crate::query::EpicRollup {
            total: 4,
            open: 2,
            done: 1,
            canceled: 1,
            blocked: 1,
            overdue: 1,
            ready: 1,
            latest_activity_at: "2026-06-21T00:00:00Z".to_string(),
        });

        let rendered = detail_metadata_lines(&item, 40)
            .into_iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("CHILDREN\n2 open · 1 done · 1 canceled"));
        assert!(rendered.contains("1 overdue · 1 blocked · 1 ready"));
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
            (88, 25, item.display_ref.clone()),
            (88, 28, local_timestamp_display(&item.task.created_at)),
            (88, 31, local_timestamp_display(&item.task.updated_at)),
        ];

        for (column, row, value) in expected {
            assert_eq!(
                detail_copy_target_at(&item, 120, 40, column, row).map(|hit| hit.value),
                Some(value)
            );
        }
        assert!(detail_copy_target_at(&item, 120, 40, 87, 25).is_none());
        assert!(detail_copy_target_at(&item, 120, 40, 88, 24).is_none());
        assert!(detail_copy_target_at(&item, 80, 40, 70, 28).is_none());
    }

    #[test]
    fn detail_metadata_target_maps_editable_values_without_body_conflicts() {
        let expected = [
            (5, DetailMetadataTarget::Project),
            (8, DetailMetadataTarget::Status),
            (11, DetailMetadataTarget::Priority),
            (14, DetailMetadataTarget::Labels),
            (17, DetailMetadataTarget::Availability),
            (18, DetailMetadataTarget::Availability),
            (21, DetailMetadataTarget::Due),
            (22, DetailMetadataTarget::Due),
        ];

        for (row, target) in expected {
            assert_eq!(
                detail_metadata_target_at(120, 40, 88, row),
                Some((target, 88, row))
            );
        }
        assert_eq!(detail_metadata_target_at(120, 40, 88, 7), None);
        assert_eq!(detail_metadata_target_at(120, 40, 50, 8), None);
        assert_eq!(detail_metadata_target_at(80, 40, 70, 11), None);
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

        let model = build_detail_content_model(
            &item,
            Rect::new(0, 0, 60, 5),
            4,
            None,
            None,
            &BTreeSet::new(),
            None,
            None,
        );

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
    fn detail_renders_pending_and_failed_attachment_rows() {
        let item = detail_test_item();
        let pending = vec![
            crate::tui::attachment_controller::PendingAttachmentView {
                attachment_id: "PENDINGATTACH01".to_string(),
                task_id: item.task.id.clone(),
                status: crate::tui::attachment_controller::PendingAttachmentStatus::Preparing,
            },
            crate::tui::attachment_controller::PendingAttachmentView {
                attachment_id: "PENDINGATTACH02".to_string(),
                task_id: item.task.id.clone(),
                status: crate::tui::attachment_controller::PendingAttachmentStatus::Failed,
            },
        ];

        let rendered = detail_body_lines_with_pending_images(
            &item,
            60,
            None,
            &BTreeSet::new(),
            None,
            &pending,
        )
        .0
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

        assert!(rendered.contains("ATTACHMENTS"));
        assert!(rendered.contains("[image: preparing]"));
        assert!(rendered.contains("[image: failed]"));
    }

    #[test]
    fn detail_empty_description_renders_attachment_section() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        item.attachments[0].filename = None;

        let rendered = detail_content_lines(&item, 80, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("ATTACHMENTS\n│ [image: attachment]"));
    }

    #[test]
    fn detail_attachment_rows_show_filename_and_generic_fallback() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![
            attachment_metadata("ATTACHMENT000001", false, true),
            attachment_metadata("ATTACHMENT000002", false, true),
        ];
        item.attachments[0].filename = Some("super_aïti_floral_transparent.png".to_string());
        item.attachments[1].filename = None;

        let rendered = detail_content_lines(&item, 80, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();

        assert!(rendered.iter().any(|line| {
            line == "│ [image: attachment] super_aïti_floral_transparent.png · 640×480 · 4 B"
        }));
        assert!(
            rendered
                .iter()
                .any(|line| line == "│ [image: attachment] · 640×480 · 4 B")
        );
    }

    #[test]
    fn detail_attachment_filename_truncates_to_content_width() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];
        item.attachments[0].filename = Some("a-very-long-attachment-filename.png".to_string());

        let rendered = detail_content_lines(&item, 30, None)
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>();
        let attachment = rendered
            .iter()
            .find(|line| line.starts_with("│ [image: attachment]"))
            .expect("attachment row");

        assert_eq!(UnicodeWidthStr::width(attachment.as_str()), 29);
        assert!(attachment.ends_with('…'));
    }

    #[test]
    fn detail_attachment_row_styles_filename_and_metadata() {
        let attachment = attachment_metadata("ATTACHMENT000001", false, true);

        let line = attachment_detail_line(&attachment, 80, false);

        assert_eq!(
            line.spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<Vec<_>>(),
            vec![
                "[image: attachment]",
                " chart.png",
                " · ",
                "640×480",
                " · ",
                "4 B",
            ]
        );
        assert_eq!(line.spans[0].style.fg, Some(FG_MUTED));
        assert_eq!(line.spans[1].style.fg, Some(FG));
        assert_eq!(line.spans[2].style.fg, Some(FG_DIM));
        assert_eq!(line.spans[3].style.fg, Some(FG_MUTED));

        let focused = attachment_detail_line(&attachment, 80, true);
        assert!(
            focused
                .spans
                .iter()
                .all(|span| span.style.fg == Some(ACCENT))
        );
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

        let (lines, placements, _, _) =
            detail_body_lines_with_images(&item, 80, None, Some(&context));

        assert_eq!(placements.len(), 1);
        assert_eq!(placements[0].height, 12);
        assert!(lines.iter().any(|line| {
            line.to_string() == "│ [image: attachment] chart.png · 640×480 · 4 B"
        }));
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

        let (unfocused_lines, unfocused_placements, _, _) =
            detail_body_lines_with_images(&item, 80, None, Some(&unfocused));
        let (focused_lines, focused_placements, _, _) =
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

        let (_lines, placements, _, _) =
            detail_body_lines_with_images(&item, 200, None, Some(&context));

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

        let (lines, placements, _, _) =
            detail_body_lines_with_images(&item, 80, None, Some(&context));

        assert!(placements.is_empty());
        assert!(lines.iter().any(|line| {
            line.to_string() == "│ [image: attachment] chart.png · 640×480 · 4 B"
        }));
    }

    #[test]
    fn detail_falls_back_to_single_placeholder_when_previews_disabled() {
        let mut item = detail_test_item();
        item.task.description = String::new();
        item.attachments = vec![attachment_metadata("ATTACHMENT000001", false, true)];

        let (lines, placements, _, _) = detail_body_lines_with_images(&item, 80, None, None);

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
            &BTreeSet::new(),
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
            interactive_rows: Vec::new(),
        };
        let mut widgets = WidgetState {
            inline_image_placements: Vec::new(),
            detail_document: None,
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
            &BTreeSet::new(),
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
            &BTreeSet::new(),
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
                    &BTreeSet::new(),
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
            inline_image_placements: Vec::new(),
            detail_document: None,
        };
        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).unwrap();

        let expanded_sections = BTreeSet::new();
        terminal
            .draw(|frame| {
                let render_context = DetailRenderContext {
                    terminal_area: frame.area(),
                    scroll: 0,
                    inline_title_editor: None,
                    active_target: None,
                    hovered_target: None,
                    expanded_sections: &expanded_sections,
                    selection: None,
                    inline_images: Some(&context),
                    pending_attachments: &[],
                    removed_epic_child: None,
                };
                render_detail(frame, &item, &render_context, &mut widgets);
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

    #[test]
    fn cached_geometry_accepts_frame_specific_detail_styles() {
        let item = detail_test_item();
        let expanded_sections = BTreeSet::new();
        let images = DetailInlineImageContext::default();
        let base = DetailRenderContext {
            terminal_area: Rect::new(0, 0, 80, 24),
            scroll: 0,
            inline_title_editor: None,
            active_target: None,
            hovered_target: None,
            expanded_sections: &expanded_sections,
            selection: None,
            inline_images: Some(&images),
            pending_attachments: &[],
            removed_epic_child: None,
        };
        let document = DetailDocument::build(&item, &base);
        let active = DetailTargetId::Task {
            section: DetailSection::DependsOn,
            task_id: item.depends_on[0].task_id.clone(),
        };
        let hovered = DetailTargetId::Task {
            section: DetailSection::Blocks,
            task_id: item.blocks[0].task_id.clone(),
        };
        let selection =
            DetailTextSelection::new(item.task.id.clone(), 80, TextCell { start: 0, end: 3 });
        let focused_images = DetailInlineImageContext {
            focused_attachment_id: Some("attachment".to_string()),
            ..images.clone()
        };
        let styled = DetailRenderContext {
            terminal_area: base.terminal_area,
            scroll: 0,
            inline_title_editor: None,
            active_target: Some(&active),
            hovered_target: Some(&hovered),
            expanded_sections: &expanded_sections,
            selection: Some(&selection),
            inline_images: Some(&focused_images),
            pending_attachments: &[],
            removed_epic_child: None,
        };

        assert!(document.matches_frame(&item, &styled));
    }

    #[test]
    fn cached_geometry_invalidates_for_semantic_detail_changes() {
        let item = detail_test_item();
        let expanded_sections = BTreeSet::new();
        let images = DetailInlineImageContext::default();
        let base = DetailRenderContext {
            terminal_area: Rect::new(0, 0, 80, 24),
            scroll: 0,
            inline_title_editor: None,
            active_target: None,
            hovered_target: None,
            expanded_sections: &expanded_sections,
            selection: None,
            inline_images: Some(&images),
            pending_attachments: &[],
            removed_epic_child: None,
        };
        let document = DetailDocument::build(&item, &base);
        assert!(document.matches_frame(&item, &base));

        let mut changed_item = item.clone();
        changed_item.task.status = crate::choices::TaskStatus::Done;
        assert!(!document.matches_frame(&changed_item, &base));

        changed_item = item.clone();
        changed_item.notes[0].body = "Updated note".to_string();
        assert!(!document.matches_frame(&changed_item, &base));

        let epic = detail_test_epic_item();
        let epic_document = DetailDocument::build(&epic, &base);
        let mut changed_epic = epic.clone();
        changed_epic.epic_children[0].status = "done".to_string();
        changed_epic.epic_children[0].unresolved = false;
        assert!(!epic_document.matches_frame(&changed_epic, &base));

        let pending = [crate::tui::attachment_controller::PendingAttachmentView {
            attachment_id: "pending".to_string(),
            task_id: item.task.id.clone(),
            status: crate::tui::attachment_controller::PendingAttachmentStatus::Preparing,
        }];
        let pending_context = DetailRenderContext {
            pending_attachments: &pending,
            ..base
        };
        assert!(!document.matches_frame(&item, &pending_context));

        let editor = TextInputView {
            kind: crate::tui::overlay::TextInputKind::EditTitle,
            title: "Edit title".to_string(),
            prompt: String::new(),
            input: item.task.title.clone(),
            cursor: 2,
        };
        let editor_context = DetailRenderContext {
            inline_title_editor: Some(&editor),
            pending_attachments: &[],
            ..base
        };
        assert!(!document.matches_frame(&item, &editor_context));

        let expanded_sections = [DetailSection::DependsOn].into_iter().collect();
        let expanded_context = DetailRenderContext {
            expanded_sections: &expanded_sections,
            pending_attachments: &[],
            ..base
        };
        assert!(!document.matches_frame(&item, &expanded_context));
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
                source: crate::choices::TaskSource::Unknown,
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
                id: "note-id".to_string(),
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
            epic_rollup: None,
            recurrence: None,
            recurrence_group: None,
            attachments: Vec::new(),
            queue: Default::default(),
        }
    }
}
