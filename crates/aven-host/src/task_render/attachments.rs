use aven_core::operations::AttachmentReadItem;

use crate::attachments::AttachmentBytesState;
use crate::render::{KvLine, yes_no};

pub(super) fn attachment_availability(attachment: &AttachmentMetadataJson) -> &'static str {
    match attachment.bytes_state {
        AttachmentBytesState::Present => "Present in Aven",
        AttachmentBytesState::PendingDownload => "Pending download",
        AttachmentBytesState::Unavailable => "Unavailable",
    }
}
pub fn print_attachment_section(attachments: &[AttachmentMetadataJson]) {
    let live = attachments
        .iter()
        .filter(|attachment| !attachment.deleted)
        .collect::<Vec<_>>();
    if live.is_empty() {
        return;
    }
    println!("Attachments:");
    for attachment in live {
        print_attachment_metadata_line(attachment);
    }
}

pub fn print_attachment_metadata_line(attachment: &AttachmentMetadataJson) {
    let line = KvLine::new("attachment")
        .field("attachment_id", &attachment.attachment_id)
        .field("media_type", &attachment.media_type)
        .field("byte_size", attachment.byte_size)
        .field("deleted", yes_no(attachment.deleted))
        .field("has_blob", yes_no(attachment.has_blob));
    println!("{}", line.finish());
}

#[allow(dead_code)]
pub fn attachment_placeholder(attachment: &AttachmentMetadataJson) -> String {
    let placeholder = attachment_state_placeholder(attachment);
    let filename = attachment
        .filename
        .as_deref()
        .map(|filename| format!(" {filename}"))
        .unwrap_or_default();
    let dimensions = match (attachment.width, attachment.height) {
        (Some(width), Some(height)) => format!(" · {width}×{height}"),
        _ => String::new(),
    };
    let file_size = human_file_size(attachment.byte_size);
    format!("{placeholder}{filename}{dimensions} · {file_size}")
}

pub fn attachment_state_placeholder(attachment: &AttachmentMetadataJson) -> &'static str {
    if attachment.deleted {
        "[image: deleted attachment]"
    } else {
        match attachment.bytes_state {
            AttachmentBytesState::Present => "[image: attachment]",
            AttachmentBytesState::PendingDownload => "[image: pending download]",
            AttachmentBytesState::Unavailable => "[image: unavailable bytes]",
        }
    }
}

pub fn human_file_size(byte_size: i64) -> String {
    const KIB: i64 = 1024;
    const MIB: i64 = KIB * 1024;

    if byte_size < KIB {
        format!("{byte_size} B")
    } else if byte_size < MIB {
        format!("{:.1} KiB", byte_size as f64 / KIB as f64)
    } else {
        format!("{:.1} MiB", byte_size as f64 / MIB as f64)
    }
}

#[cfg(any(test, feature = "test-support"))]
pub fn attachment_unavailable_placeholder(_attachment: &AttachmentMetadataJson) -> String {
    "[image: unavailable bytes]".to_string()
}
pub fn attachment_metadata_json(item: AttachmentReadItem) -> AttachmentMetadataJson {
    AttachmentMetadataJson {
        attachment_id: item.attachment.attachment_id,
        task_id: item.attachment.task_id.to_string(),
        sha256: item.attachment.sha256,
        media_type: item.attachment.media_type,
        byte_size: item.attachment.byte_size,
        filename: item.attachment.filename,
        alt_text: item.attachment.alt_text,
        width: item.attachment.width,
        height: item.attachment.height,
        created_at: item.attachment.created_at,
        deleted: item.attachment.deleted,
        deleted_at: item.attachment.deleted_at,
        bytes_state: item.bytes_state,
        has_blob: item.has_blob,
    }
}

pub type AttachmentMetadataJson = crate::query::AttachmentMetadata;
