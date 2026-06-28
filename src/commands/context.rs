use anyhow::Result;
use serde::Serialize;
use sqlx::{Row, SqliteConnection};

use super::conflicts::conflict_display_value;
use crate::cli::ContextArgs;
use crate::operations::{ConflictDetail, task_conflicts};
use crate::query::{TaskDependencyItem, task_dependency_summary};
use crate::refs::{display_ref, display_suffix, resolve_task_ref};
use crate::render::{print_multiline_block, quote};
use crate::task_render::labels_for_task_in_workspace;
use crate::types::Task;
use crate::workspaces::active_workspace;

pub(crate) async fn cmd_context(conn: &mut SqliteConnection, args: ContextArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    let snapshot = task_context_snapshot(conn, &task).await?;
    if args.json {
        serde_json::to_writer_pretty(std::io::stdout(), &snapshot)?;
        println!();
    } else {
        print_task_context(&snapshot);
    }
    Ok(())
}

#[derive(Serialize)]
struct TaskContextSnapshot {
    task: ContextTask,
    project: ContextProject,
    workspace: ContextWorkspace,
    labels: Vec<String>,
    dependencies: ContextDependencies,
    notes: Vec<ContextNote>,
    conflicts: Vec<ContextConflict>,
    has_conflicts: bool,
    is_blocked: bool,
    has_open_dependents: bool,
}

#[derive(Serialize)]
struct ContextTask {
    id: String,
    ref_suffix: String,
    display_ref: String,
    title: String,
    description: String,
    status: String,
    priority: String,
    deleted: bool,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct ContextProject {
    id: String,
    key: String,
    name: String,
    prefix: String,
}

#[derive(Serialize)]
struct ContextWorkspace {
    id: String,
    key: String,
    name: String,
}

#[derive(Serialize)]
struct ContextDependencies {
    depends_on_open: usize,
    depends_on_total: usize,
    blocks_open: usize,
    blocks_total: usize,
    depends_on: Vec<ContextDependencyTask>,
    blocks: Vec<ContextDependencyTask>,
}

#[derive(Serialize)]
struct ContextDependencyTask {
    id: String,
    display_ref: String,
    title: String,
    status: String,
    priority: String,
    deleted: bool,
    unresolved: bool,
    created_at: String,
}

#[derive(Serialize)]
struct ContextNote {
    id: String,
    created_at: String,
    body: String,
}

#[derive(Serialize)]
struct ContextConflict {
    field: String,
    variants: Vec<ContextConflictVariant>,
}

#[derive(Serialize)]
struct ContextConflictVariant {
    token: String,
    value: String,
}

async fn task_context_snapshot(
    conn: &mut SqliteConnection,
    task: &Task,
) -> Result<TaskContextSnapshot> {
    let workspace = active_workspace();
    let display_ref = display_ref(conn, task).await?;
    let ref_suffix = display_suffix(conn, &task.id).await?;
    let labels = labels_for_task_in_workspace(conn, &task.workspace_id, &task.id).await?;
    let summary = task_dependency_summary(conn, &task.workspace_id, &task.id).await?;
    let notes = load_context_notes(conn, &task.workspace_id, &task.id).await?;
    let details = task_conflicts(conn, &task.id, None).await?;

    let depends_on_open = summary
        .depends_on
        .iter()
        .filter(|item| item.unresolved)
        .count();
    let blocks_open = summary.blocks.iter().filter(|item| item.unresolved).count();
    let depends_on_total = summary.depends_on.len();
    let blocks_total = summary.blocks.len();
    let has_conflicts = !details.is_empty();
    let is_blocked = depends_on_open > 0;
    let has_open_dependents = blocks_open > 0;

    Ok(TaskContextSnapshot {
        task: ContextTask {
            id: task.id.clone(),
            ref_suffix,
            display_ref,
            title: task.title.clone(),
            description: task.description.clone(),
            status: task.status.clone(),
            priority: task.priority.clone(),
            deleted: task.deleted,
            created_at: task.created_at.clone(),
            updated_at: task.updated_at.clone(),
        },
        project: ContextProject {
            id: task.project_id.clone(),
            key: task.project_key.clone(),
            name: context_project_name(conn, &task.workspace_id, &task.project_id).await?,
            prefix: task.project_prefix.clone(),
        },
        workspace: ContextWorkspace {
            id: workspace.id,
            key: workspace.key,
            name: workspace.name,
        },
        labels,
        dependencies: ContextDependencies {
            depends_on_open,
            depends_on_total,
            blocks_open,
            blocks_total,
            depends_on: summary
                .depends_on
                .into_iter()
                .map(context_dependency_task)
                .collect(),
            blocks: summary
                .blocks
                .into_iter()
                .map(context_dependency_task)
                .collect(),
        },
        notes,
        conflicts: context_conflicts(conn, &task.workspace_id, details).await?,
        has_conflicts,
        is_blocked,
        has_open_dependents,
    })
}

async fn load_context_notes(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    task_id: &str,
) -> Result<Vec<ContextNote>> {
    let rows = sqlx::query(
        "SELECT id, body, created_at FROM notes
         WHERE workspace_id = ? AND task_id = ? ORDER BY created_at, id",
    )
    .bind(workspace_id)
    .bind(task_id)
    .fetch_all(&mut *conn)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| ContextNote {
            id: row.get("id"),
            created_at: row.get("created_at"),
            body: row.get("body"),
        })
        .collect())
}

