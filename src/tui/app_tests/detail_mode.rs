use super::*;

fn detail_link(item: &crate::query::TaskListItem) -> crate::query::TaskDependencyLink {
    crate::query::TaskDependencyLink {
        task_id: item.task.id.clone(),
        display_ref: item.display_ref.clone(),
        title: item.task.title.clone(),
        status: item.task.status.as_str().to_string(),
        priority: item.task.priority.as_str().to_string(),
        unresolved: true,
    }
}

fn detail_navigation_state(app: &App, task_id: crate::ids::TaskId, scroll: u16) -> DetailSnapshot {
    DetailSnapshot {
        task_id,
        scroll,
        focused_target: None,
        expanded_sections: std::collections::BTreeSet::new(),
        view_state: app.store.view_state.clone(),
    }
}

#[path = "detail_mode/interaction.rs"]
mod interaction;

#[path = "detail_mode/navigation.rs"]
mod navigation;

#[path = "detail_mode/relationships.rs"]
mod relationships;

#[path = "detail_mode/attachments.rs"]
mod attachments;

#[path = "detail_mode/history_navigation.rs"]
mod history_navigation;

#[path = "detail_mode/editing.rs"]
mod editing;
