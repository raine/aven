use std::collections::{HashMap, HashSet};

use anyhow::{Result, bail};
use aven_core::db::Database;
use aven_core::metadata::TaskMetadataInput;

use super::validation::{validate_optional_priority, validate_optional_status};
use crate::cli::BulkUpdateArgs;
use crate::ids::{MetadataFieldId, WorkspaceId};
use crate::operations::TaskUpdate;
use crate::query::{
    self, SortDirection, TaskAvailabilityFilter, TaskFilters, TaskQueryMode, TaskSort,
};
use crate::refs::DisplayRefContext;
use crate::render::{KvLine, changed_text};
use crate::types::Task;
use crate::workspaces::Workspace;

pub(crate) async fn cmd_bulk_update(
    database: &Database,
    workspace: &Workspace,
    args: BulkUpdateArgs,
) -> Result<()> {
    ensure_bulk_update_has_selector(&args)?;
    ensure_bulk_update_has_mutation(&args)?;
    validate_bulk_update_args(&args)?;

    let workspace_id = workspace.id.clone();
    let labels = resolve_bulk_label_mutations(database, &workspace_id, &args).await?;
    ensure_disjoint_labels(&labels.add, &labels.remove)?;
    let set_metadata = super::tasks::parse_metadata_args(&args.metadata)?;
    ensure_disjoint_metadata(&set_metadata, &args.remove_metadata)?;
    let mutations = BulkResolvedMutations {
        labels,
        set_metadata,
        remove_metadata: args.remove_metadata.clone(),
        set_project_key: resolve_bulk_project_mutation(database, &workspace_id, &args).await?,
    };

    let filters = bulk_update_filters(&args);
    let items = database
        .list_bulk_update_task_items(
            &workspace.id,
            filters,
            TaskQueryMode::Flat,
            TaskSort::Updated,
            SortDirection::Desc,
        )
        .await?;
    let matched = items.len();
    let planned = plan_bulk_updates(database, &workspace_id, items, &args, &mutations).await?;

    let would_change = planned.iter().filter(|item| item.will_change).count();
    let outcomes = if args.dry_run {
        Vec::new()
    } else {
        let updates = planned
            .iter()
            .filter(|item| item.will_change)
            .map(|item| (item.item.task.id.clone(), item.update.clone()))
            .collect();
        database.update_tasks(workspace, updates).await?
    };
    if !args.dry_run && outcomes.len() != would_change {
        bail!(
            "error internal-bulk-update-outcome-mismatch expected={would_change} actual={}",
            outcomes.len()
        );
    }
    let display_refs = if outcomes.is_empty() {
        None
    } else {
        Some(database.display_ref_context(&workspace.id).await?)
    };
    let mut outcomes = outcomes.into_iter();
    let mut changed = 0;
    let mut unchanged = 0;
    for planned in planned {
        let item = planned.item;
        if args.dry_run {
            print_dry_run_bulk_update(&item, planned.will_change);
            continue;
        }
        if !planned.will_change {
            unchanged += 1;
            print_unchanged_bulk_update(&item);
            continue;
        }
        let outcome = outcomes
            .next()
            .expect("batch update returns one outcome per changing task");
        changed += 1;
        print_changed_bulk_update(
            display_refs
                .as_ref()
                .expect("changed bulk updates load display refs"),
            &outcome.task,
        );
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
        || !args.metadata.is_empty()
        || !args.remove_metadata.is_empty()
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

struct BulkResolvedMutations {
    labels: BulkLabelMutations,
    set_metadata: Vec<TaskMetadataInput>,
    remove_metadata: Vec<String>,
    set_project_key: Option<String>,
}

struct PlannedBulkUpdate {
    item: query::TaskListItem,
    update: TaskUpdate,
    will_change: bool,
}

async fn resolve_bulk_label_mutations(
    database: &Database,
    workspace_id: &WorkspaceId,
    args: &BulkUpdateArgs,
) -> Result<BulkLabelMutations> {
    let add = dedup_labels(database.resolve_labels(workspace_id, &args.label).await?);
    let remove = dedup_labels(
        database
            .resolve_labels(workspace_id, &args.remove_label)
            .await?,
    );
    Ok(BulkLabelMutations { add, remove })
}

async fn resolve_bulk_project_mutation(
    database: &Database,
    workspace_id: &WorkspaceId,
    args: &BulkUpdateArgs,
) -> Result<Option<String>> {
    if let Some(project) = args.set_project.as_deref() {
        return Ok(Some(
            database
                .resolve_existing_project(workspace_id, project)
                .await?
                .key,
        ));
    }
    Ok(None)
}

fn bulk_update_filters(args: &BulkUpdateArgs) -> TaskFilters {
    TaskFilters {
        label: args.filter_label.clone(),
        availability: TaskAvailabilityFilter::All,
        ..TaskFilters::default()
            .with_project(args.project.clone())
            .with_status(args.status.clone())
            .with_priority(args.priority.clone())
            .include_deleted(args.include_deleted)
    }
}

async fn plan_bulk_updates(
    database: &Database,
    workspace_id: &WorkspaceId,
    items: Vec<query::TaskListItem>,
    args: &BulkUpdateArgs,
    mutations: &BulkResolvedMutations,
) -> Result<Vec<PlannedBulkUpdate>> {
    let planned = items
        .into_iter()
        .map(|item| {
            let update = bulk_update_for_item(&item, args, mutations);
            let will_change = bulk_update_has_changes(&update);
            PlannedBulkUpdate {
                item,
                update,
                will_change,
            }
        })
        .collect::<Vec<_>>();
    let metadata_fields = resolve_bulk_metadata_fields(database, workspace_id, &planned).await?;
    let conflict_candidates = planned
        .iter()
        .filter(|planned| planned.will_change)
        .flat_map(|planned| {
            bulk_update_conflict_fields(&planned.update, &metadata_fields)
                .into_iter()
                .map(|field| (planned.item.task.id.clone(), field))
        })
        .collect::<Vec<_>>();
    let conflicts = database
        .unresolved_task_conflict_fields(workspace_id, &conflict_candidates)
        .await?;
    for planned in &planned {
        if planned.will_change {
            preflight_bulk_update_item(planned, &metadata_fields, &conflicts)?;
        }
    }
    Ok(planned)
}

async fn resolve_bulk_metadata_fields(
    database: &Database,
    workspace_id: &WorkspaceId,
    planned: &[PlannedBulkUpdate],
) -> Result<HashMap<String, MetadataFieldId>> {
    let mut fields = HashMap::new();
    let mut seen = HashSet::new();
    for planned in planned {
        for key in planned
            .update
            .set_metadata
            .iter()
            .map(|input| input.key.as_str())
            .chain(planned.update.remove_metadata.iter().map(String::as_str))
        {
            let key = aven_core::metadata::normalize_metadata_key(key)
                .expect("bulk metadata keys were validated");
            if !seen.insert(key.clone()) {
                continue;
            }
            if let Some(field) = database.find_metadata_field(workspace_id, &key).await? {
                fields.insert(key, field.id);
            }
        }
    }
    Ok(fields)
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

fn ensure_disjoint_metadata(set: &[TaskMetadataInput], remove: &[String]) -> Result<()> {
    let mut set_keys = HashSet::new();
    for input in set {
        let key = aven_core::metadata::normalize_metadata_key(&input.key)?;
        if !set_keys.insert(key) {
            bail!("error duplicate-metadata-key");
        }
    }
    let mut remove_keys = HashSet::new();
    for input in remove {
        let key = aven_core::metadata::normalize_metadata_key(input)?;
        if !remove_keys.insert(key.clone()) {
            bail!("error duplicate-metadata-key");
        }
        if set_keys.contains(&key) {
            bail!("error bulk-update-metadata-conflict key={key}");
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
    mutations: &BulkResolvedMutations,
) -> TaskUpdate {
    TaskUpdate {
        title: None,
        description: None,
        project: mutations
            .set_project_key
            .as_deref()
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
        cycle_priority: None,
        available_at: None,
        due_on: None,
        deleted: None,
        is_epic: None,
        add_labels: mutations
            .labels
            .add
            .iter()
            .filter(|label| !item.labels.contains(label))
            .cloned()
            .collect(),
        remove_labels: mutations
            .labels
            .remove
            .iter()
            .filter(|label| item.labels.contains(label))
            .cloned()
            .collect(),
        set_metadata: mutations
            .set_metadata
            .iter()
            .filter(|input| {
                let key = aven_core::metadata::normalize_metadata_key(&input.key)
                    .expect("bulk metadata keys were validated");
                item.metadata
                    .iter()
                    .find(|metadata| metadata.key == key)
                    .is_none_or(|metadata| metadata.value != input.value)
            })
            .cloned()
            .collect(),
        remove_metadata: mutations
            .remove_metadata
            .iter()
            .filter(|input| {
                let key = aven_core::metadata::normalize_metadata_key(input)
                    .expect("bulk metadata keys were validated");
                item.metadata.iter().any(|metadata| metadata.key == key)
            })
            .cloned()
            .collect(),
        label_selection: None,
        create_missing_labels: false,
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
        || !update.set_metadata.is_empty()
        || !update.remove_metadata.is_empty()
}

fn preflight_bulk_update_item(
    planned: &PlannedBulkUpdate,
    metadata_fields: &HashMap<String, MetadataFieldId>,
    conflicts: &HashMap<crate::ids::TaskId, HashSet<String>>,
) -> Result<()> {
    let item = &planned.item;
    for field in bulk_update_conflict_fields(&planned.update, metadata_fields) {
        ensure_bulk_field_clear(conflicts, &item.display_ref, &item.task.id, &field)?;
    }
    Ok(())
}

fn bulk_update_conflict_fields(
    update: &TaskUpdate,
    metadata_fields: &HashMap<String, MetadataFieldId>,
) -> Vec<String> {
    let mut fields = Vec::new();
    if update.status.is_some() {
        fields.push("status".to_string());
    }
    if update.priority.is_some() {
        fields.push("priority".to_string());
    }
    if update.project.is_some() {
        fields.push("project".to_string());
    }
    if !update.add_labels.is_empty() || !update.remove_labels.is_empty() {
        fields.push("labels".to_string());
    }
    for key in update
        .set_metadata
        .iter()
        .map(|input| input.key.as_str())
        .chain(update.remove_metadata.iter().map(String::as_str))
    {
        let key = aven_core::metadata::normalize_metadata_key(key)
            .expect("bulk metadata keys were validated");
        if let Some(field_id) = metadata_fields.get(&key) {
            fields.push(format!("metadata:{field_id}"));
        }
    }
    fields
}

fn ensure_bulk_field_clear(
    conflicts: &HashMap<crate::ids::TaskId, HashSet<String>>,
    display_ref: &str,
    task_id: &crate::ids::TaskId,
    field: &str,
) -> Result<()> {
    if conflicts
        .get(task_id)
        .is_some_and(|fields| fields.contains(field))
    {
        bail!("error bulk-update-conflicted-field ref={display_ref} field={field}");
    }
    Ok(())
}