fn context_dependency_task(item: TaskDependencyItem) -> ContextDependencyTask {
    ContextDependencyTask {
        id: item.task.id,
        display_ref: item.display_ref,
        title: item.task.title,
        status: item.task.status,
        priority: item.task.priority,
        deleted: item.task.deleted,
        unresolved: item.unresolved,
        created_at: item.created_at,
    }
}

async fn context_project_name(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project_id: &str,
) -> Result<String> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT name FROM projects WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(project_id)
    .fetch_one(&mut *conn)
    .await?)
}

async fn context_conflicts(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    details: Vec<ConflictDetail>,
) -> Result<Vec<ContextConflict>> {
    let mut conflicts = Vec::with_capacity(details.len());
    for detail in details {
        let local_value =
            conflict_display_value(conn, workspace_id, &detail.field, &detail.local_value).await?;
        let remote_value =
            conflict_display_value(conn, workspace_id, &detail.field, &detail.remote_value).await?;
        conflicts.push(ContextConflict {
            field: detail.field,
            variants: vec![
                ContextConflictVariant {
                    token: detail.variant_a,
                    value: local_value,
                },
                ContextConflictVariant {
                    token: detail.variant_b,
                    value: remote_value,
                },
            ],
        });
    }
    Ok(conflicts)
}

fn print_task_context(snapshot: &TaskContextSnapshot) {
    println!(
        "context {} suffix={} id={} status={} priority={} deleted={} blocked={} conflicts={} blocks_open={} labels={} title={}",
        snapshot.task.display_ref,
        snapshot.task.ref_suffix,
        snapshot.task.id,
        snapshot.task.status,
        snapshot.task.priority,
        yes_no(snapshot.task.deleted),
        yes_no(snapshot.is_blocked),
        yes_no(snapshot.has_conflicts),
        yes_no(snapshot.has_open_dependents),
        snapshot.labels.join(","),
        quote(&snapshot.task.title),
    );
    println!(
        "project={} prefix={} name={}",
        snapshot.project.key,
        snapshot.project.prefix,
        quote(&snapshot.project.name)
    );
    println!("workspace={}", snapshot.workspace.key);
    println!(
        "created={} updated={}",
        snapshot.task.created_at, snapshot.task.updated_at
    );
    if !snapshot.task.description.is_empty() {
        print_multiline_block("description", &snapshot.task.description);
    }
    let deps = &snapshot.dependencies;
    println!(
        "depends_on open={} total={}",
        deps.depends_on_open, deps.depends_on_total
    );
    for item in &deps.depends_on {
        println!(
            "- {} status={} unresolved={} title={}",
            item.display_ref,
            item.status,
            yes_no(item.unresolved),
            quote(&item.title),
        );
    }
    println!(
        "blocks open={} total={}",
        deps.blocks_open, deps.blocks_total
    );
    for item in &deps.blocks {
        println!(
            "- {} status={} unresolved={} title={}",
            item.display_ref,
            item.status,
            yes_no(item.unresolved),
            quote(&item.title),
        );
    }
    for note in &snapshot.notes {
        println!("note created={}", note.created_at);
        print_multiline_block("body", &note.body);
    }
    for conflict in &snapshot.conflicts {
        println!(
            "conflict {} field={}",
            snapshot.task.display_ref, conflict.field
        );
        for variant in &conflict.variants {
            println!("variant {}", variant.token);
            print_multiline_block("value", &variant.value);
        }
    }
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
