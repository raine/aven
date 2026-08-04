use anyhow::Result;
use aven_core::db::Database;
use aven_core::operations::AttachmentReadItem;
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::attachments::AttachmentBytesState;
use crate::query::{TaskDependencyLink, TaskDependencySummary, TaskListItem};
use crate::render::{KvLine, print_multiline_block, quote, yes_no};
use crate::workspaces::Workspace;

pub(crate) fn print_task_line_item(item: &TaskListItem) {
    let labels = item.labels.join(",");
    if let Some(group) = &item.recurrence_group {
        let counts = &group.counts;
        let line = KvLine::new(group.series_ref.clone())
            .field("status", item.task.status)
            .field("priority", item.task.priority)
            .field("labels", &labels)
            .optional(
                "latest",
                counts
                    .latest_outcome
                    .map(|value| value.as_str().to_string()),
            )
            .optional("slot", counts.latest_slot_on.clone())
            .field("completed", counts.completed)
            .field("skipped", counts.skipped)
            .field("missed", counts.missed)
            .quoted("title", &item.task.title)
            .finish();
        println!("{line}");
        return;
    }
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
            "series",
            item.recurrence
                .as_ref()
                .map(|value| value.series_ref.clone()),
        )
        .optional(
            "slot",
            item.recurrence.as_ref().map(|value| value.slot_on.clone()),
        )
        .optional(
            "repeat",
            item.recurrence
                .as_ref()
                .map(|value| value.rule_label.clone()),
        )
        .optional(
            "series_state",
            item.recurrence
                .as_ref()
                .map(|value| value.lifecycle.as_str().to_string()),
        )
        .optional(
            "outcome",
            item.recurrence
                .as_ref()
                .and_then(|value| value.outcome)
                .map(|value| value.as_str().to_string()),
        )
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

