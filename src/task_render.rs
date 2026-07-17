use serde::Serialize;

use crate::query::{TaskDependencyLink, TaskDependencySummary, TaskListItem};
use crate::render::{KvLine, print_multiline_block, quote};

pub(crate) fn print_task_line_item(item: &TaskListItem) {
    let labels = item.labels.join(",");
    let line = KvLine::new(item.display_ref.clone())
        .field("status", item.task.status)
        .field("priority", item.task.priority)
        .field("labels", &labels)
        .optional("conflicts", item.has_conflict.then(|| "yes".to_string()))
        .optional("deleted", item.task.deleted.then(|| "yes".to_string()))
        .optional("epic", item.task.is_epic.then(|| "yes".to_string()))
        .optional(
            "available_at",
            (!item.task.available_at.is_empty()).then(|| item.task.available_at.clone()),
        )
        .optional(
            "due_on",
            (!item.task.due_on.is_empty()).then(|| item.task.due_on.clone()),
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
    pub(crate) detail: crate::query::TaskDetail,
    pub(crate) conflicts: Vec<TaskConflictReport>,
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
        id: item.task.id.clone(),
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
        available_at: item.task.available_at.clone(),
        due_on: item.task.due_on.clone(),
        created_at: item.task.created_at.clone(),
        updated_at: item.task.updated_at.clone(),
    }
}

pub(crate) fn task_epic_link_json(link: &TaskDependencyLink) -> TaskEpicLinkJson {
    TaskEpicLinkJson {
        r#ref: link.display_ref.clone(),
        id: link.task_id.clone(),
        title: link.title.clone(),
        status: link.status.clone(),
        priority: link.priority.clone(),
        open: link.unresolved,
    }
}

#[derive(Serialize)]
pub(crate) struct TaskFullJson {
    pub(crate) task: TaskLineJson,
    pub(crate) project_prefix: String,
    pub(crate) description: String,
    pub(crate) dependencies: TaskDependencySummaryJson,
    pub(crate) notes: Vec<TaskNoteJson>,
    pub(crate) conflicts: Vec<TaskConflictReport>,
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
                id: d.task.id.clone(),
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
                id: d.task.id.clone(),
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
