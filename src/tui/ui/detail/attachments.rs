use std::collections::HashSet;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::super::truncate::truncate_line_width;
use crate::query::TaskListItem;
use crate::task_render::{AttachmentMetadataJson, attachment_state_placeholder, human_file_size};
use crate::tui::app::WidgetState;
use crate::tui::theme::{self, BG};

use super::body::DetailBodyBlock;
use super::document::detail_body_area;
use super::text::quoted_line;

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
pub(super) struct DetailBodyImagePlacement {
    pub(super) attachment_id: String,
    pub(super) source_hash: String,
    pub(super) line_index: usize,
    pub(super) width: u16,
    pub(super) height: u16,
}

#[derive(Debug, Clone)]
pub(super) struct DetailBodyAttachmentPlacement {
    pub(super) attachment_id: String,
    pub(super) line_index: usize,
    pub(super) height: usize,
}
pub(super) fn extend_pending_attachment_section(
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
            Style::new()
                .fg(theme::fg_dim())
                .add_modifier(Modifier::BOLD),
        )));
    }
    for attachment in pending {
        let (label, color) = match attachment.status {
            crate::tui::attachment_controller::PendingAttachmentStatus::Preparing => {
                ("[image: preparing]", theme::fg_muted())
            }
            crate::tui::attachment_controller::PendingAttachmentStatus::Failed => {
                ("[image: failed]", theme::red())
            }
        };
        lines.push(quoted_line(Line::from(label), Style::new().fg(color)));
    }
}

pub(super) fn extend_attachment_section(
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
        Style::new()
            .fg(theme::fg_dim())
            .add_modifier(Modifier::BOLD),
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
                    Style::new().fg(if focused {
                        theme::accent()
                    } else {
                        theme::fg_muted()
                    }),
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
                let frame_style = Style::new().fg(if focused {
                    theme::accent()
                } else {
                    theme::border()
                });
                lines.push(quoted_line(placeholder, Style::new().fg(theme::fg_muted())));
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
                        Style::new().fg(theme::fg_muted()),
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

pub(super) fn attachment_detail_block(
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

pub(super) fn attachment_detail_line(
    attachment: &AttachmentMetadataJson,
    content_width: usize,
    focused: bool,
) -> Line<'static> {
    let state_style = Style::new().fg(if focused {
        theme::accent()
    } else {
        theme::fg_muted()
    });
    let filename_style = Style::new().fg(if focused {
        theme::accent()
    } else {
        theme::fg()
    });
    let separator_style = Style::new().fg(if focused {
        theme::accent()
    } else {
        theme::fg_dim()
    });
    let metadata_style = Style::new().fg(if focused {
        theme::accent()
    } else {
        theme::fg_muted()
    });
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

pub(super) fn truncate_styled_line(line: Line<'static>, max_width: usize) -> Line<'static> {
    truncate_line_width(line, max_width, Style::default())
}

pub(super) fn image_preview_size(
    attachment: &AttachmentMetadataJson,
    content_width: usize,
) -> (u16, u16) {
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

pub(super) fn render_attachment_preview_message(
    frame: &mut Frame,
    area: Rect,
    message: &'static str,
) {
    frame.render_widget(
        Paragraph::new(message)
            .alignment(ratatui::layout::Alignment::Center)
            .style(Style::new().fg(theme::fg_muted()).bg(BG)),
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
        .border_style(Style::new().fg(theme::accent()))
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

pub(super) fn fitted_image_size(
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
