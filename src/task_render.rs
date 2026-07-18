use serde::Serialize;

use crate::attachments::AttachmentBytesState;
use crate::operations::AttachmentReadItem;
use crate::query::{TaskDependencyLink, TaskDependencySummary, TaskListItem};
use crate::render::{KvLine, print_multiline_block, quote, yes_no};

pub(crate) fn print_task_line_item(item: &TaskListItem) {
    let labels = item.labels.join(",");
    let line = KvLine::new(item.display_ref.clone())
        .field("status", item.task.status)
        .field("priority", item.task.priority)
        .field("labels", &labels)
        .optional("conflicts", item.has_conflict.then(|| "yes".to_string()))
        .optional("deleted", item.task.deleted.then(|| "yes".to_string()))
        .optional("epic", item.task.is_epic.then(|| "yes".to_string()))
        .optional("available_at", item.task.available_at.clone())
        .optional("due_on", item.task.due_on.clone())
        .optional(
            "blocked_by",
            (item.unresolved_blocker_count > 0).then(|| item.unresolved_blocker_count.to_string()),
        )
        .optional(
            "blocks",
            (item.dependent_count > 0).then(|| item.dependent_count.to_string()),
        )
        .quoted("title", &item.task.title)
        .finish();
    println!("{line}");
}

pub(crate) struct TaskFullReport {
    pub(crate) detail: crate::query::TaskDetail,
    pub(crate) conflicts: Vec<TaskConflictReport>,
    pub(crate) attachments: Vec<AttachmentMetadataJson>,
}

pub(crate) fn print_full_task_report(report: &TaskFullReport) {
    let detail = &report.detail;
    print_task_line_item(&detail.item);
    let task = &detail.item.task;
    println!("id={}", task.id);
    println!(
        "project={} prefix={}",
        task.project_key, task.project_prefix
    );
    println!("created={} updated={}", task.created_at, task.updated_at);
    if !task.description.is_empty() {
        println!("description<<EOF");
        print!("{}", task.description);
        if !task.description.ends_with('\n') {
            println!();
        }
        println!("EOF");
    }
    print_attachment_section(&report.attachments);
    print_task_dependency_summary(&detail.dependencies);
    for note in &detail.notes {
        println!("note created={}", note.created_at);
        print_multiline_block("body", &note.body);
    }
    for conflict in &report.conflicts {
        println!(
            "conflict {} field={}",
            detail.item.display_ref, conflict.field
        );
        println!("variant {}", conflict.variant_a);
        print_multiline_block("value", &conflict.local_value);
        println!("variant {}", conflict.variant_b);
        print_multiline_block("value", &conflict.remote_value);
    }
}

pub(crate) fn task_full_json(report: &TaskFullReport) -> TaskFullJson {
    let detail = &report.detail;
    let task = &detail.item.task;
    TaskFullJson {
        task: task_line_json_item(&detail.item),
        project_prefix: task.project_prefix.clone(),
        description: task.description.clone(),
        dependencies: task_dependency_summary_json(&detail.dependencies),
        notes: detail
            .notes
            .iter()
            .map(|note| TaskNoteJson {
                body: note.body.clone(),
                created_at: note.created_at.clone(),
            })
            .collect(),
        conflicts: report.conflicts.clone(),
        attachments: report.attachments.clone(),
    }
}

pub(crate) fn print_task_dependency_summary(summary: &TaskDependencySummary) {
    print_dependency_section("depends_on", &summary.depends_on);
    print_dependency_section("blocks", &summary.blocks);
}

fn print_dependency_section(label: &str, items: &[crate::query::TaskDependencyItem]) {
    let open = items.iter().filter(|item| item.unresolved).count();
    println!("{label} open={open} total={}", items.len());
    for item in items {
        println!(
            "- {} status={} title={}",
            item.display_ref,
            item.task.status,
            quote(&item.task.title)
        );
    }
}