pub(crate) fn task_markdown(report: &TaskFullReport) -> String {
    let detail = &report.detail;
    let item = &detail.item;
    let task = &item.task;
    let mut output = String::new();

    output.push_str("# ");
    if report
        .conflicts
        .iter()
        .any(|conflict| conflict.field == "title")
    {
        output.push_str("Task ");
        output.push_str(&markdown_heading(&item.display_ref));
    } else {
        output.push_str(&markdown_heading(&single_line(&task.title)));
    }
    output.push_str("\n\n");

    if task.deleted || !report.conflicts.is_empty() {
        let warning = match (task.deleted, report.conflicts.len()) {
            (true, 0) => "This task is deleted.".to_string(),
            (false, count) => format!(
                "This task has {} unresolved sync {}. No conflict variant is authoritative.",
                count,
                if count == 1 { "conflict" } else { "conflicts" }
            ),
            (true, count) => format!(
                "This task is deleted and has {} unresolved sync {}. No conflict variant is authoritative.",
                count,
                if count == 1 { "conflict" } else { "conflicts" }
            ),
        };
        output.push_str("> **Warning:** ");
        output.push_str(&warning);
        output.push_str("\n\n");
    }

    output.push_str(&markdown_code(&item.display_ref));
    output.push_str(" · **");
    output.push_str(status_label(task.status.as_str()));
    output.push_str("** · Project **");
    output.push_str(&markdown_text(&project_label(
        &detail.project_name,
        &task.project_key,
    )));
    output.push_str("**");
    if task.priority.as_str() != "none" {
        output.push_str(" · **");
        output.push_str(priority_label(task.priority.as_str()));
        output.push_str(" priority**");
    }
    if task.is_epic {
        output.push_str(" · **Epic**");
    }
    output.push('\n');

    if !item.labels.is_empty() {
        output.push_str("\n**Labels:** ");
        for (index, label) in item.labels.iter().enumerate() {
            if index > 0 {
                output.push_str(", ");
            }
            output.push_str(&markdown_code(label));
        }
        output.push('\n');
    }

    if task.available_at.is_some() || task.due_on.is_some() {
        output.push('\n');
        if let Some(available_at) = &task.available_at {
            output.push_str("**Available:** ");
            output.push_str(&human_timestamp(available_at));
        }
        if task.available_at.is_some() && task.due_on.is_some() {
            output.push_str(" · ");
        }
        if let Some(due_on) = &task.due_on {
            output.push_str("**Due:** ");
            output.push_str(&markdown_text(due_on));
        }
        output.push('\n');
    }

    if !report.conflicts.is_empty() {
        output.push_str("\n## Unresolved sync conflicts\n\n");
        output.push_str(
            "Resolve these values in Aven before treating this snapshot as authoritative.\n",
        );
        for conflict in &report.conflicts {
            output.push_str("\n### ");
            output.push_str(&markdown_heading(&field_label(&conflict.field)));
            output.push_str("\n\n");
            markdown_conflict_variant(
                &mut output,
                "Local",
                &conflict.variant_a,
                &conflict.local_value,
            );
            output.push('\n');
            markdown_conflict_variant(
                &mut output,
                "Remote",
                &conflict.variant_b,
                &conflict.remote_value,
            );
        }
    }

    if !task.description.is_empty() {
        markdown_authored_section(&mut output, "Description", &task.description, 3);
    }

    if !detail.notes.is_empty() {
        output.push_str("\n## Notes\n");
        for note in &detail.notes {
            output.push_str("\n### ");
            output.push_str(&human_timestamp(&note.created_at));
            output.push_str("\n\n");
            if note.body.is_empty() {
                output.push_str("_Empty note._\n");
            } else {
                markdown_authored_body(&mut output, &note.body, 4);
            }
        }
    }

    if item.epic_parent.is_some()
        || !item.epic_children.is_empty()
        || !detail.dependencies.depends_on.is_empty()
        || !detail.dependencies.blocks.is_empty()
    {
        output.push_str("\n## Relationships\n");
        if let Some(parent) = &item.epic_parent {
            markdown_link_section(&mut output, "Parent epic", std::slice::from_ref(parent));
        }
        markdown_link_section(&mut output, "Child tasks", &item.epic_children);
        markdown_dependency_section(&mut output, "Blocked by", &detail.dependencies.depends_on);
        markdown_dependency_section(&mut output, "Blocks", &detail.dependencies.blocks);
    }

    if let Some(recurrence) = &item.recurrence {
        output.push_str("\n## Recurrence\n\n");
        output.push_str("**Repeats:** ");
        output.push_str(&markdown_text(&recurrence.rule_label));
        output.push_str(" · **Time zone:** ");
        output.push_str(&markdown_code(&recurrence.timezone));
        output.push_str(" · **Slot:** ");
        output.push_str(&markdown_text(&recurrence.slot_on));
        output.push_str(" · **Series:** ");
        output.push_str(&markdown_code(&recurrence.series_ref));
        output.push_str(" (");
        output.push_str(recurrence.lifecycle.as_str());
        output.push_str(", ");
        output.push_str(
            recurrence
                .outcome
                .map_or("pending", |outcome| outcome.as_str()),
        );
        output.push_str(")\n");
    }

    let live_attachments = report
        .attachments
        .iter()
        .filter(|attachment| !attachment.deleted)
        .collect::<Vec<_>>();
    if !live_attachments.is_empty() {
        output.push_str("\n## Attachments\n\n");
        output.push_str("Files are not included in this document.\n\n");
        output.push_str("| File | Type | Size | Dimensions | Alt text | Availability |\n");
        output.push_str("| --- | --- | ---: | --- | --- | --- |\n");
        for attachment in live_attachments {
            let name = attachment
                .filename
                .as_deref()
                .unwrap_or("Unnamed attachment");
            output.push_str("| ");
            output.push_str(&markdown_table_text(name));
            output.push_str(" | ");
            output.push_str(&markdown_table_text(&attachment.media_type));
            output.push_str(" | ");
            output.push_str(&human_file_size(attachment.byte_size));
            output.push_str(" | ");
            if let (Some(width), Some(height)) = (attachment.width, attachment.height) {
                output.push_str(&format!("{width}×{height}"));
            } else {
                output.push_str("n/a");
            }
            output.push_str(" | ");
            output.push_str(&markdown_table_text(
                attachment.alt_text.as_deref().unwrap_or("n/a"),
            ));
            output.push_str(" | ");
            output.push_str(attachment_availability(attachment));
            output.push_str(" |\n");
        }
    }

    output.push_str("\n---\n\nAven · task ");
    output.push_str(&markdown_code(task.id.as_str()));
    output.push_str(" · workspace ");
    output.push_str(&markdown_code(&workspace_label(report)));
    output.push_str(" · created ");
    output.push_str(&human_timestamp(&task.created_at));
    if task.updated_at != task.created_at {
        output.push_str(" · updated ");
        output.push_str(&human_timestamp(&task.updated_at));
    }
    output.push_str(
        "\n\n<sub>Created with <a href=\"https://github.com/raine/aven\">aven</a></sub>\n",
    );
    output
}

