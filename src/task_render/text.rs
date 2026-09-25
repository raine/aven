use crate::query::{TaskDependencySummary, TaskListItem};
use crate::render::{KvLine, print_multiline_block, quote, yes_no};

use super::TaskFullReport;

pub(crate) fn task_line_text(item: &TaskListItem) -> String {
    let labels = item.labels.join(",");
    if let Some(group) = &item.recurrence_group {
        let counts = &group.counts;
        return KvLine::new(group.series_ref.clone())
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
    }
    KvLine::new(item.display_ref.clone())
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
        .finish()
}

pub(crate) fn print_task_line_item(item: &TaskListItem) {
    println!("{}", task_line_text(item));
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
    for metadata in &detail.item.metadata {
        println!(
            "metadata field_id={} key={}",
            metadata.field_id, metadata.key
        );
        print_multiline_block("value", &metadata.value);
    }
    super::print_attachment_section(&report.attachments);
    print_task_dependency_summary(&detail.dependencies);
    println!("Related total={}", detail.related.len());
    for related in &detail.related {
        println!(
            "- {} status={} priority={} deleted={} title={}",
            related.display_ref,
            related.status,
            related.priority,
            yes_no(related.deleted),
            quote(&related.title)
        );
    }
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
