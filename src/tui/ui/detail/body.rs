use std::collections::BTreeSet;
use std::rc::Rc;

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::super::recent_actions;
use super::super::task_display::description_or_placeholder;
use super::super::timestamps::{local_activity_timestamp_display, local_timestamp_display};
use crate::query::TaskListItem;
use crate::tui::app::{DetailSection, DetailTargetId};
use crate::tui::markdown::{MarkdownRenderContext, render_markdown_without_link_urls};
use crate::tui::text::truncate_width;
use crate::tui::theme;

use super::DetailInteractiveRow;
use super::attachments::{
    DetailBodyImagePlacement, DetailInlineImageContext, extend_attachment_section,
    extend_pending_attachment_section,
};
use super::relationships::{
    DetailEpicChild, extend_dependency_sections, extend_epic_children_section,
    extend_epic_parent_section, extend_related_section, push_disclosure_row,
};
use super::text::{
    DetailHyperlink, SelectableLine, detail_body_blocks, keycap_style, markdown_hyperlinks,
    plain_metadata_lines, quoted_block_lines, quoted_line,
};

#[derive(Debug, Clone)]
pub(super) struct DetailBodyDocument {
    pub(super) lines: Vec<Line<'static>>,
    pub(super) image_placements: Rc<Vec<DetailBodyImagePlacement>>,
    pub(super) interactive_rows: Rc<Vec<DetailInteractiveRow>>,
    pub(super) hyperlinks: Vec<DetailHyperlink>,
    pub(super) selectable_description: Vec<SelectableLine>,
    pub(super) selectable_text: String,
    pub(super) section_body_indices: Vec<usize>,
}