pub(crate) fn attachment_metadata_json(item: AttachmentReadItem) -> AttachmentMetadataJson {
    AttachmentMetadataJson {
        attachment_id: item.attachment.attachment_id,
        task_id: item.attachment.task_id,
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

pub(crate) fn print_attachment_section(attachments: &[AttachmentMetadataJson]) {
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

pub(crate) fn print_attachment_metadata_line(attachment: &AttachmentMetadataJson) {
    let line = KvLine::new("attachment")
        .field("attachment_id", &attachment.attachment_id)
        .field("media_type", &attachment.media_type)
        .field("byte_size", attachment.byte_size)
        .field("deleted", yes_no(attachment.deleted))
        .field("has_blob", yes_no(attachment.has_blob));
    println!("{}", line.finish());
}

#[allow(dead_code)]
pub(crate) fn attachment_placeholder(attachment: &AttachmentMetadataJson) -> String {
    if attachment.deleted {
        "[image: deleted attachment]".to_string()
    } else {
        match attachment.bytes_state {
            AttachmentBytesState::Present => "[image: attachment]".to_string(),
            AttachmentBytesState::PendingDownload => "[image: pending download]".to_string(),
            AttachmentBytesState::Unavailable => "[image: unavailable bytes]".to_string(),
        }
    }
}

#[cfg(test)]
pub(crate) fn attachment_unavailable_placeholder(_attachment: &AttachmentMetadataJson) -> String {
    "[image: unavailable bytes]".to_string()
}

// --- JSON DTOs ---

#[derive(Serialize)]
pub(crate) struct TaskEpicLinkJson {
    pub(crate) r#ref: String,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) open: bool,
}

#[derive(Serialize)]
pub(crate) struct TaskLineJson {
    pub(crate) r#ref: String,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) project: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) labels: Vec<String>,
    pub(crate) deleted: bool,
    pub(crate) is_epic: bool,
    pub(crate) epic_parent: Option<TaskEpicLinkJson>,
    pub(crate) epic_children: Vec<TaskEpicLinkJson>,
    pub(crate) has_conflict: bool,
    pub(crate) blocked_by: i64,
    pub(crate) blocks: i64,
    pub(crate) available_at: String,
    pub(crate) due_on: String,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

pub(crate) fn task_line_json_item(item: &TaskListItem) -> TaskLineJson {
    TaskLineJson {
        r#ref: item.display_ref.clone(),
        id: item.task.id.to_string(),
        title: item.task.title.clone(),
        project: item.task.project_key.clone(),
        status: item.task.status.to_string(),
        priority: item.task.priority.to_string(),
        labels: item.labels.clone(),
        deleted: item.task.deleted,
        is_epic: item.task.is_epic,
        epic_parent: item.epic_parent.as_ref().map(task_epic_link_json),
        epic_children: item.epic_children.iter().map(task_epic_link_json).collect(),
        has_conflict: item.has_conflict,
        blocked_by: item.unresolved_blocker_count,
        blocks: item.dependent_count,
        available_at: item.task.available_at.clone().unwrap_or_default(),
        due_on: item.task.due_on.clone().unwrap_or_default(),
        created_at: item.task.created_at.clone(),
        updated_at: item.task.updated_at.clone(),
    }
}

pub(crate) fn task_epic_link_json(link: &TaskDependencyLink) -> TaskEpicLinkJson {
    TaskEpicLinkJson {
        r#ref: link.display_ref.clone(),
        id: link.task_id.to_string(),
        title: link.title.clone(),
        status: link.status.clone(),
        priority: link.priority.clone(),
        open: link.unresolved,
    }
}

#[derive(Serialize, Debug, Clone)]
pub(crate) struct AttachmentMetadataJson {
    pub(crate) attachment_id: String,
    pub(crate) task_id: String,
    #[serde(skip)]
    pub(crate) sha256: String,
    pub(crate) media_type: String,
    pub(crate) byte_size: i64,
    pub(crate) filename: Option<String>,
    pub(crate) alt_text: Option<String>,
    pub(crate) width: Option<i64>,
    pub(crate) height: Option<i64>,
    pub(crate) created_at: String,
    pub(crate) deleted: bool,
    pub(crate) deleted_at: Option<String>,
    #[serde(skip)]
    pub(crate) bytes_state: AttachmentBytesState,
    pub(crate) has_blob: bool,
}

#[derive(Serialize)]
pub(crate) struct TaskFullJson {
    pub(crate) task: TaskLineJson,
    pub(crate) project_prefix: String,
    pub(crate) description: String,
    pub(crate) dependencies: TaskDependencySummaryJson,
    pub(crate) notes: Vec<TaskNoteJson>,
    pub(crate) conflicts: Vec<TaskConflictReport>,
    pub(crate) attachments: Vec<AttachmentMetadataJson>,
}

#[derive(Serialize)]
pub(crate) struct TaskNoteJson {
    pub(crate) body: String,
    pub(crate) created_at: String,
}

#[derive(Clone, Serialize)]
pub(crate) struct TaskConflictReport {
    pub(crate) field: String,
    pub(crate) variant_a: String,
    pub(crate) local_value: String,
    pub(crate) variant_b: String,
    pub(crate) remote_value: String,
}

#[derive(Serialize)]
pub(crate) struct TaskDependencySummaryJson {
    pub(crate) depends_on_open: i64,
    pub(crate) depends_on_total: i64,
    pub(crate) blocks_open: i64,
    pub(crate) blocks_total: i64,
    pub(crate) depends_on: Vec<TaskDependencyItemJson>,
    pub(crate) blocks: Vec<TaskDependencyItemJson>,
}

#[derive(Serialize)]
pub(crate) struct TaskDependencyItemJson {
    pub(crate) r#ref: String,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) deleted: bool,
    pub(crate) unresolved: bool,
    pub(crate) created_at: String,
}

pub(crate) fn task_dependency_summary_json(
    summary: &TaskDependencySummary,
) -> TaskDependencySummaryJson {
    TaskDependencySummaryJson {
        depends_on_open: summary.depends_on.iter().filter(|d| d.unresolved).count() as i64,
        depends_on_total: summary.depends_on.len() as i64,
        blocks_open: summary.blocks.iter().filter(|d| d.unresolved).count() as i64,
        blocks_total: summary.blocks.len() as i64,
        depends_on: summary
            .depends_on
            .iter()
            .map(|d| TaskDependencyItemJson {
                r#ref: d.display_ref.clone(),
                id: d.task.id.to_string(),
                title: d.task.title.clone(),
                status: d.task.status.to_string(),
                priority: d.task.priority.to_string(),
                deleted: d.task.deleted,
                unresolved: d.unresolved,
                created_at: d.task.created_at.clone(),
            })
            .collect(),
        blocks: summary
            .blocks
            .iter()
            .map(|d| TaskDependencyItemJson {
                r#ref: d.display_ref.clone(),
                id: d.task.id.to_string(),
                title: d.task.title.clone(),
                status: d.task.status.to_string(),
                priority: d.task.priority.to_string(),
                deleted: d.task.deleted,
                unresolved: d.unresolved,
                created_at: d.task.created_at.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_attachment_metadata(
        attachment_id: &str,
        has_blob: bool,
        deleted: bool,
        filename: Option<&str>,
        alt_text: Option<&str>,
    ) -> AttachmentMetadataJson {
        AttachmentMetadataJson {
            attachment_id: attachment_id.to_string(),
            task_id: "TASK000000000000".to_string(),
            sha256: "0".repeat(64),
            media_type: "image/png".to_string(),
            byte_size: 9,
            filename: filename.map(str::to_string),
            alt_text: alt_text.map(str::to_string),
            width: None,
            height: None,
            created_at: "001".to_string(),
            deleted,
            deleted_at: deleted.then(|| "002".to_string()),
            bytes_state: if has_blob {
                AttachmentBytesState::Present
            } else {
                AttachmentBytesState::PendingDownload
            },
            has_blob,
        }
    }

    #[test]
    fn attachment_placeholders_describe_attachment_states() {
        let present = test_attachment_metadata(
            "7KQ9A1X4MV2P8D6R",
            true,
            false,
            Some("diagram.png"),
            Some("diagram"),
        );
        let pending =
            test_attachment_metadata("8KQ9A1X4MV2P8D6R", false, false, Some("photo.png"), None);
        let unavailable =
            test_attachment_metadata("AKQ9A1X4MV2P8D6R", false, false, Some("archive.png"), None);
        let mut unavailable = unavailable;
        unavailable.bytes_state = AttachmentBytesState::Unavailable;
        let deleted =
            test_attachment_metadata("9KQ9A1X4MV2P8D6R", true, true, None, Some("old screenshot"));

        assert_eq!(attachment_placeholder(&present), "[image: attachment]");
        assert_eq!(
            attachment_placeholder(&pending),
            "[image: pending download]"
        );
        assert_eq!(
            attachment_placeholder(&unavailable),
            "[image: unavailable bytes]"
        );
        assert_eq!(
            attachment_placeholder(&deleted),
            "[image: deleted attachment]"
        );
        assert_eq!(
            attachment_unavailable_placeholder(&present),
            "[image: unavailable bytes]"
        );
    }
}
