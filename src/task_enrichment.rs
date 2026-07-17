use crate::ids::{TaskId, WorkspaceId};
use std::collections::{HashMap, HashSet};

use crate::attachments::AttachmentBytesState;
use crate::query::fragments;
use crate::query::{TaskDependencyLink, TaskNote};
use crate::refs::DisplayRefContext;
use crate::task_render::AttachmentMetadataJson;
use anyhow::Result;
use sqlx::{QueryBuilder, Row, Sqlite, SqliteConnection};

const SQLITE_BIND_CHUNK_SIZE: usize = 900;

pub(crate) struct TaskEnrichment {
    pub(crate) labels_by_task: HashMap<TaskId, Vec<String>>,
    pub(crate) notes_by_task: HashMap<TaskId, Vec<TaskNote>>,
    pub(crate) attachments_by_task: HashMap<TaskId, Vec<AttachmentMetadataJson>>,
    pub(crate) conflicted_task_ids: HashSet<TaskId>,
    pub(crate) unresolved_blocker_counts_by_task: HashMap<TaskId, i64>,
    pub(crate) dependent_counts_by_task: HashMap<TaskId, i64>,
    pub(crate) depends_on_by_task: HashMap<TaskId, Vec<TaskDependencyLink>>,
    pub(crate) blocks_by_task: HashMap<TaskId, Vec<TaskDependencyLink>>,
    pub(crate) epic_children_by_task: HashMap<TaskId, Vec<TaskDependencyLink>>,
    pub(crate) epic_parent_by_task: HashMap<TaskId, TaskDependencyLink>,
}

pub(crate) async fn load_task_enrichment(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<TaskEnrichment> {
    Ok(TaskEnrichment {
        labels_by_task: labels_for_tasks(conn, workspace_id, task_ids).await?,
        notes_by_task: notes_for_tasks(conn, workspace_id, task_ids).await?,
        attachments_by_task: attachments_for_tasks(conn, workspace_id, task_ids).await?,
        conflicted_task_ids: tasks_with_unresolved_conflicts(conn, workspace_id, task_ids).await?,
        unresolved_blocker_counts_by_task: unresolved_blocker_counts_for_tasks(
            conn,
            workspace_id,
            task_ids,
        )
        .await?,
        dependent_counts_by_task: dependent_counts_for_tasks(conn, workspace_id, task_ids).await?,
        depends_on_by_task: dependency_links_for_tasks(
            conn,
            workspace_id,
            task_ids,
            false,
            display_refs,
        )
        .await?,
        blocks_by_task: dependency_links_for_tasks(
            conn,
            workspace_id,
            task_ids,
            true,
            display_refs,
        )
        .await?,
        epic_children_by_task: epic_children_for_tasks(conn, workspace_id, task_ids, display_refs)
            .await?,
        epic_parent_by_task: epic_parents_for_tasks(conn, workspace_id, task_ids, display_refs)
            .await?,
    })
}

async fn attachments_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, Vec<AttachmentMetadataJson>>> {
    let mut attachments_by_task = HashMap::new();
    if task_ids.is_empty() {
        return Ok(attachments_by_task);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT ta.attachment_id, ta.task_id, ta.sha256, ta.media_type, ta.byte_size,
                    ta.filename, ta.alt_text, ta.width, ta.height, ta.created_at,
                    ta.deleted, ta.deleted_at,
                    CASE WHEN bi.sha256 IS NULL THEN 0 ELSE 1 END AS has_inventory,
                    COALESCE(bi.available, 0) AS has_blob
             FROM task_attachments ta
             LEFT JOIN blob_inventory bi ON bi.sha256 = ta.sha256
             WHERE ta.workspace_id =",
        );
        query.push_bind(workspace_id);
        query.push(" AND ta.task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") AND ta.deleted = 0 ORDER BY ta.task_id, ta.created_at, ta.attachment_id");

        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: TaskId = row.get("task_id");
            let has_blob = row.get::<i64, _>("has_blob") != 0;
            let bytes_state = if has_blob {
                AttachmentBytesState::Present
            } else if row.get::<i64, _>("has_inventory") != 0 {
                AttachmentBytesState::Unavailable
            } else {
                AttachmentBytesState::PendingDownload
            };
            let attachment = AttachmentMetadataJson {
                attachment_id: row.get("attachment_id"),
                task_id: task_id.to_string(),
                sha256: row.get("sha256"),
                media_type: row.get("media_type"),
                byte_size: row.get("byte_size"),
                filename: row.get("filename"),
                alt_text: row.get("alt_text"),
                width: row.get("width"),
                height: row.get("height"),
                created_at: row.get("created_at"),
                deleted: row.get::<i64, _>("deleted") != 0,
                deleted_at: row.get("deleted_at"),
                bytes_state,
                has_blob,
            };
            attachments_by_task
                .entry(task_id)
                .or_insert_with(Vec::new)
                .push(attachment);
        }
    }
    Ok(attachments_by_task)
}