fn markdown_authored_section(
    output: &mut String,
    heading: &str,
    body: &str,
    minimum_heading: usize,
) {
    output.push_str("\n## ");
    output.push_str(heading);
    output.push_str("\n\n");
    markdown_authored_body(output, body, minimum_heading);
}

fn markdown_authored_body(output: &mut String, body: &str, minimum_heading: usize) {
    let body = prepare_authored_markdown(body, minimum_heading);
    output.push_str(body.trim_end_matches(['\r', '\n']));
    if let Some((marker, length)) = unclosed_markdown_fence(&body) {
        output.push('\n');
        output.extend(std::iter::repeat_n(marker, length));
    }
    output.push('\n');
}

fn prepare_authored_markdown(body: &str, minimum_heading: usize) -> String {
    let normalized = body.replace("\r\n", "\n").replace('\r', "\n");
    let lines = normalized.lines().collect::<Vec<_>>();
    let mut output = String::new();
    let mut active_fence = None;
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if let Some((marker, length)) = markdown_fence(line) {
            match active_fence {
                None => active_fence = Some((marker, length)),
                Some((opening_marker, opening_length))
                    if marker == opening_marker
                        && length >= opening_length
                        && fence_suffix(line, length).trim().is_empty() =>
                {
                    active_fence = None;
                }
                Some(_) => {}
            }
            output.push_str(line);
            output.push('\n');
            index += 1;
            continue;
        }

        if active_fence.is_some() {
            output.push_str(line);
            output.push('\n');
            index += 1;
            continue;
        }

        if index + 1 < lines.len()
            && !line.trim().is_empty()
            && let Some(level) = setext_heading_level(lines[index + 1])
        {
            output.extend(std::iter::repeat_n('#', level.max(minimum_heading)));
            output.push(' ');
            output.push_str(line.trim());
            output.push('\n');
            index += 2;
            continue;
        }

        if let Some((indent, level, text)) = atx_heading(line) {
            output.push_str(indent);
            output.extend(std::iter::repeat_n('#', level.max(minimum_heading)));
            output.push(' ');
            output.push_str(text);
            output.push('\n');
            index += 1;
            continue;
        }

        let mut safe = line.replace("<!--", "\\<!--").replace("-->", "--\\>");
        if safe.trim_start().starts_with('<') {
            let indent = safe.len() - safe.trim_start().len();
            safe.insert(indent, '\\');
        }
        output.push_str(&safe);
        output.push('\n');
        index += 1;
    }

    output
}

fn atx_heading(line: &str) -> Option<(&str, usize, &str)> {
    let trimmed = line.trim_start_matches(' ');
    let indent_length = line.len() - trimmed.len();
    if indent_length > 3 {
        return None;
    }
    let level = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let text = trimmed.get(level..)?;
    if !text.is_empty() && !text.starts_with(char::is_whitespace) {
        return None;
    }
    Some((&line[..indent_length], level, text.trim_start()))
}

fn setext_heading_level(line: &str) -> Option<usize> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.chars().all(|ch| ch == '=') {
        Some(1)
    } else if trimmed.chars().all(|ch| ch == '-') {
        Some(2)
    } else {
        None
    }
}

fn markdown_fence(line: &str) -> Option<(char, usize)> {
    let trimmed = line.trim_start_matches(' ');
    if line.len() - trimmed.len() > 3 {
        return None;
    }
    let marker @ ('`' | '~') = trimmed.chars().next()? else {
        return None;
    };
    let length = trimmed.chars().take_while(|ch| *ch == marker).count();
    (length >= 3).then_some((marker, length))
}

fn fence_suffix(line: &str, length: usize) -> &str {
    let trimmed = line.trim_start_matches(' ');
    &trimmed[length..]
}

fn unclosed_markdown_fence(body: &str) -> Option<(char, usize)> {
    let mut active = None;
    for line in body.lines() {
        let Some((marker, length)) = markdown_fence(line) else {
            continue;
        };
        match active {
            None => active = Some((marker, length)),
            Some((opening_marker, opening_length))
                if marker == opening_marker
                    && length >= opening_length
                    && fence_suffix(line, length).trim().is_empty() =>
            {
                active = None;
            }
            Some(_) => {}
        }
    }
    active
}

