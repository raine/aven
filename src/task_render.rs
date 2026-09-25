mod attachments;
mod json;
mod markdown;
mod text;

#[cfg(test)]
pub(crate) use attachments::attachment_placeholder;
#[cfg(test)]
pub(crate) use attachments::attachment_unavailable_placeholder;
pub(crate) use attachments::{
    AttachmentMetadataJson, attachment_metadata_json, attachment_state_placeholder,
    human_file_size, print_attachment_section,
};
pub(crate) use json::{
    TaskConflictReport, TaskEpicLinkJson, TaskLineJson, TaskRecurrenceJson, TaskRelatedJson,
    task_dependency_summary_json, task_epic_link_json, task_full_json, task_line_json_item,
    task_recurrence_json, task_related_json,
};
pub(crate) use markdown::{gist_description, gist_filename, task_markdown};
pub(crate) use text::{
    print_full_task_report, print_task_dependency_summary, print_task_line_item, task_line_text,
};

use anyhow::Result;
use aven_core::db::Database;

use crate::workspaces::Workspace;

#[cfg(test)]
use crate::attachments::AttachmentBytesState;
use attachments::attachment_availability;
#[cfg(test)]
use markdown::{markdown_authored_body, markdown_code, prepare_authored_markdown};

pub(crate) struct TaskFullReport {
    pub(crate) workspace_key: String,
    pub(crate) workspace_name: String,
    pub(crate) detail: crate::query::TaskDetail,
    pub(crate) conflicts: Vec<TaskConflictReport>,
    pub(crate) attachments: Vec<AttachmentMetadataJson>,
}

pub(crate) async fn build_full_task_report(
    database: &Database,
    workspace: &Workspace,
    detail: crate::query::TaskDetail,
) -> Result<TaskFullReport> {
    let task = &detail.item.task;
    let mut conflicts = Vec::with_capacity(detail.conflicts.len());
    for conflict in &detail.conflicts {
        let local_value = database
            .conflict_display_value(&task.workspace_id, &conflict.field, &conflict.local_value)
            .await?;
        let remote_value = database
            .conflict_display_value(&task.workspace_id, &conflict.field, &conflict.remote_value)
            .await?;
        conflicts.push(TaskConflictReport {
            field: conflict.field.clone(),
            variant_a: conflict.variant_a.clone(),
            local_value,
            variant_b: conflict.variant_b.clone(),
            remote_value,
        });
    }
    let attachments = database
        .attachment_read_items_by_task(&task.workspace_id, &task.id, true)
        .await?
        .into_iter()
        .map(attachment_metadata_json)
        .collect();
    Ok(TaskFullReport {
        workspace_key: workspace.key.clone(),
        workspace_name: workspace.name.clone(),
        detail,
        conflicts,
        attachments,
    })
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
    fn authored_markdown_closes_unfinished_fences() {
        let mut output = String::new();
        markdown_authored_body(&mut output, "before\n````rust\nlet value = 1;", 3);
        assert_eq!(output, "before\n````rust\nlet value = 1;\n````\n");

        let mut output = String::new();
        markdown_authored_body(&mut output, "~~~text\nbody\n~~~\n", 3);
        assert_eq!(output, "~~~text\nbody\n~~~\n");
    }

    #[test]
    fn authored_markdown_stays_within_generated_heading_hierarchy() {
        let body = "# Outcome\n\nScope\n-----\n\n```md\n# example\n```\n\n<!-- hidden -->\n<section>unsafe</section>";

        assert_eq!(
            prepare_authored_markdown(body, 3),
            "### Outcome\n\n### Scope\n\n```md\n# example\n```\n\n\\<!-- hidden --\\>\n\\<section>unsafe</section>\n"
        );
    }

    #[test]
    fn markdown_code_uses_safe_delimiters() {
        assert_eq!(markdown_code("AVN-1234"), "`AVN-1234`");
        assert_eq!(markdown_code("use `code`"), "`` use `code` ``");
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
        let unnamed =
            test_attachment_metadata("BKQ9A1X4MV2P8D6R", true, false, None, Some("unnamed image"));
        let deleted =
            test_attachment_metadata("9KQ9A1X4MV2P8D6R", true, true, None, Some("old screenshot"));

        assert_eq!(
            attachment_placeholder(&present),
            "[image: attachment] diagram.png · 9 B"
        );
        assert_eq!(
            attachment_placeholder(&unnamed),
            "[image: attachment] · 9 B"
        );
        assert_eq!(
            attachment_placeholder(&pending),
            "[image: pending download] photo.png · 9 B"
        );
        assert_eq!(
            attachment_placeholder(&unavailable),
            "[image: unavailable bytes] archive.png · 9 B"
        );
        assert_eq!(
            attachment_placeholder(&deleted),
            "[image: deleted attachment] · 9 B"
        );
        assert_eq!(
            attachment_unavailable_placeholder(&present),
            "[image: unavailable bytes]"
        );
    }

    #[test]
    fn human_file_size_uses_binary_units() {
        assert_eq!(human_file_size(999), "999 B");
        assert_eq!(human_file_size(1_536), "1.5 KiB");
        assert_eq!(human_file_size(2_621_440), "2.5 MiB");
    }
}