async fn notes_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, Vec<TaskNote>>> {
    let mut notes_by_task = HashMap::new();
    if task_ids.is_empty() {
        return Ok(notes_by_task);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT task_id, body, created_at FROM notes WHERE workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY task_id, created_at DESC, id DESC");

        for row in query.build().fetch_all(&mut *conn).await? {
            let task_id: TaskId = row.get("task_id");
            let note = TaskNote {
                body: row.get("body"),
                created_at: row.get("created_at"),
            };
            notes_by_task
                .entry(task_id)
                .or_insert_with(Vec::new)
                .push(note);
        }
    }
    Ok(notes_by_task)
}

async fn labels_for_tasks(
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

async fn unresolved_blocker_counts_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, i64>> {
    let mut counts = HashMap::new();
    if task_ids.is_empty() {
        return Ok(counts);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT d.task_id, COUNT(*) AS blockers
             FROM task_dependencies d
             JOIN tasks blocker
              ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
             WHERE d.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND d.task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(format!(
            ") AND {} GROUP BY d.task_id",
            fragments::open_task_clause("blocker"),
        ));

        for row in query.build().fetch_all(&mut *conn).await? {
            counts.insert(row.get("task_id"), row.get::<i64, _>("blockers"));
        }
    }
    Ok(counts)
}

async fn dependent_counts_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
) -> Result<HashMap<TaskId, i64>> {
    let mut counts = HashMap::new();
    if task_ids.is_empty() {
        return Ok(counts);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT d.depends_on_task_id, COUNT(*) AS dependents
             FROM task_dependencies d
             JOIN tasks blocker
              ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
             JOIN tasks dependent
              ON dependent.workspace_id = d.workspace_id AND dependent.id = d.task_id
             WHERE d.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND d.depends_on_task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(format!(
            ") AND {} AND {} GROUP BY d.depends_on_task_id",
            fragments::open_task_clause("blocker"),
            fragments::open_task_clause("dependent"),
        ));

        for row in query.build().fetch_all(&mut *conn).await? {
            counts.insert(
                row.get("depends_on_task_id"),
                row.get::<i64, _>("dependents"),
            );
        }
    }
    Ok(counts)
}

