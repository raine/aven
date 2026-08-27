use crate::choices::{TaskPriority, TaskStatus};
use crate::query::TaskListItem;
use crate::queue::QueueBand;

pub(crate) fn task_list_item(title: &str) -> TaskListItem {
    TaskListItem {
        metadata: Vec::new(),
        activity: Vec::new(),
        task: crate::types::Task {
            id: crate::test_support::task_id("task-1"),
            workspace_id: "0000000000000001".parse().unwrap(),
            title: title.to_string(),
            description: String::new(),
            project_id: "0000000000000001".parse().unwrap(),
            project_key: "app".to_string(),
            project_prefix: "APP".to_string(),
            status: TaskStatus::Todo,
            priority: TaskPriority::None,
            source: crate::choices::TaskSource::Unknown,
            created_at: "2026-06-20T00:00:00Z".to_string(),
            updated_at: "2026-06-20T00:00:00Z".to_string(),
            queue_activity_at: "2026-06-20T00:00:00Z".to_string(),
            available_at: None,
            due_on: None,
            deleted: false,
            is_epic: false,
        },
        display_ref: "APP-1".to_string(),
        labels: Vec::new(),
        notes: Vec::new(),
        attachments: Vec::new(),
        has_conflict: false,
        unresolved_blocker_count: 0,
        dependent_count: 0,
        depends_on: Vec::new(),
        blocks: Vec::new(),
        related: Vec::new(),
        epic_children: Vec::new(),
        epic_child_dependencies: Default::default(),
        epic_parent: None,
        epic_rollup: None,
        recurrence: None,
        recurrence_group: None,
        queue: Default::default(),
    }
}

pub(crate) fn task_list_item_with_status_and_queue(
    title: &str,
    status: &str,
    band: QueueBand,
) -> TaskListItem {
    let mut item = task_list_item(title);
    item.task.status = TaskStatus::parse(status).expect("valid status");
    item.queue.band = band;
    item
}

pub(crate) fn task_list_item_with_id(title: &str, id: &str) -> TaskListItem {
    let mut item = task_list_item(title);
    item.task.id = crate::test_support::task_id(id);
    item
}

pub(crate) fn task_list_item_with_id_and_status_and_queue(
    title: &str,
    id: &str,
    status: &str,
    band: QueueBand,
) -> TaskListItem {
    let mut item = task_list_item_with_status_and_queue(title, status, band);
    item.task.id = crate::test_support::task_id(id);
    item
}
