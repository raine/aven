use anyhow::{Result, bail};
use serde_json::json;
use sqlx::SqliteConnection;

use crate::cli::{DepCommand, DepSubcommand, EpicCommand, EpicSubcommand};
use crate::operations::{
    add_task_dependency, add_task_to_epic, remove_task_dependency, remove_task_from_epic,
};
use crate::query::{self, SortDirection, TaskFilters, TaskQueryMode, TaskSort};
use crate::refs::{DisplayRefContext, resolve_task_ref_in_workspace};
use crate::render::{changed_text, print_json_pretty, quote};
use crate::task_render::{
    print_task_dependency_summary, task_dependency_summary_json, task_epic_link_json,
    task_line_json_item,
};
use crate::workspaces::Workspace;

pub(crate) async fn cmd_dep(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: DepCommand,
) -> Result<()> {
    match args.command {
        DepSubcommand::Add(args) => {
            let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
            let depends_on =
                resolve_task_ref_in_workspace(conn, workspace, &args.depends_on_ref).await?;
            let outcome = add_task_dependency(conn, workspace, &task.id, &depends_on.id).await?;
            let display_refs = DisplayRefContext::for_workspace(conn, &workspace.id).await?;
            println!(
                "dependency-added {} changed={} depends_on={}",
                display_refs.display_ref(&outcome.task),
                changed_text(outcome.changed),
                display_refs.display_ref(&outcome.depends_on),
            );
        }
        DepSubcommand::Remove(args) => {
            let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
            let depends_on =
                resolve_task_ref_in_workspace(conn, workspace, &args.depends_on_ref).await?;
            let outcome = remove_task_dependency(conn, workspace, &task.id, &depends_on.id).await?;
            let display_refs = DisplayRefContext::for_workspace(conn, &workspace.id).await?;
            println!(
                "dependency-removed {} changed={} depends_on={}",
                display_refs.display_ref(&outcome.task),
                changed_text(outcome.changed),
                display_refs.display_ref(&outcome.depends_on),
            );
        }
        DepSubcommand::List(args) => {
            let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
            let summary =
                query::task_dependency_summary(conn, &task.workspace_id, &task.id).await?;
            if args.json {
                let json = task_dependency_summary_json(&summary);
                print_json_pretty(&json)?;
            } else {
                print_task_dependency_summary(&summary);
            }
        }
    }
    Ok(())
}

pub(crate) async fn cmd_epic(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: EpicCommand,
) -> Result<()> {
    match args.command {
        EpicSubcommand::Add(args) => {
            let child = resolve_task_ref_in_workspace(conn, workspace, &args.child_ref).await?;
            let epic = resolve_task_ref_in_workspace(conn, workspace, &args.epic_ref).await?;
            let outcome = add_task_to_epic(conn, workspace, &child.id, &epic.id).await?;
            let display_refs = DisplayRefContext::for_workspace(conn, &workspace.id).await?;
            println!(
                "epic-added {} changed={} epic={}",
                display_refs.display_ref(&outcome.child),
                changed_text(outcome.changed),
                display_refs.display_ref(&outcome.epic),
            );
        }
        EpicSubcommand::Remove(args) => {
            let child = resolve_task_ref_in_workspace(conn, workspace, &args.child_ref).await?;
            let epic = resolve_task_ref_in_workspace(conn, workspace, &args.epic_ref).await?;
            let outcome = remove_task_from_epic(conn, workspace, &child.id, &epic.id).await?;
            let display_refs = DisplayRefContext::for_workspace(conn, &workspace.id).await?;
            println!(
                "epic-removed {} changed={} epic={}",
                display_refs.display_ref(&outcome.child),
                changed_text(outcome.changed),
                display_refs.display_ref(&outcome.epic),
            );
        }
        EpicSubcommand::List(args) => {
            let epic = resolve_task_ref_in_workspace(conn, workspace, &args.epic_ref).await?;
            let mut items = query::list_task_items_in_workspace(
                conn,
                &workspace.id,
                TaskFilters {
                    task_ids: vec![epic.id.clone()],
                    include_deleted: true,
                    ..TaskFilters::default()
                },
                TaskQueryMode::Flat,
                TaskSort::Created,
                SortDirection::Asc,
            )
            .await?;
            let Some(item) = items.pop() else {
                bail!("error task-not-found ref={}", args.epic_ref);
            };
            if args.json {
                print_json_pretty(&json!({
                    "epic": task_line_json_item(&item),
                    "children": item.epic_children.iter().map(task_epic_link_json).collect::<Vec<_>>()
                }))?;
            } else {
                println!(
                    "epic {} title={} children={}",
                    item.display_ref,
                    quote(&item.task.title),
                    item.epic_children.len()
                );
                for child in &item.epic_children {
                    println!(
                        "- {} status={} priority={} title={}",
                        child.display_ref,
                        child.status,
                        child.priority,
                        quote(&child.title)
                    );
                }
            }
        }
    }
    Ok(())
}
