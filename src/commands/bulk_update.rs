use std::collections::HashSet;

use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use super::validation::{validate_optional_priority, validate_optional_status};
use crate::cli::BulkUpdateArgs;
use crate::db::conflict_exists;
use crate::ids::WorkspaceId;
use crate::labels::resolve_labels_in_workspace;
use crate::operations::{TaskUpdate, update_task};
use crate::projects::resolve_existing_project_in_workspace;
use crate::query::{
    self, SortDirection, TaskAvailabilityFilter, TaskFilters, TaskQueryMode, TaskSort,
};
use crate::refs::DisplayRefContext;
use crate::render::{KvLine, changed_text};
use crate::types::Task;
use crate::workspaces::Workspace;

pub(crate) async fn cmd_bulk_update(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: BulkUpdateArgs,
) -> Result<()> {
    ensure_bulk_update_has_selector(&args)?;
    ensure_bulk_update_has_mutation(&args)?;
    validate_bulk_update_args(&args)?;

    let workspace_id = workspace.id.clone();
    let labels = resolve_bulk_label_mutations(conn, &workspace_id, &args).await?;
    ensure_disjoint_labels(&labels.add, &labels.remove)?;
    let set_project_key = resolve_bulk_project_mutation(conn, &workspace_id, &args).await?;

    let filters = bulk_update_filters(&args);
    let display_refs = DisplayRefContext::for_workspace(conn, &workspace.id).await?;
    let items = query::list_task_items_with_display_refs(
        conn,
        &workspace.id,
        filters,
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
        &display_refs,
    )
    .await?;
    let matched = items.len();
    let planned = plan_bulk_updates(
        conn,
        &workspace_id,
        items,
        &args,
        &labels,
        set_project_key.as_deref(),
    )
    .await?;

    let would_change = planned.iter().filter(|item| item.will_change).count();
    let mut changed = 0;
    let mut unchanged = 0;
    for planned in planned {
        let item = planned.item;
        let update = planned.update;
        if args.dry_run {
            print_dry_run_bulk_update(&item, planned.will_change);
            continue;
        }
        if !planned.will_change {
            unchanged += 1;
            print_unchanged_bulk_update(&item);
            continue;
        }
        let outcome = update_task(conn, workspace, &item.task.id, update).await?;
        changed += 1;
        print_changed_bulk_update(&display_refs, &outcome.task);
    }
    if args.dry_run {
        unchanged = matched - would_change;
    }
    println!(
        "bulk-update-summary matched={matched} changed={changed} would_change={would_change} unchanged={unchanged} dry_run={}",
        if args.dry_run { "yes" } else { "no" }
    );
    Ok(())
}

fn ensure_bulk_update_has_selector(args: &BulkUpdateArgs) -> Result<()> {
    if args.project.is_some()
        || args.status.is_some()
        || args.priority.is_some()
        || args.filter_label.is_some()
        || args.all
    {
        return Ok(());
    }
    bail!("error bulk-update-requires-selector hint=\"add a filter or --all\"");
}

fn ensure_bulk_update_has_mutation(args: &BulkUpdateArgs) -> Result<()> {
    if args.set_status.is_some()
        || args.set_priority.is_some()
        || args.set_project.is_some()
        || !args.label.is_empty()
        || !args.remove_label.is_empty()
    {
        return Ok(());
    }
    bail!("error bulk-update-requires-mutation hint=\"add a mutation flag\"");
}

fn validate_bulk_update_args(args: &BulkUpdateArgs) -> Result<()> {
    validate_optional_status(args.status.as_deref())?;
    validate_optional_priority(args.priority.as_deref())?;
    validate_optional_status(args.set_status.as_deref())?;
    validate_optional_priority(args.set_priority.as_deref())?;
    Ok(())
}

struct BulkLabelMutations {
    add: Vec<String>,
    remove: Vec<String>,
}

struct PlannedBulkUpdate {
    item: query::TaskListItem,
    update: TaskUpdate,
    will_change: bool,
}

async fn resolve_bulk_label_mutations(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    args: &BulkUpdateArgs,
) -> Result<BulkLabelMutations> {
    let add = dedup_labels(resolve_labels_in_workspace(conn, workspace_id, &args.label).await?);
    let remove =
        dedup_labels(resolve_labels_in_workspace(conn, workspace_id, &args.remove_label).await?);
    Ok(BulkLabelMutations { add, remove })
}

async fn resolve_bulk_project_mutation(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    args: &BulkUpdateArgs,
) -> Result<Option<String>> {
    if let Some(project) = args.set_project.as_deref() {
        return Ok(Some(
            resolve_existing_project_in_workspace(conn, workspace_id, project)
                .await?
                .key,
        ));
    }
    Ok(None)
}

fn bulk_update_filters(args: &BulkUpdateArgs) -> TaskFilters {
    TaskFilters {
        label: args.filter_label.clone(),
        availability: TaskAvailabilityFilter::Available,
        ..TaskFilters::default()
            .with_project(args.project.clone())
            .with_status(args.status.clone())
            .with_priority(args.priority.clone())
            .include_deleted(args.include_deleted)
    }
}

