use crate::ids::{TaskId, WorkspaceId};
use crate::metadata::TaskMetadataValue;
use std::collections::{HashMap, HashSet};

use crate::query::{
    AttachmentMetadata, EpicRollup, RecentActionItem, TaskConflictValue, TaskDependencyLink,
    TaskNote, TaskRecurrenceSummary,
};
use crate::refs::DisplayRefContext;
use anyhow::Result;
use sqlx::sqlite::SqliteRow;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

mod attachments;
mod dependencies;
mod epics;
mod notes;

const SQLITE_BIND_CHUNK_SIZE: usize = 900;

#[derive(Default)]
pub struct TaskEnrichment {
    pub labels_by_task: HashMap<TaskId, Vec<String>>,
    pub notes_by_task: HashMap<TaskId, Vec<TaskNote>>,
    pub task_ids_with_notes: HashSet<TaskId>,
    pub attachments_by_task: HashMap<TaskId, Vec<AttachmentMetadata>>,
    pub live_attachment_counts_by_task: HashMap<TaskId, u32>,
    pub metadata_by_task: HashMap<TaskId, Vec<TaskMetadataValue>>,
    pub activity_by_task: HashMap<TaskId, Vec<RecentActionItem>>,
    pub conflicts_by_task: HashMap<TaskId, Vec<TaskConflictValue>>,
    pub conflicted_task_ids: HashSet<TaskId>,
    pub unresolved_blocker_counts_by_task: HashMap<TaskId, i64>,
    pub dependent_counts_by_task: HashMap<TaskId, i64>,
    pub depends_on_by_task: HashMap<TaskId, Vec<TaskDependencyLink>>,
    pub blocks_by_task: HashMap<TaskId, Vec<TaskDependencyLink>>,
    pub related_by_task: HashMap<TaskId, Vec<crate::query::TaskRelatedLink>>,
    pub epic_children_by_task: HashMap<TaskId, Vec<TaskDependencyLink>>,
    pub epic_child_dependencies_by_task: HashMap<TaskId, Vec<TaskDependencyLink>>,
    pub epic_parent_by_task: HashMap<TaskId, TaskDependencyLink>,
    pub epic_rollups_by_task: HashMap<TaskId, EpicRollup>,
    pub recurrence_by_task: HashMap<TaskId, TaskRecurrenceSummary>,
}

