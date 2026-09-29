mod attachments;
mod body;
mod document;
mod metadata;
mod relationships;
mod text;

pub(crate) use attachments::{
    DetailInlineImageContext, DetailInlineImagePlacement, attachment_is_locally_openable,
    attachment_is_locally_previewable, render_attachment_preview,
};
pub(crate) use document::{DetailDocument, DetailInteractiveRow, DetailRenderContext};
pub(crate) use metadata::{DetailMetadataTarget, detail_copy_target_at, detail_metadata_target_at};

#[cfg(test)]
use crate::tui::theme;
#[cfg(test)]
use attachments::{DetailBodyAttachmentPlacement, DetailBodyImagePlacement};
#[cfg(test)]
use body::build_detail_body_document;
#[cfg(test)]
use document::{
    DetailContentRenderModel, apply_active_style, detail_body_area, detail_content_layout,
    project_detail_content_model, render_detail_content_from_model, visible_detail_image_rect,
};
#[cfg(test)]
use metadata::detail_metadata_lines_with_children;
#[cfg(test)]
use ratatui::style::Style;
use relationships::DETAIL_DEPENDENCY_TREE_CAP;
#[cfg(test)]
use relationships::{
    DependencyDirection, EpicChildState, dependency_heading, dependency_tree_item_lines,
    detail_epic_children, epic_child_dependency_lines,
};
#[cfg(test)]
use text::{
    DetailSelectableDocument, apply_detail_selection_from_document, detail_header_options,
    detail_selectable_document_from_body,
};

use ratatui::Frame;
use ratatui::layout::Rect;
#[cfg(test)]
use ratatui::text::{Line, Span};
use std::collections::BTreeSet;

use crate::query::TaskListItem;
use crate::tui::app::{DetailSection, DetailTargetId, WidgetState};
use crate::tui::detail_selection::{DetailTextSelection, TextCell};
use crate::tui::overlay::TextInputView;
use crate::tui::store::{DetailRevision, TuiStore};
#[cfg(test)]
use unicode_width::UnicodeWidthStr;
pub(crate) fn detail_target_is_actionable(item: &TaskListItem, target: &DetailTargetId) -> bool {
    match target {
        DetailTargetId::CustomMetadata => !item.metadata.is_empty(),
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
            DetailSection::Related => item
                .related
                .iter()
                .any(|link| (!link.deleted || item.task.deleted) && &link.task_id == task_id),
            DetailSection::CustomMetadata
            | DetailSection::Attachments
            | DetailSection::Notes
            | DetailSection::Activity => false,
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
            DetailSection::Related => {
                item.related
                    .iter()
                    .filter(|link| !link.deleted || item.task.deleted)
                    .count()
                    > DETAIL_DEPENDENCY_TREE_CAP
            }
            DetailSection::Activity => !item.activity.is_empty(),
            DetailSection::CustomMetadata
            | DetailSection::EpicParent
            | DetailSection::Attachments
            | DetailSection::Notes => false,
        },
    }
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
    let document = DetailDocument::reuse_or_build(widgets.detail_document.as_ref(), item, context);
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
        detail_revision: DetailRevision::UNCACHED,
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
        body.image_placements.to_vec(),
        attachment_placements,
        body.interactive_rows.to_vec(),
    )
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
            Span::styled("└─ ", Style::new().fg(theme::border())),
            Span::styled(
                format!("+{hidden} more"),
                Style::new().fg(theme::fg_muted()),
            ),
        ]));
    }

    lines
}

#[cfg(test)]
fn detail_metadata_lines(item: &TaskListItem, width: usize) -> Vec<Line<'static>> {
    detail_metadata_lines_with_children(item, &detail_epic_children(item, None), width)
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
            detail_revision: store.tasks.revision(),
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
mod tests;
