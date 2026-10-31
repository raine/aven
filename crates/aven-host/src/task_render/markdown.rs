use chrono::{DateTime, Utc};

use crate::query::TaskDependencyLink;

use super::TaskFullReport;

pub fn task_markdown(report: &TaskFullReport) -> String {
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

    if !item.metadata.is_empty() {
        output.push_str("\n## Metadata\n");
        for metadata in &item.metadata {
            output.push_str("\n### ");
            output.push_str(&markdown_heading(&metadata.key));
            output.push_str("\n\n");
            markdown_literal_block(&mut output, &metadata.value);
        }
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
        || !detail.related.is_empty()
    {
        output.push_str("\n## Relationships\n");
        if let Some(parent) = &item.epic_parent {
            markdown_link_section(&mut output, "Parent epic", std::slice::from_ref(parent));
        }
        markdown_link_section(&mut output, "Child tasks", &item.epic_children);
        markdown_dependency_section(&mut output, "Blocked by", &detail.dependencies.depends_on);
        markdown_dependency_section(&mut output, "Blocks", &detail.dependencies.blocks);
        if !detail.related.is_empty() {
            output.push_str("\n### Related\n\n");
            for related in &detail.related {
                output.push_str("- ");
                output.push_str(&markdown_code(&related.display_ref));
                output.push(' ');
                output.push_str(&markdown_text(&single_line(&related.title)));
                if related.deleted {
                    output.push_str(" · deleted");
                }
                output.push('\n');
            }
        }
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
            output.push_str(&super::human_file_size(attachment.byte_size));
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
            output.push_str(super::attachment_availability(attachment));
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

pub(super) fn markdown_authored_body(output: &mut String, body: &str, minimum_heading: usize) {
    let body = prepare_authored_markdown(body, minimum_heading);
    output.push_str(body.trim_end_matches(['\r', '\n']));
    if let Some((marker, length)) = unclosed_markdown_fence(&body) {
        output.push('\n');
        output.extend(std::iter::repeat_n(marker, length));
    }
    output.push('\n');
}

pub(super) fn prepare_authored_markdown(body: &str, minimum_heading: usize) -> String {
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

pub(super) fn markdown_code(value: &str) -> String {
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

pub fn gist_filename(report: &TaskFullReport) -> String {
    format!("{}.md", report.detail.item.display_ref)
}

pub fn gist_description(report: &TaskFullReport) -> String {
    format!(
        "{} {}",
        report.detail.item.display_ref,
        single_line(&report.detail.item.task.title)
    )
}