async fn dependency_links_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    blocks_only: bool,
    display_refs: &DisplayRefContext,
) -> Result<HashMap<TaskId, Vec<TaskDependencyLink>>> {
    let mut links = HashMap::new();
    if task_ids.is_empty() {
        return Ok(links);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        if chunk.is_empty() {
            continue;
        }
        let initial = if blocks_only {
            format!(
                "SELECT d.depends_on_task_id AS source_task_id,
                        t.id, t.title, t.status, t.priority, p.prefix AS project_prefix,
                        d.created_at AS dependency_created_at,
                        CASE
                            WHEN {}
                             AND {}
                            THEN 1 ELSE 0
                        END AS unresolved
                 FROM task_dependencies d
                 JOIN tasks blocker
                  ON blocker.workspace_id = d.workspace_id AND blocker.id = d.depends_on_task_id
                 JOIN tasks t
                  ON t.workspace_id = d.workspace_id AND t.id = d.task_id
                 JOIN projects p
                  ON p.workspace_id = t.workspace_id AND p.id = t.project_id
                 WHERE d.workspace_id =",
                fragments::open_task_clause("blocker"),
                fragments::open_task_clause("t"),
            )
        } else {
            format!(
                "SELECT d.task_id AS source_task_id,
                        t.id, t.title, t.status, t.priority, p.prefix AS project_prefix,
                        d.created_at AS dependency_created_at,
                        CASE
                            WHEN {}
                            THEN 1 ELSE 0
                        END AS unresolved
                 FROM task_dependencies d
                 JOIN tasks t
                  ON t.workspace_id = d.workspace_id AND t.id = d.depends_on_task_id
                 JOIN projects p
                  ON p.workspace_id = t.workspace_id AND p.id = t.project_id
                 WHERE d.workspace_id =",
                fragments::open_task_clause("t"),
            )
        };
        let mut query = QueryBuilder::<Sqlite>::new(&initial);
        query.push_bind(workspace_id);
        let source_column = if blocks_only {
            "d.depends_on_task_id"
        } else {
            "d.task_id"
        };
        query.push(" AND ");
        query.push(source_column);
        query.push(" IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") ORDER BY unresolved DESC, t.status, t.title, d.created_at, t.id");

        for row in query.build().fetch_all(&mut *conn).await? {
            let source_task_id: TaskId = row.get("source_task_id");
            let task_id: TaskId = row.get("id");
            let project_prefix: String = row.get("project_prefix");
            links
                .entry(source_task_id)
                .or_insert_with(Vec::new)
                .push(TaskDependencyLink {
                    task_id: task_id.clone(),
                    display_ref: display_refs.display_ref_for_id(
                        workspace_id,
                        &project_prefix,
                        &task_id,
                    ),
                    title: row.get("title"),
                    status: row.get("status"),
                    priority: row.get("priority"),
                    unresolved: row.get::<i64, _>("unresolved") != 0,
                });
        }
    }
    Ok(links)
}

async fn epic_children_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<HashMap<TaskId, Vec<TaskDependencyLink>>> {
    let mut links = HashMap::new();
    if task_ids.is_empty() {
        return Ok(links);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT l.epic_task_id AS source_task_id,
                    t.id, t.title, t.status, t.priority, p.prefix AS project_prefix,
                    CASE WHEN ",
        );
        query.push(fragments::open_task_clause("t"));
        query.push(
            " THEN 1 ELSE 0 END AS unresolved
             FROM task_epic_links l
             JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.child_task_id
             JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
             WHERE l.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND l.epic_task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(
            ") AND t.deleted = 0 ORDER BY unresolved DESC, t.status, t.title, l.created_at, t.id",
        );

        for row in query.build().fetch_all(&mut *conn).await? {
            let source_task_id: TaskId = row.get("source_task_id");
            let task_id: TaskId = row.get("id");
            let project_prefix: String = row.get("project_prefix");
            links
                .entry(source_task_id)
                .or_insert_with(Vec::new)
                .push(TaskDependencyLink {
                    task_id: task_id.clone(),
                    display_ref: display_refs.display_ref_for_id(
                        workspace_id,
                        &project_prefix,
                        &task_id,
                    ),
                    title: row.get("title"),
                    status: row.get("status"),
                    priority: row.get("priority"),
                    unresolved: row.get::<i64, _>("unresolved") != 0,
                });
        }
    }
    Ok(links)
}