fn markdown_link_section(output: &mut String, heading: &str, links: &[TaskDependencyLink]) {
    if links.is_empty() {
        return;
    }
    output.push_str("\n### ");
    output.push_str(heading);
    output.push_str("\n\n");
    for link in links {
        output.push_str("- ");
        output.push_str(&markdown_code(&link.display_ref));
        output.push(' ');
        output.push_str(&markdown_text(&single_line(&link.title)));
        output.push_str(" · ");
        output.push_str(status_label(&link.status));
        if link.priority != "none" {
            output.push_str(" · ");
            output.push_str(priority_label(&link.priority));
            output.push_str(" priority");
        }
        output.push('\n');
    }
}

fn markdown_dependency_section(
    output: &mut String,
    heading: &str,
    dependencies: &[crate::query::TaskDependencyItem],
) {
    if dependencies.is_empty() {
        return;
    }
    output.push_str("\n### ");
    output.push_str(heading);
    output.push_str("\n\n");
    for dependency in dependencies {
        output.push_str("- ");
        output.push_str(&markdown_code(&dependency.display_ref));
        output.push(' ');
        output.push_str(&markdown_text(&single_line(&dependency.task.title)));
        output.push_str(" · ");
        output.push_str(status_label(dependency.task.status.as_str()));
        if dependency.task.priority.as_str() != "none" {
            output.push_str(" · ");
            output.push_str(priority_label(dependency.task.priority.as_str()));
            output.push_str(" priority");
        }
        output.push_str(if dependency.task.deleted {
            " · Deleted"
        } else if dependency.unresolved {
            " · Open"
        } else {
            " · Resolved"
        });
        output.push('\n');
    }
}

fn markdown_conflict_variant(output: &mut String, label: &str, variant: &str, value: &str) {
    if !value.contains('\n') && value.chars().count() <= 80 {
        output.push_str("- **");
        output.push_str(label);
        output.push_str("** (");
        output.push_str(&markdown_code(variant));
        output.push_str("): ");
        output.push_str(&markdown_code(value));
        output.push('\n');
        return;
    }
    output.push_str("**");
    output.push_str(label);
    output.push_str("** (");
    output.push_str(&markdown_code(variant));
    output.push_str(")\n\n");
    markdown_literal_block(output, value);
}

fn markdown_table_text(value: &str) -> String {
    markdown_text(&single_line(value)).replace('|', "\\|")
}

fn markdown_heading(value: &str) -> String {
    markdown_text(value).replace('`', "\\`")
}

fn markdown_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('<', "\\<")
        .replace('>', "\\>")
        .replace('#', "\\#")
}

fn markdown_code(value: &str) -> String {
    let longest_run = value
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat((longest_run + 1).max(1));
    if longest_run == 0 {
        format!("{fence}{value}{fence}")
    } else {
        format!("{fence} {value} {fence}")
    }
}

fn single_line(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn status_label(value: &str) -> &str {
    match value {
        "inbox" => "Inbox",
        "backlog" => "Backlog",
        "todo" => "Todo",
        "active" => "Active",
        "done" => "Done",
        "canceled" => "Canceled",
        other => other,
    }
}

fn priority_label(value: &str) -> &str {
    match value {
        "low" => "Low",
        "medium" => "Medium",
        "high" => "High",
        "urgent" => "Urgent",
        "none" => "None",
        other => other,
    }
}

fn project_label(name: &str, key: &str) -> String {
    if name == key {
        name.to_string()
    } else {
        format!("{name} ({key})")
    }
}

fn workspace_label(report: &TaskFullReport) -> String {
    if report.workspace_name == report.workspace_key {
        report.workspace_key.clone()
    } else {
        format!("{} ({})", report.workspace_name, report.workspace_key)
    }
}

fn human_timestamp(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| {
            timestamp
                .with_timezone(&Utc)
                .format("%Y-%m-%d %H:%M UTC")
                .to_string()
        })
        .unwrap_or_else(|_| markdown_text(value))
}

