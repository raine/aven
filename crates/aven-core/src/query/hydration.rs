use crate::ids::WorkspaceId;
use anyhow::Result;
use chrono::NaiveDate;
use sqlx::SqliteConnection;

use crate::queue::queue_meta_on;
use crate::refs::DisplayRefContext;
use crate::task_enrichment::{
    load_task_bulk_update_enrichment, load_task_enrichment, load_task_enrichment_without_activity,
    load_task_list_enrichment,
};
use crate::types::Task;

use super::{TaskItemHydration, TaskListItem};

#[derive(Clone, Copy)]
pub(super) enum TaskHydration {
    List,
    BulkUpdate,
    Detail,
    DetailWithoutActivity,
}

/// Build a `Vec<TaskListItem>` from tasks by loading enrichment and display refs.
///
/// Preserves input task order. Callers are responsible for any post-processing
/// such as sorting, filtering, or truncation.
pub async fn build_task_list_items(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    tasks: Vec<Task>,
    now_seconds: i64,
    local_today: NaiveDate,
    display_refs: &DisplayRefContext,
    hydration: TaskHydration,
) -> Result<Vec<TaskListItem>> {
    let task_ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
    let mut enrichment = match hydration {
        TaskHydration::List => {
            load_task_list_enrichment(conn, workspace_id, &task_ids, display_refs).await?
        }
        TaskHydration::BulkUpdate => {
            load_task_bulk_update_enrichment(conn, workspace_id, &task_ids).await?
        }
        TaskHydration::Detail => {
            load_task_enrichment(conn, workspace_id, &task_ids, display_refs).await?
        }
        TaskHydration::DetailWithoutActivity => {
            load_task_enrichment_without_activity(conn, workspace_id, &task_ids, display_refs)
                .await?
        }
    };

    let mut items = Vec::with_capacity(tasks.len());
    for task in tasks {
        let task_id = task.id.clone();
        let display_ref = display_refs.display_ref(&task);
        let labels = enrichment
            .labels_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let notes = enrichment
            .notes_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let has_notes = match hydration {
            TaskHydration::Detail | TaskHydration::DetailWithoutActivity => !notes.is_empty(),
            TaskHydration::List => enrichment.task_ids_with_notes.contains(&task_id),
            TaskHydration::BulkUpdate => false,
        };
        let attachments = enrichment
            .attachments_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let metadata = enrichment
            .metadata_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let activity = enrichment
            .activity_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let has_conflict = enrichment.conflicted_task_ids.contains(&task_id);
        let unresolved_blocker_count = *enrichment
            .unresolved_blocker_counts_by_task
            .get(&task_id)
            .unwrap_or(&0);
        let dependent_count = *enrichment
            .dependent_counts_by_task
            .get(&task_id)
            .unwrap_or(&0);
        let depends_on = enrichment
            .depends_on_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let blocks = enrichment
            .blocks_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let related = enrichment
            .related_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let epic_children = enrichment
            .epic_children_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let epic_child_dependencies = epic_children
            .iter()
            .filter_map(|child| {
                enrichment
                    .epic_child_dependencies_by_task
                    .get(&child.task_id)
                    .cloned()
                    .map(|dependencies| (child.task_id.clone(), dependencies))
            })
            .collect();
        let epic_parent = enrichment.epic_parent_by_task.remove(&task_id);
        let epic_rollup = task.is_epic.then(|| {
            let mut rollup = enrichment
                .epic_rollups_by_task
                .remove(&task_id)
                .unwrap_or_default();
            if rollup.latest_activity_at < task.updated_at {
                rollup.latest_activity_at.clone_from(&task.updated_at);
            }
            rollup
        });
        let recurrence = enrichment.recurrence_by_task.remove(&task_id);
        let queue = queue_meta_on(
            &task,
            has_conflict,
            unresolved_blocker_count > 0,
            dependent_count,
            now_seconds,
            local_today,
        );

        items.push(TaskListItem {
            task,
            display_ref,
            labels,
            notes,
            has_notes,
            attachments,
            metadata,
            activity,
            has_conflict,
            unresolved_blocker_count,
            dependent_count,
            depends_on,
            blocks,
            related,
            epic_children,
            epic_child_dependencies,
            epic_parent,
            epic_rollup,
            queue,
            recurrence,
            recurrence_group: None,
            hydration: match hydration {
                TaskHydration::Detail | TaskHydration::DetailWithoutActivity => {
                    TaskItemHydration::Detail
                }
                TaskHydration::List | TaskHydration::BulkUpdate => TaskItemHydration::Summary,
            },
        });
    }

    Ok(items)
}