pub async fn load_task_enrichment(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<TaskEnrichment> {
    load_task_enrichment_with_detail(conn, workspace_id, task_ids, display_refs, true, true).await
}

pub(crate) async fn load_task_enrichment_without_activity(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<TaskEnrichment> {
    load_task_enrichment_with_detail(conn, workspace_id, task_ids, display_refs, true, false).await
}

pub(crate) async fn load_task_list_enrichment(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<TaskEnrichment> {
    load_task_enrichment_with_detail(conn, workspace_id, task_ids, display_refs, false, false).await
}

pub(crate) async fn load_task_bulk_update_enrichment(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<TaskEnrichment> {
    Ok(TaskEnrichment {
        labels_by_task: labels_for_tasks(conn, workspace_id, task_ids).await?,
        metadata_by_task: crate::metadata::metadata_by_task_ids(conn, workspace_id, task_ids)
            .await?,
        recurrence_by_task: crate::query::task_recurrence_summaries(conn, workspace_id, task_ids)
            .await?,
        ..TaskEnrichment::default()
    })
}

async fn load_task_enrichment_with_detail(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
    include_detail: bool,
    include_activity: bool,
) -> Result<TaskEnrichment> {
    let (notes_by_task, task_ids_with_notes, attachments_by_task, metadata_by_task) =
        if include_detail {
            (
                notes::notes_for_tasks(conn, workspace_id, task_ids).await?,
                HashSet::new(),
                attachments::attachments_for_tasks(conn, workspace_id, task_ids).await?,
                crate::metadata::metadata_by_task_ids(conn, workspace_id, task_ids).await?,
            )
        } else {
            (
                HashMap::new(),
                notes::task_ids_with_notes(conn, workspace_id, task_ids).await?,
                HashMap::new(),
                HashMap::new(),
            )
        };
    let live_attachment_counts_by_task = if include_detail {
        attachments_by_task
            .iter()
            .map(|(task_id, attachments)| {
                (
                    task_id.clone(),
                    attachments.len().min(u32::MAX as usize) as u32,
                )
            })
            .collect()
    } else {
        attachments::live_attachment_counts_for_tasks(conn, workspace_id, task_ids).await?
    };
    let activity_by_task = if include_activity {
        crate::query::task_activity_for_tasks_in_workspace(conn, workspace_id, task_ids).await?
    } else {
        HashMap::new()
    };
    let (conflicted_task_ids, conflicts_by_task) = if include_detail {
        conflicts_for_tasks(conn, workspace_id, task_ids).await?
    } else {
        (
            tasks_with_unresolved_conflicts(conn, workspace_id, task_ids).await?,
            HashMap::new(),
        )
    };
    let epic_children_by_task =
        epics::epic_children_for_tasks(conn, workspace_id, task_ids, display_refs).await?;
    let epic_child_dependencies_by_task = if include_detail {
        let child_ids = epic_children_by_task
            .values()
            .flatten()
            .map(|child| child.task_id.clone())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        dependencies::dependency_links_for_tasks(
            conn,
            workspace_id,
            &child_ids,
            false,
            display_refs,
        )
        .await?
    } else {
        HashMap::new()
    };
    Ok(TaskEnrichment {
        labels_by_task: labels_for_tasks(conn, workspace_id, task_ids).await?,
        notes_by_task,
        task_ids_with_notes,
        attachments_by_task,
        live_attachment_counts_by_task,
        metadata_by_task,
        activity_by_task,
        conflicts_by_task,
        conflicted_task_ids,
        unresolved_blocker_counts_by_task: dependencies::unresolved_blocker_counts_for_tasks(
            conn,
            workspace_id,
            task_ids,
        )
        .await?,
        dependent_counts_by_task: dependencies::dependent_counts_for_tasks(
            conn,
            workspace_id,
            task_ids,
        )
        .await?,
        depends_on_by_task: dependencies::dependency_links_for_tasks(
            conn,
            workspace_id,
            task_ids,
            false,
            display_refs,
        )
        .await?,
        blocks_by_task: dependencies::dependency_links_for_tasks(
            conn,
            workspace_id,
            task_ids,
            true,
            display_refs,
        )
        .await?,
        related_by_task: if include_detail {
            crate::query::related_links_for_tasks(conn, workspace_id, task_ids, display_refs)
                .await?
        } else {
            HashMap::new()
        },
        epic_children_by_task,
        epic_child_dependencies_by_task,
        epic_parent_by_task: epics::epic_parents_for_tasks(
            conn,
            workspace_id,
            task_ids,
            display_refs,
        )
        .await?,
        epic_rollups_by_task: epics::epic_rollups_for_tasks(conn, workspace_id, task_ids).await?,
        recurrence_by_task: crate::query::task_recurrence_summaries(conn, workspace_id, task_ids)
            .await?,
    })
}
pub(crate) async fn labels_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, Vec<String>>> {
    let mut labels_by_task = HashMap::new();
    if task_ids.is_empty() {
        return Ok(labels_by_task);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT task_id, label FROM task_labels WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY task_id, label");

        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: TaskId = row.get("task_id");
            let label: String = row.get("label");
            labels_by_task
                .entry(task_id)
                .or_insert_with(Vec::new)
                .push(label);
        }
    }
    Ok(labels_by_task)
}

fn dependency_link_from_row(
    row: &SqliteRow,
    workspace_id: &WorkspaceId,
    display_refs: &DisplayRefContext,
) -> TaskDependencyLink {
    let task_id: TaskId = row.get("id");
    let project_prefix: String = row.get("project_prefix");
    TaskDependencyLink {
        project_key: row.get("project_key"),
        task_id: task_id.clone(),
        display_ref: display_refs.display_ref_for_id(workspace_id, &project_prefix, &task_id),
        title: row.get("title"),
        status: row.get("status"),
        priority: row.get("priority"),
        unresolved: row.get::<i64, _>("unresolved") != 0,
    }
}
async fn conflicts_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<(HashSet<TaskId>, HashMap<TaskId, Vec<TaskConflictValue>>)> {
    let mut conflicted = HashSet::new();
    let mut conflicts_by_task = HashMap::new();
    if task_ids.is_empty() {
        return Ok((conflicted, conflicts_by_task));
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT task_id, field, local_value, remote_value
             FROM conflicts WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND resolved = 0 AND task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY task_id, field, id");

        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: TaskId = row.get("task_id");
            conflicted.insert(task_id.clone());
            conflicts_by_task
                .entry(task_id)
                .or_insert_with(Vec::new)
                .push(TaskConflictValue {
                    field: row.get("field"),
                    local_value: row.get("local_value"),
                    remote_value: row.get("remote_value"),
                });
        }
    }
    Ok((conflicted, conflicts_by_task))
}

async fn tasks_with_unresolved_conflicts(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashSet<TaskId>> {
    let mut conflicted = HashSet::new();
    if task_ids.is_empty() {
        return Ok(conflicted);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT DISTINCT task_id FROM conflicts WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND resolved = 0 AND task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(")");

        for row in query.build().fetch_all(&mut *conn).await? {
            conflicted.insert(row.get("task_id"));
        }
    }
    Ok(conflicted)
}
pub(crate) async fn epic_parents_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<HashMap<TaskId, TaskDependencyLink>> {
    epics::epic_parents_for_tasks(conn, workspace_id, task_ids, display_refs).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn task_enrichment_loads_notes_across_bind_chunks() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace_id = crate::workspaces::default_workspace_id();
        let task_ids = (0..=SQLITE_BIND_CHUNK_SIZE)
            .map(|index| format!("{index:016}").parse().unwrap())
            .collect::<Vec<TaskId>>();

        sqlx::query(
            "INSERT INTO notes(workspace_id, id, task_id, body, created_at, change_id)
             VALUES (?, 'note-first-old', '0000000000000000', 'older', '001', 'change-first-old'),
                    (?, 'note-first-new', '0000000000000000', 'newer', '002', 'change-first-new'),
                    (?, 'note-last', ?, 'last', '003', 'change-last')",
        )
        .bind(&workspace_id)
        .bind(&workspace_id)
        .bind(&workspace_id)
        .bind(task_ids.last().unwrap())
        .execute(&mut *conn)
        .await
        .unwrap();

        let display_refs = DisplayRefContext::for_workspace(&mut conn, &workspace_id)
            .await
            .unwrap();
        let enrichment = load_task_enrichment(&mut conn, &workspace_id, &task_ids, &display_refs)
            .await
            .unwrap();

        assert_eq!(
            enrichment
                .notes_by_task
                .get("0000000000000000")
                .unwrap()
                .iter()
                .map(|note| note.body.as_str())
                .collect::<Vec<_>>(),
            ["newer", "older"]
        );
        assert_eq!(
            enrichment
                .notes_by_task
                .get(task_ids.last().unwrap())
                .unwrap()[0]
                .body,
            "last"
        );
    }
}
