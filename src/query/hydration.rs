use anyhow::Result;
use sqlx::SqliteConnection;

use crate::queue::queue_meta;
use crate::refs::display_refs_for_tasks;
use crate::task_enrichment::load_task_enrichment;
use crate::types::Task;

use super::TaskListItem;

/// Build a `Vec<TaskListItem>` from tasks by loading enrichment and display refs.
///
/// Preserves input task order. Callers are responsible for any post-processing
/// such as sorting, filtering, or truncation.
pub(crate) async fn build_task_list_items(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    tasks: Vec<Task>,
    now_seconds: i64,
) -> Result<Vec<TaskListItem>> {
    let display_refs = display_refs_for_tasks(conn, &tasks).await?;
    let task_ids = tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>();
    let mut enrichment = load_task_enrichment(conn, workspace_id, &task_ids).await?;

    let mut items = Vec::with_capacity(tasks.len());
    for task in tasks {
        let task_id = task.id.clone();
        let display_ref = display_refs
            .get(&task_id)
            .cloned()
            .unwrap_or_else(|| format!("{}-{}", task.project_prefix, task_id));
        let labels = enrichment
            .labels_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let notes = enrichment
            .notes_by_task
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
        let epic_children = enrichment
            .epic_children_by_task
            .remove(&task_id)
            .unwrap_or_default();
        let epic_parent = enrichment.epic_parent_by_task.remove(&task_id);
        let queue = queue_meta(
            &task,
            has_conflict,
            unresolved_blocker_count > 0,
            now_seconds,
        );

        items.push(TaskListItem {
            task,
            display_ref,
            labels,
            notes,
            has_conflict,
            unresolved_blocker_count,
            dependent_count,
            depends_on,
            blocks,
            epic_children,
            epic_parent,
            queue,
        });
    }

    Ok(items)
}