async fn plan_bulk_updates(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    items: Vec<query::TaskListItem>,
    args: &BulkUpdateArgs,
    labels: &BulkLabelMutations,
    set_project_key: Option<&str>,
) -> Result<Vec<PlannedBulkUpdate>> {
    let mut planned = Vec::with_capacity(items.len());
    for item in items {
        let update =
            bulk_update_for_item(&item, args, &labels.add, &labels.remove, set_project_key);
        let will_change = bulk_update_has_changes(&update);
        preflight_bulk_update_item(conn, workspace_id, &item, &update).await?;
        planned.push(PlannedBulkUpdate {
            item,
            update,
            will_change,
        });
    }
    Ok(planned)
}

fn print_dry_run_bulk_update(item: &query::TaskListItem, will_change: bool) {
    let line = KvLine::new(format!("would-update {}", item.display_ref))
        .field("changed", changed_text(will_change))
        .field("status", item.task.status)
        .field("priority", item.task.priority)
        .field("labels", item.labels.join(","))
        .quoted("title", &item.task.title)
        .finish();
    println!("{line}");
}

fn print_unchanged_bulk_update(item: &query::TaskListItem) {
    let line = KvLine::new(format!("bulk-updated {}", item.display_ref))
        .field("changed", changed_text(false))
        .field("status", item.task.status)
        .field("priority", item.task.priority)
        .quoted("title", &item.task.title)
        .finish();
    println!("{line}");
}

fn print_changed_bulk_update(display_refs: &DisplayRefContext, task: &Task) {
    let line = KvLine::new(format!("bulk-updated {}", display_refs.display_ref(task)))
        .field("changed", changed_text(true))
        .field("status", task.status)
        .field("priority", task.priority)
        .quoted("title", &task.title)
        .finish();
    println!("{line}");
}

fn ensure_disjoint_labels(add_labels: &[String], remove_labels: &[String]) -> Result<()> {
    let add_labels = add_labels.iter().collect::<HashSet<_>>();
    for label in remove_labels {
        if add_labels.contains(label) {
            bail!("error bulk-update-label-conflict label={label}");
        }
    }
    Ok(())
}

fn dedup_labels(labels: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    labels
        .into_iter()
        .filter(|label| seen.insert(label.clone()))
        .collect()
}

fn bulk_update_for_item(
    item: &query::TaskListItem,
    args: &BulkUpdateArgs,
    add_labels: &[String],
    remove_labels: &[String],
    set_project_key: Option<&str>,
) -> TaskUpdate {
    TaskUpdate {
        title: None,
        description: None,
        project: set_project_key
            .filter(|project_key| *project_key != item.task.project_key)
            .map(str::to_string),
        status: args
            .set_status
            .as_deref()
            .filter(|status| *status != item.task.status.as_str())
            .map(str::to_string),
        priority: args
            .set_priority
            .as_deref()
            .filter(|priority| *priority != item.task.priority.as_str())
            .map(str::to_string),
        available_at: None,
        due_on: None,
        is_epic: None,
        add_labels: add_labels
            .iter()
            .filter(|label| !item.labels.contains(label))
            .cloned()
            .collect(),
        remove_labels: remove_labels
            .iter()
            .filter(|label| item.labels.contains(label))
            .cloned()
            .collect(),
    }
}

fn bulk_update_has_changes(update: &TaskUpdate) -> bool {
    update.title.is_some()
        || update.description.is_some()
        || update.project.is_some()
        || update.status.is_some()
        || update.priority.is_some()
        || !update.add_labels.is_empty()
        || !update.remove_labels.is_empty()
}

async fn preflight_bulk_update_item(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    item: &query::TaskListItem,
    update: &TaskUpdate,
) -> Result<()> {
    if update.status.is_some() {
        ensure_bulk_field_clear(
            conn,
            workspace_id,
            &item.display_ref,
            &item.task.id,
            "status",
        )
        .await?;
    }
    if update.priority.is_some() {
        ensure_bulk_field_clear(
            conn,
            workspace_id,
            &item.display_ref,
            &item.task.id,
            "priority",
        )
        .await?;
    }
    if update.project.is_some() {
        ensure_bulk_field_clear(
            conn,
            workspace_id,
            &item.display_ref,
            &item.task.id,
            "project",
        )
        .await?;
    }
    if !update.add_labels.is_empty() || !update.remove_labels.is_empty() {
        ensure_bulk_field_clear(
            conn,
            workspace_id,
            &item.display_ref,
            &item.task.id,
            "labels",
        )
        .await?;
    }
    Ok(())
}

async fn ensure_bulk_field_clear(
    conn: &mut SqliteConnection,
    workspace_id: &WorkspaceId,
    display_ref: &str,
    task_id: &crate::ids::TaskId,
    field: &str,
) -> Result<()> {
    if conflict_exists(conn, workspace_id, task_id, field).await? {
        bail!("error bulk-update-conflicted-field ref={display_ref} field={field}");
    }
    Ok(())
}