async fn epic_parents_for_tasks(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    task_ids: &[TaskId],
    display_refs: &DisplayRefContext,
) -> Result<HashMap<TaskId, TaskDependencyLink>> {
    let mut links = HashMap::new();
    if task_ids.is_empty() {
        return Ok(links);
    }
    for chunk in task_ids.chunks(SQLITE_BIND_CHUNK_SIZE) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "SELECT l.child_task_id AS source_task_id,
                    t.id, t.title, t.status, t.priority, p.prefix AS project_prefix,
                    CASE WHEN ",
        );
        query.push(fragments::open_task_clause("t"));
        query.push(
            " THEN 1 ELSE 0 END AS unresolved
             FROM task_epic_links l
             JOIN tasks t ON t.workspace_id = l.workspace_id AND t.id = l.epic_task_id
             JOIN projects p ON p.workspace_id = t.workspace_id AND p.id = t.project_id
             WHERE l.workspace_id = ",
        );
        query.push_bind(workspace_id);
        query.push(" AND l.child_task_id IN (");
        {
            let mut separated = query.separated(", ");
            for task_id in chunk {
                separated.push_bind(task_id);
            }
        }
        query.push(") AND t.deleted = 0 ORDER BY t.title, l.created_at, t.id");

        for row in query.build().fetch_all(&mut *conn).await? {
            let source_task_id: TaskId = row.get("source_task_id");
            let task_id: TaskId = row.get("id");
            let project_prefix: String = row.get("project_prefix");
            links.insert(
                source_task_id,
                TaskDependencyLink {
                    task_id: task_id.clone(),
                    display_ref: display_refs.display_ref_for_id(
                        workspace_id,
                        &project_prefix,
                        &task_id,
                    ),
                    title: row.get("title"),
                    status: row.get("status"),
                    priority: row.get("priority"),
                    unresolved: row.get::<i64, _>("unresolved") != 0,
                },
            );
        }
    }
    Ok(links)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn attachment_enrichment_orders_live_rows_and_distinguishes_byte_states() {
        let (_temp, mut conn) = crate::test_support::test_conn().await;
        let workspace_id = crate::workspaces::DEFAULT_WORKSPACE_ID;
        let task_id = "TASK000000000001";
        sqlx::query(
            "INSERT INTO task_attachments(workspace_id, attachment_id, task_id, sha256, byte_size, media_type, created_at, deleted, deleted_at)
             VALUES (?, 'ATTACHMENT000002', ?, 'bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 2, 'image/png', '001', 0, NULL),
                    (?, 'ATTACHMENT000001', ?, 'aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1, 'image/png', '001', 0, NULL),
                    (?, 'ATTACHMENT000003', ?, 'cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc', 3, 'image/png', '002', 0, NULL),
                    (?, 'ATTACHMENT000004', ?, 'dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd', 4, 'image/png', '003', 1, '004')",
        )
        .bind(workspace_id)
        .bind(task_id)
        .bind(workspace_id)
        .bind(task_id)
        .bind(workspace_id)
        .bind(task_id)
        .bind(workspace_id)
        .bind(task_id)
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO blob_inventory(sha256, byte_size, media_type, available, first_seen_at)
             VALUES ('aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa', 1, 'image/png', 1, '001'),
                    ('bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb', 2, 'image/png', 0, '001')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        let attachments = attachments_for_tasks(&mut conn, workspace_id, &[task_id.to_string()])
            .await
            .unwrap()
            .remove(task_id)
            .unwrap();

        assert_eq!(
            attachments
                .iter()
                .map(|attachment| attachment.attachment_id.as_str())
                .collect::<Vec<_>>(),
            ["ATTACHMENT000001", "ATTACHMENT000002", "ATTACHMENT000003"]
        );
        assert_eq!(attachments[0].bytes_state, AttachmentBytesState::Present);
        assert_eq!(
            attachments[1].bytes_state,
            AttachmentBytesState::Unavailable
        );
        assert_eq!(
            attachments[2].bytes_state,
            AttachmentBytesState::PendingDownload
        );
    }

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
