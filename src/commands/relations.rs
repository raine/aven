use anyhow::{Result, bail};
use aven_core::db::Database;
use serde_json::json;

use crate::cli::{DepCommand, DepSubcommand, EpicCommand, EpicSubcommand};
use crate::query::{SortDirection, TaskFilters, TaskQueryMode, TaskSort};
use crate::render::{changed_text, print_json_pretty, quote};
use crate::task_render::{
    print_task_dependency_summary, task_dependency_summary_json, task_epic_link_json,
    task_line_json_item,
};
use crate::workspaces::Workspace;

pub(crate) async fn cmd_dep(
    database: &Database,
    workspace: &Workspace,
    args: DepCommand,
) -> Result<()> {
    match args.command {
        DepSubcommand::Add(args) => {
            let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
            let depends_on = database
                .resolve_task_ref(workspace, &args.depends_on_ref)
                .await?;
            let outcome = database
                .add_task_dependency(workspace, &task.id, &depends_on.id)
                .await?;
            let display_refs = database.display_ref_context(&workspace.id).await?;
            println!(
                "dependency-added {} changed={} depends_on={}",
                display_refs.display_ref(&outcome.task),
                changed_text(outcome.changed),
                display_refs.display_ref(&outcome.depends_on),
            );
        }
        DepSubcommand::Remove(args) => {
            let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
            let depends_on = database
                .resolve_task_ref(workspace, &args.depends_on_ref)
                .await?;
            let outcome = database
                .remove_task_dependency(workspace, &task.id, &depends_on.id)
                .await?;
            let display_refs = database.display_ref_context(&workspace.id).await?;
            println!(
                "dependency-removed {} changed={} depends_on={}",
                display_refs.display_ref(&outcome.task),
                changed_text(outcome.changed),
                display_refs.display_ref(&outcome.depends_on),
            );
        }
        DepSubcommand::List(args) => {
            let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
            let summary = database
                .task_dependency_summary(&task.workspace_id, &task.id)
                .await?;
            if args.json {
                print_json_pretty(&task_dependency_summary_json(&summary))?;
            } else {
                print_task_dependency_summary(&summary);
            }
        }
    }
    Ok(())
}

pub(crate) async fn cmd_epic(
    database: &Database,
    workspace: &Workspace,
    args: EpicCommand,
) -> Result<()> {
    match args.command {
        EpicSubcommand::Add(args) => {
            let child = database
                .resolve_task_ref(workspace, &args.child_ref)
                .await?;
            let epic = database.resolve_task_ref(workspace, &args.epic_ref).await?;
            let outcome = database
                .add_task_to_epic(workspace, &child.id, &epic.id)
                .await?;
            let display_refs = database.display_ref_context(&workspace.id).await?;
            println!(
                "epic-added {} changed={} epic={}",
                display_refs.display_ref(&outcome.child),
                changed_text(outcome.changed),
                display_refs.display_ref(&outcome.epic),
            );
        }
        EpicSubcommand::Remove(args) => {
            let child = database
                .resolve_task_ref(workspace, &args.child_ref)
                .await?;
            let epic = database.resolve_task_ref(workspace, &args.epic_ref).await?;
            let outcome = database
                .remove_task_from_epic(workspace, &child.id, &epic.id)
                .await?;
            let display_refs = database.display_ref_context(&workspace.id).await?;
            println!(
                "epic-removed {} changed={} epic={}",
                display_refs.display_ref(&outcome.child),
                changed_text(outcome.changed),
                display_refs.display_ref(&outcome.epic),
            );
        }
        EpicSubcommand::List(args) => {
            let epic = database.resolve_task_ref(workspace, &args.epic_ref).await?;
            let mut items = database
                .list_task_items(
                    &workspace.id,
                    TaskFilters {
                        task_ids: crate::query::TaskIdFilter::Only(vec![epic.id.clone()]),
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