fn field_label(value: &str) -> String {
    value
        .split('_')
        .filter(|part| !part.is_empty())
        .enumerate()
        .map(|(index, part)| {
            if index == 0 {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
                    .unwrap_or_default()
            } else {
                part.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn attachment_availability(attachment: &AttachmentMetadataJson) -> &'static str {
    match attachment.bytes_state {
        AttachmentBytesState::Present => "Present in Aven",
        AttachmentBytesState::PendingDownload => "Pending download",
        AttachmentBytesState::Unavailable => "Unavailable",
    }
}

fn markdown_literal_block(output: &mut String, value: &str) {
    let longest_run = value
        .split(|ch| ch != '`')
        .map(str::len)
        .max()
        .unwrap_or_default();
    let fence = "`".repeat((longest_run + 1).max(3));
    output.push_str(&fence);
    output.push_str("text\n");
    output.push_str(value);
    if !value.ends_with('\n') {
        output.push('\n');
    }
    output.push_str(&fence);
    output.push('\n');
}

pub(crate) fn gist_filename(report: &TaskFullReport) -> String {
    format!("{}.md", report.detail.item.display_ref)
}

pub(crate) fn gist_description(report: &TaskFullReport) -> String {
    format!(
        "{} {}",
        report.detail.item.display_ref,
        single_line(&report.detail.item.task.title)
    )
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
        println!("note id={} created={}", note.id, note.created_at);
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
                id: note.id.clone(),
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

pub(crate) fn attachment_state_placeholder(attachment: &AttachmentMetadataJson) -> &'static str {
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

pub(crate) fn human_file_size(byte_size: i64) -> String {
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
pub(crate) struct TaskRecurrenceJson {
    pub(crate) series_ref: String,
    pub(crate) series_id: String,
    pub(crate) slot_on: String,
    pub(crate) rule: String,
    pub(crate) timezone: String,
    pub(crate) lifecycle: String,
    pub(crate) outcome: Option<String>,
    pub(crate) projection_state: String,
}

#[derive(Serialize)]
pub(crate) struct TaskRecurrenceGroupJson {
    pub(crate) series_ref: String,
    pub(crate) series_id: String,
    pub(crate) completed: usize,
    pub(crate) skipped: usize,
    pub(crate) missed: usize,
    pub(crate) pause_intervals: usize,
    pub(crate) latest_slot_on: Option<String>,
    pub(crate) latest_outcome: Option<String>,
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
    pub(crate) recurrence: Option<TaskRecurrenceJson>,
    pub(crate) recurrence_group: Option<TaskRecurrenceGroupJson>,
    pub(crate) created_at: String,
    pub(crate) updated_at: String,
}

pub(crate) fn task_line_json_item(item: &TaskListItem) -> TaskLineJson {
    TaskLineJson {
        r#ref: item
            .recurrence_group
            .as_ref()
            .map(|group| group.series_ref.clone())
            .unwrap_or_else(|| item.display_ref.clone()),
        id: item
            .recurrence_group
            .as_ref()
            .map(|group| group.series_id.to_string())
            .unwrap_or_else(|| item.task.id.to_string()),
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
        recurrence: item.recurrence.as_ref().map(task_recurrence_json),
        recurrence_group: item
            .recurrence_group
            .as_ref()
            .map(task_recurrence_group_json),
        created_at: item.task.created_at.clone(),
        updated_at: item.task.updated_at.clone(),
    }
}

pub(crate) fn task_recurrence_json(
    value: &crate::query::TaskRecurrenceSummary,
) -> TaskRecurrenceJson {
    TaskRecurrenceJson {
        series_ref: value.series_ref.clone(),
        series_id: value.series_id.to_string(),
        slot_on: value.slot_on.clone(),
        rule: value.rule_label.clone(),
        timezone: value.timezone.clone(),
        lifecycle: value.lifecycle.as_str().to_string(),
        outcome: value.outcome.map(|outcome| outcome.as_str().to_string()),
        projection_state: value.projection_state.as_str().to_string(),
    }
}

pub(crate) fn task_recurrence_group_json(
    value: &crate::query::RecurrenceTaskGroup,
) -> TaskRecurrenceGroupJson {
    TaskRecurrenceGroupJson {
        series_ref: value.series_ref.clone(),
        series_id: value.series_id.to_string(),
        completed: value.counts.completed,
        skipped: value.counts.skipped,
        missed: value.counts.missed,
        pause_intervals: value.counts.pause_intervals,
        latest_slot_on: value.counts.latest_slot_on.clone(),
        latest_outcome: value
            .counts
            .latest_outcome
            .map(|outcome| outcome.as_str().to_string()),
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

pub(crate) fn attachment_metadata_json(item: AttachmentReadItem) -> AttachmentMetadataJson {
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

pub(crate) type AttachmentMetadataJson = crate::query::AttachmentMetadata;

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
    pub(crate) id: String,
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