#[derive(Debug, Clone)]
pub(super) enum DetailBodyBlock {
    Line(Line<'static>),
    Image {
        placeholder: Line<'static>,
        attachment_id: String,
        source_hash: String,
        width: u16,
        height: u16,
    },
}
pub(super) fn build_detail_body_document(
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
                lines.push(quoted_line(line, Style::new().fg(theme::fg_muted())));
            }
            DetailBodyBlock::Image {
                placeholder,
                attachment_id,
                source_hash,
                width,
                height,
            } => {
                let line_index = lines.len().saturating_add(1);
                lines.push(quoted_line(placeholder, Style::new().fg(theme::fg_muted())));
                for _ in 0..height {
                    lines.push(Line::from(vec![Span::styled(
                        "│ ",
                        Style::new().fg(theme::border()),
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

    if !item.metadata.is_empty() {
        lines.push(Line::raw(""));
        section_body_indices.push(lines.len());
        interactive_rows.push(DetailInteractiveRow {
            target: DetailTargetId::CustomMetadata,
            line_index: lines.len(),
            height: 1,
        });
        lines.push(Line::from(vec![
            Span::styled(
                "CUSTOM METADATA",
                Style::new()
                    .fg(theme::fg_dim())
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" (", Style::new().fg(theme::fg_dim())),
            Span::styled("e", keycap_style()),
            Span::raw(" "),
            Span::styled("m", keycap_style()),
            Span::styled(" edit)", Style::new().fg(theme::fg_dim())),
        ]));
        let key_width = item
            .metadata
            .iter()
            .map(|value| value.key.width())
            .max()
            .unwrap_or(0)
            .min(24)
            .min(width / 3);
        for value in &item.metadata {
            let long_key = value.key.width() > key_width;
            if long_key {
                for mut line in plain_metadata_lines(
                    &value.key,
                    width.saturating_sub(2),
                    Style::new().fg(theme::fg_dim()),
                ) {
                    line.spans.insert(0, Span::raw("  "));
                    lines.push(line);
                }
            }
            let indent = if long_key { 4 } else { key_width + 4 };
            let (text, style) = (value.value.as_str(), Style::new().fg(theme::fg()));
            for (index, mut line) in plain_metadata_lines(text, width.saturating_sub(indent), style)
                .into_iter()
                .enumerate()
            {
                let prefix = if index == 0 && !long_key {
                    format!("  {:key_width$}  ", value.key)
                } else {
                    " ".repeat(indent)
                };
                line.spans
                    .insert(0, Span::styled(prefix, Style::new().fg(theme::fg_dim())));
                lines.push(line);
            }
            if value.value.contains('\n') || long_key {
                lines.push(Line::raw(""));
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
    if item
        .related
        .iter()
        .any(|link| !link.deleted || item.task.deleted)
    {
        let related_start = lines.len();
        extend_related_section(
            &mut lines,
            &mut interactive_rows,
            item,
            width,
            None,
            expanded_sections.contains(&DetailSection::Related),
        );
        section_body_indices.push(related_start.saturating_add(1));
    }
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
    let activity_start = lines.len();
    extend_activity_section(
        &mut lines,
        &mut interactive_rows,
        item,
        width,
        expanded_sections.contains(&DetailSection::Activity),
    );
    section_body_indices.push(activity_start.saturating_add(1));
    section_body_indices.sort_unstable();
    section_body_indices.dedup();

    DetailBodyDocument {
        lines,
        image_placements: Rc::new(image_placements),
        interactive_rows: Rc::new(interactive_rows),
        hyperlinks,
        selectable_description,
        selectable_text,
        section_body_indices,
    }
}

pub(super) fn extend_detail_note_section(
    lines: &mut Vec<Line<'static>>,
    rows: &mut Vec<DetailInteractiveRow>,
    hyperlinks: &mut Vec<DetailHyperlink>,
    item: &TaskListItem,
    width: usize,
) {
    let mut header = vec![
        Span::styled(
            "NOTES",
            Style::new()
                .fg(theme::fg_dim())
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(" (", Style::new().fg(theme::fg_dim())),
        Span::styled("n", keycap_style()),
        Span::styled(" add", Style::new().fg(theme::fg_dim())),
    ];
    if !item.notes.is_empty() && width >= 42 {
        header.extend([
            Span::styled(" · ", Style::new().fg(theme::fg_dim())),
            Span::styled("e", keycap_style()),
            Span::styled(" edit · ", Style::new().fg(theme::fg_dim())),
            Span::styled("D", keycap_style()),
            Span::styled(" delete", Style::new().fg(theme::fg_dim())),
        ]);
    }
    header.push(Span::styled(")", Style::new().fg(theme::fg_dim())));
    lines.push(Line::from(header));
    if item.notes.is_empty() {
        lines.push(Line::from(Span::styled(
            "none",
            Style::new().fg(theme::fg_muted()),
        )));
    } else {
        for note in &item.notes {
            lines.push(Line::from(""));
            let mut rendered = vec![Line::from(Span::styled(
                local_timestamp_display(&note.created_at),
                Style::new().fg(theme::fg_dim()),
            ))];
            let note_lines = quoted_block_lines(&note.body, width, Style::new().fg(theme::fg()));
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

pub(super) fn extend_activity_section(
    lines: &mut Vec<Line<'static>>,
    rows: &mut Vec<DetailInteractiveRow>,
    item: &TaskListItem,
    width: usize,
    expanded: bool,
) {
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "ACTIVITY",
        Style::new()
            .fg(theme::fg_dim())
            .add_modifier(Modifier::BOLD),
    )));
    if item.activity.is_empty() {
        lines.push(Line::from(Span::styled(
            "No recorded task activity.",
            Style::new().fg(theme::fg_muted()),
        )));
        return;
    }

    let disclosure = DetailTargetId::Expand {
        section: DetailSection::Activity,
    };
    if !expanded {
        let count = item.activity.len();
        let label = format!(
            "Show {count} {}",
            if count == 1 { "event" } else { "events" }
        );
        push_disclosure_row(lines, rows, disclosure, &label, None);
        return;
    }

    let available = item.queue.band == crate::queue::QueueBand::Available;
    let idle_index = item.queue_idle_activity_index();
    let idle_tag = item
        .queue
        .idle_seconds
        .map(crate::tui::time::compact_duration)
        .map(|duration| format!("idle {duration}"));
    let anchored_idle = idle_index
        .zip(idle_tag.as_deref())
        .filter(|(_, tag)| width.saturating_sub(19 + UnicodeWidthStr::width(*tag) + 2) >= 20);
    if available && let Some(tag) = idle_tag.as_deref() {
        lines.push(Line::from(Span::styled(
            truncate_width(&format!("{tag} · since becoming available"), width),
            Style::new().fg(theme::fg_dim()),
        )));
    } else if anchored_idle.is_none()
        && let Some((index, tag)) = idle_index.zip(idle_tag.as_deref())
    {
        lines.push(Line::from(Span::styled(
            truncate_width(
                &format!(
                    "{tag} · since latest {}",
                    idle_activity_noun(&item.activity[index])
                ),
                width,
            ),
            Style::new().fg(theme::fg_dim()),
        )));
    } else if idle_index.is_none()
        && let Some(tag) = idle_tag.as_deref()
    {
        lines.push(Line::from(Span::styled(
            truncate_width(&format!("{tag} · based on recent task activity"), width),
            Style::new().fg(theme::fg_dim()),
        )));
    }

    for (index, action) in item.activity.iter().enumerate() {
        let timestamp = local_activity_timestamp_display(&action.created_at);
        let icon = recent_actions::action_icon(action);
        let prefix_width =
            2 + UnicodeWidthStr::width(timestamp.as_str()) + 2 + UnicodeWidthStr::width(icon) + 1;
        let show_idle = anchored_idle.is_some_and(|(idle_index, _)| idle_index == index);
        let reserved = if show_idle {
            idle_tag
                .as_deref()
                .map(|tag| UnicodeWidthStr::width(tag) + 2)
                .unwrap_or(0)
        } else {
            0
        };
        let summary = truncate_width(
            &action.task_activity_summary(&item.task.title),
            width.saturating_sub(prefix_width + reserved),
        );
        let mut spans = vec![
            Span::raw("  "),
            Span::styled(timestamp, Style::new().fg(theme::fg_dim())),
            Span::raw("  "),
            Span::styled(icon, recent_actions::action_style(action)),
            Span::raw(" "),
            Span::styled(summary.clone(), Style::new().fg(theme::fg())),
        ];
        if show_idle && let Some(tag) = idle_tag.as_deref() {
            let padding = width.saturating_sub(
                prefix_width
                    + UnicodeWidthStr::width(summary.as_str())
                    + UnicodeWidthStr::width(tag),
            );
            spans.push(Span::raw(" ".repeat(padding.max(2))));
            spans.push(Span::styled(
                tag.to_string(),
                Style::new().fg(theme::fg_dim()),
            ));
        }
        lines.push(Line::from(spans));
    }
    push_disclosure_row(lines, rows, disclosure, "Hide activity", None);
}

fn idle_activity_noun(action: &crate::query::RecentActionItem) -> &'static str {
    match action.op_type.as_str() {
        crate::change_log::op_type::CREATE_TASK => "creation",
        crate::change_log::op_type::NOTE_ADD
        | crate::change_log::op_type::NOTE_EDIT
        | crate::change_log::op_type::NOTE_DELETE => "note",
        _ if action.field.as_deref() == Some("priority") => "priority change",
        _ => "status change",
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
