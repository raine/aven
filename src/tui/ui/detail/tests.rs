use std::collections::BTreeSet;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Modifier;
use ratatui::text::Line;

use super::super::task_list::EPIC_MARKER;
use super::super::timestamps::{local_activity_timestamp_display, local_timestamp_display};
use super::attachments::attachment_detail_line;
use super::text::detail_title_lines;
use super::*;
use crate::choices::{TaskPriority, TaskStatus};
use crate::tui::theme::{self, ACCENT, BG_PANEL, FG, FG_DIM, FG_MUTED, INVERSE_FG, YELLOW};

mod activity;
mod attachments;
mod document;
mod metadata;
mod relationships;
mod text;

fn attachment_metadata(
    attachment_id: &str,
    deleted: bool,
    has_blob: bool,
) -> crate::task_render::AttachmentMetadataJson {
    crate::task_render::AttachmentMetadataJson {
        attachment_id: attachment_id.to_string(),
        task_id: "7KQ9A1X".to_string(),
        sha256: "0".repeat(64),
        media_type: "image/png".to_string(),
        byte_size: 4,
        filename: Some("chart.png".to_string()),
        alt_text: Some("Chart".to_string()),
        width: Some(640),
        height: Some(480),
        created_at: "2026-06-20T12:00:00Z".to_string(),
        deleted,
        deleted_at: deleted.then(|| "2026-06-20T12:00:00Z".to_string()),
        bytes_state: if has_blob {
            crate::attachments::AttachmentBytesState::Present
        } else {
            crate::attachments::AttachmentBytesState::PendingDownload
        },
        has_blob,
    }
}

fn detail_test_epic_item() -> TaskListItem {
    let mut item = detail_test_item();
    item.task.is_epic = true;
    item.epic_children = vec![
        crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id("child-task-id"),
            display_ref: "APP-CHLD".to_string(),
            title: "Build the first child task".to_string(),
            status: "todo".to_string(),
            priority: "medium".to_string(),
            unresolved: true,
        },
        crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id("done-child-task-id"),
            display_ref: "APP-DONE".to_string(),
            title: "Finished child task".to_string(),
            status: "done".to_string(),
            priority: "none".to_string(),
            unresolved: false,
        },
    ];
    item
}

fn detail_test_item() -> TaskListItem {
    TaskListItem {
        metadata: Vec::new(),
        activity: Vec::new(),
        conflicts: vec![crate::query::TaskConflictValue {
            field: "title".to_string(),
            local_value: "Fix token refresh race".to_string(),
            remote_value: "Fix refresh race".to_string(),
        }],
        task: crate::types::Task {
            id: crate::test_support::task_id("7KQ9A1X"),
            workspace_id: "0000000000000001".parse().unwrap(),
            title: "Fix token refresh race".to_string(),
            description: "Two token refresh requests fire together.".to_string(),
            project_id: "0000000000000001".parse().unwrap(),
            project_key: "app".to_string(),
            project_prefix: "APP".to_string(),
            status: TaskStatus::Active,
            priority: TaskPriority::Urgent,
            source: crate::choices::TaskSource::Unknown,
            created_at: "2026-06-19T12:00:00Z".to_string(),
            updated_at: "2026-06-20T12:00:00Z".to_string(),
            queue_activity_at: "2026-06-20T12:00:00Z".to_string(),
            available_at: None,
            due_on: None,
            deleted: false,
            is_epic: false,
        },
        display_ref: "APP-7KQ9A1X".to_string(),
        labels: vec!["bug".to_string(), "mobile".to_string()],
        notes: vec![crate::query::TaskNote {
            id: "note-id".to_string(),
            body: "Confirmed race in useTokenRefresh.ts".to_string(),
            created_at: "2026-06-20T12:00:00Z".to_string(),
        }],
        has_notes: true,
        has_conflict: true,
        unresolved_blocker_count: 0,
        dependent_count: 0,
        depends_on: vec![crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id("blocker-task-id"),
            display_ref: "APP-7KQ1".to_string(),
            title: "Ship auth service".to_string(),
            status: "todo".to_string(),
            priority: "high".to_string(),
            unresolved: true,
        }],
        blocks: vec![crate::query::TaskDependencyLink {
            project_key: "app".to_string(),
            task_id: crate::test_support::task_id("dependent-task-id"),
            display_ref: "APP-7KQ2".to_string(),
            title: "Write rollout notes".to_string(),
            status: "inbox".to_string(),
            priority: "none".to_string(),
            unresolved: true,
        }],
        related: Vec::new(),
        epic_children: Vec::new(),
        epic_child_dependencies: Default::default(),
        epic_parent: None,
        epic_rollup: None,
        recurrence: None,
        recurrence_group: None,
        hydration: crate::query::TaskItemHydration::Detail,
        attachments: Vec::new(),
        live_attachment_count: 0,
        queue: Default::default(),
    }
}
