use crate::ids::WorkspaceId;
mod config;
mod conflicts;
mod context;
mod data_safety;
mod doctor;
mod prime;
mod projects;
mod self_update;
mod skill;
mod workspaces;

use std::fs;
use std::path::Path;

use std::collections::HashSet;

use anyhow::{Result, bail};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use sqlx::SqliteConnection;

use crate::sync::sync_server_url_is_valid;
use doctor::{DoctorRenderer, DoctorReport, workspace_counts};

pub(crate) use self::config::cmd_config;
pub(crate) use self::conflicts::cmd_conflict;
pub(crate) use self::context::cmd_context;
pub(crate) use self::data_safety::{
    cmd_backup, cmd_backup_restore, cmd_export, cmd_import, database_integrity_report,
    ensure_integrity_ok,
};
pub(crate) use self::prime::run as cmd_prime;
pub(crate) use self::projects::cmd_project;
pub(crate) use self::self_update::run as cmd_self_update;
pub(crate) use self::skill::install as cmd_skill_install;
pub(crate) use self::workspaces::cmd_workspace;
use crate::choices::{TaskPriority, TaskStatus};
use crate::cli::{
    AddArgs, BulkUpdateArgs, DepCommand, DepSubcommand, EpicCommand, EpicSubcommand,
    InternalNaturalAddArgs, LabelCommand, LabelSubcommand, ListArgs, NoteArgs, NoteDeleteArgs,
    RefArgs, SearchArgs, ShowArgs, TaskEditArgs, TaskSearchArgs, TextCommand, TextSubcommand,
};
use crate::config::{self as app_config, AppConfig};
use crate::db::{conflict_exists, get_meta};
use crate::input::{read_optional_text, read_required_text};
use crate::labels::{list_labels_in_workspace, resolve_labels_in_workspace};
use crate::operations::{
    TaskDraft, TaskUpdate, add_note, add_task_dependency, add_task_to_epic, create_label_operation,
    create_task, delete_label_operation, delete_note, remove_task_dependency,
    remove_task_from_epic, set_task_deleted, update_task,
};
use crate::projects::resolve_existing_project_in_workspace;
use crate::query::{
    self, SortDirection, TaskAvailabilityFilter, TaskFilters, TaskQueryMode, TaskSearchQuery,
    TaskSearchResult, TaskSort,
};
use crate::refs::{display_ref, display_suffix_in_workspace, resolve_task_ref_in_workspace};
use crate::render::{
    KvLine, changed_text, print_json_pretty, print_multiline_block, print_text_diff, quote,
};
use crate::task_fields::TaskField;
use crate::task_render::{
    TaskConflictJson, TaskFullJson, TaskNoteJson, conflict_display_value, print_full_task_detail,
    print_task_dependency_summary, print_task_line_item, task_dependency_summary_json,
    task_epic_link_json, task_line_json_item,
};
use crate::types::Task;
use crate::workspaces::{Workspace, resolve_active_workspace, workspace_for_id};

pub(crate) async fn cmd_add(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    config: &AppConfig,
    args: AddArgs,
) -> Result<()> {
    validate_priority(&args.priority)?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?
    .unwrap_or_default();
    let draft = if args.natural {
        if !description.is_empty()
            || args.project.is_some()
            || args.priority != "none"
            || !args.label.is_empty()
            || args.available_at.is_some()
            || args.due.is_some()
        {
            bail!(
                "error natural-add-exclusive hint=\"use plain add flags or --natural, not both\""
            );
        }
        crate::task_intake::parse_task_intake(
            conn,
            &config.agent.task_intake,
            &args.title,
            workspace,
        )
        .await?
    } else {
        TaskDraft {
            title: args.title,
            description,
            project: args.project,
            status: "inbox".to_string(),
            priority: args.priority,
            labels: args.label,
            available_at: args
                .available_at
                .as_deref()
                .map(crate::time_input::parse_available_at_input)
                .transpose()?
                .unwrap_or_default(),
            due_on: args
                .due
                .as_deref()
                .map(crate::time_input::parse_due_on_input)
                .transpose()?
                .unwrap_or_default(),
            is_epic: args.epic,
        }
    };
    let outcome = create_task(conn, workspace, draft).await?;
    let task = outcome.task;
    println!(
        "created {} ref={} project={} status={} priority={}{}{} title={}",
        display_ref(conn, &task).await?,
        display_suffix_in_workspace(conn, workspace, &task.id).await?,
        task.project_key,
        task.status,
        task.priority,
        available_at_display(&task.available_at),
        due_on_display(&task.due_on),
        quote(&task.title)
    );
    Ok(())
}

pub(crate) async fn cmd_internal_natural_add(
    conn: &mut SqliteConnection,
    config: &AppConfig,
    args: InternalNaturalAddArgs,
) -> Result<()> {
    let workspace = workspace_for_id(conn, &args.workspace_id).await?;
    let outcome = async {
        let draft = crate::task_intake::parse_task_intake_with_project(
            conn,
            &config.agent.task_intake,
            &args.input,
            &workspace,
            args.project.as_deref(),
        )
        .await?;
        let outcome = create_task(conn, &workspace, draft).await?;
        if args.tui_undo {
            let task_id = outcome.task.id.clone();
            let snapshot = crate::undo::task_snapshot(conn, &args.workspace_id, &task_id).await?;
            crate::undo::record_tui_undo(
                conn,
                &args.workspace_id,
                &format!("task {task_id}"),
                crate::undo::UndoPayload {
                    commands: vec![crate::undo::UndoCommand::DeleteCreatedTask {
                        task_id,
                        create_change_id: outcome.create_change_id.clone(),
                        expected: snapshot,
                    }],
                },
            )
            .await?;
        }
        Ok(outcome)
    }
    .await;
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::error!(
                workspace_id = %args.workspace_id,
                has_project_context = args.project.is_some(),
                error = %error,
                "internal natural-add failed"
            );
            return Err(error);
        }
    };
    let task = outcome.task;
    tracing::info!(
        workspace_id = %args.workspace_id,
        task_id = %task.id,
        project = %task.project_key,
        "created task from internal natural-add"
    );
    println!(
        "created {} ref={} project={} status={} priority={}{}{} title={}",
        display_ref(conn, &task).await?,
        display_suffix_in_workspace(conn, &workspace, &task.id).await?,
        task.project_key,
        task.status,
        task.priority,
        available_at_display(&task.available_at),
        due_on_display(&task.due_on),
        quote(&task.title)
    );
    crate::daemon::wake_if_enabled(config);
    Ok(())
}

fn available_at_display(available_at: &str) -> String {
    if available_at.is_empty() {
        String::new()
    } else {
        format!(" available_at={available_at}")
    }
}

fn due_on_display(due_on: &str) -> String {
    if due_on.is_empty() {
        String::new()
    } else {
        format!(" due_on={due_on}")
    }
}

pub(crate) async fn cmd_show(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: ShowArgs,
) -> Result<()> {
    let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
    if args.full {
        let detail = query::task_detail(conn, &task).await?;
        if args.json {
            let mut conflict_items = Vec::with_capacity(detail.conflicts.len());
            for conflict in &detail.conflicts {
                let local_value = conflict_display_value(
                    conn,
                    &task.workspace_id,
                    &conflict.field,
                    &conflict.local_value,
                )
                .await?;
                let remote_value = conflict_display_value(
                    conn,
                    &task.workspace_id,
                    &conflict.field,
                    &conflict.remote_value,
                )
                .await?;
                conflict_items.push(TaskConflictJson {
                    field: conflict.field.clone(),
                    variant_a: conflict.variant_a.clone(),
                    local_value,
                    variant_b: conflict.variant_b.clone(),
                    remote_value,
                });
            }
            let full = TaskFullJson {
                task: task_line_json_item(&detail.item),
                project_prefix: task.project_prefix.clone(),
                description: task.description.clone(),
                dependencies: task_dependency_summary_json(&detail.dependencies),
                notes: detail
                    .notes
                    .iter()
                    .map(|note| TaskNoteJson {
                        body: note.body.clone(),
                        created_at: note.created_at.clone(),
                    })
                    .collect(),
                conflicts: conflict_items,
            };
            print_json_pretty(&full)?;
        } else {
            print_full_task_detail(conn, &detail).await?;
        }
    } else {
        let item = query::list_task_items_in_workspace(
            conn,
            &workspace.id,
            TaskFilters {
                task_ids: vec![task.id.clone()],
                ..TaskFilters::default().include_deleted(task.deleted)
            },
            TaskQueryMode::Flat,
            TaskSort::Updated,
            SortDirection::Desc,
        )
        .await?
        .into_iter()
        .next()
        .expect("task must exist after resolve");
        if args.json {
            print_json_pretty(&task_line_json_item(&item))?;
        } else {
            print_task_line_item(&item).await?;
        }
    }
    Ok(())
}

pub(crate) async fn cmd_list(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: ListArgs,
) -> Result<()> {
    if args.ready && args.blocked {
        bail!(
            "error list-dependency-filter-conflict hint=\"pass at most one of --ready or --blocked\""
        );
    }
    if (args.ready || args.blocked) && (args.all || args.deleted) {
        bail!(
            "error list-dependency-filter-all-conflict hint=\"dependency filters only include open tasks\""
        );
    }
    if args.ready && args.epics {
        bail!("error list-epic-ready-conflict hint=\"pass at most one of --ready or --epics\"");
    }
    if args.upcoming && (args.ready || args.blocked || args.epics || args.deleted) {
        bail!(
            "error list-upcoming-filter-conflict hint=\"combine --upcoming only with project, status, priority, label, or --all\""
        );
    }
    let filters = list_task_filters(&args);
    let sort = if args.upcoming {
        TaskSort::AvailableAt
    } else if args.overdue {
        TaskSort::DueOn
    } else {
        TaskSort::Updated
    };
    let direction = if args.upcoming || args.overdue {
        SortDirection::Asc
    } else {
        SortDirection::Desc
    };
    let mut items = query::list_task_items_in_workspace(
        conn,
        &workspace.id,
        filters,
        TaskQueryMode::Flat,
        sort,
        direction,
    )
    .await?;
    if let Some(limit) = args.limit {
        items.truncate(limit);
    }
    if args.json {
        let items = items.iter().map(task_line_json_item).collect::<Vec<_>>();
        print_json_pretty(&items)?;
    } else {
        for item in items {
            print_task_line_item(&item).await?;
        }
    }
    Ok(())
}

#[derive(Serialize)]
struct SearchJsonItem {
    r#ref: String,
    id: String,
    title: String,
    project: String,
    status: String,
    priority: String,
    labels: Vec<String>,
    deleted: bool,
    score: i64,
    matched_field: query::SearchMatchedField,
    snippet: Option<String>,
}

pub(crate) async fn cmd_search(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: TaskSearchArgs,
) -> Result<()> {
    let text = args.query.join(" ");
    if text.trim().is_empty() {
        bail!("error search-query-required hint=\"pass one or more search terms\"");
    }
    let results = query::search_task_items_in_workspace(
        conn,
        &workspace.id,
        TaskSearchQuery {
            text,
            include_deleted: args.all,
            limit: args.limit,
        },
    )
    .await?;
    if args.json {
        let items = results.iter().map(search_json_item).collect::<Vec<_>>();
        print_json_pretty(&items)?;
    } else {
        for result in results {
            print_search_result(&result);
        }
    }
    Ok(())
}

fn print_search_result(result: &TaskSearchResult) {
    let item = &result.item;
    let labels = item.labels.join(",");
    let line = KvLine::new(item.display_ref.clone())
        .field("status", item.task.status)
        .field("priority", item.task.priority)
        .field("project", &item.task.project_key)
        .field("labels", &labels)
        .field("match", result.matched_field.as_str())
        .field("score", result.score)
        .optional("deleted", item.task.deleted.then(|| "yes".to_string()))
        .quoted("title", &item.task.title)
        .finish();
    println!("{line}");
    if let Some(snippet) = &result.snippet {
        println!("  snippet={}", quote(snippet));
    }
}

fn search_json_item(result: &TaskSearchResult) -> SearchJsonItem {
    SearchJsonItem {
        r#ref: result.item.display_ref.clone(),
        id: result.item.task.id.clone(),
        title: result.item.task.title.clone(),
        project: result.item.task.project_key.clone(),
        status: result.item.task.status.as_str().to_string(),
        priority: result.item.task.priority.as_str().to_string(),
        labels: result.item.labels.clone(),
        deleted: result.item.task.deleted,
        score: result.score,
        matched_field: result.matched_field,
        snippet: result.snippet.clone(),
    }
}

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
            println!(
                "dependency-added {} changed={} depends_on={}",
                display_ref(conn, &outcome.task).await?,
                changed_text(outcome.changed),
                display_ref(conn, &outcome.depends_on).await?,
            );
        }
        DepSubcommand::Remove(args) => {
            let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
            let depends_on =
                resolve_task_ref_in_workspace(conn, workspace, &args.depends_on_ref).await?;
            let outcome = remove_task_dependency(conn, workspace, &task.id, &depends_on.id).await?;
            println!(
                "dependency-removed {} changed={} depends_on={}",
                display_ref(conn, &outcome.task).await?,
                changed_text(outcome.changed),
                display_ref(conn, &outcome.depends_on).await?,
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
            println!(
                "epic-added {} changed={} epic={}",
                display_ref(conn, &outcome.child).await?,
                changed_text(outcome.changed),
                display_ref(conn, &outcome.epic).await?,
            );
        }
        EpicSubcommand::Remove(args) => {
            let child = resolve_task_ref_in_workspace(conn, workspace, &args.child_ref).await?;
            let epic = resolve_task_ref_in_workspace(conn, workspace, &args.epic_ref).await?;
            let outcome = remove_task_from_epic(conn, workspace, &child.id, &epic.id).await?;
            println!(
                "epic-removed {} changed={} epic={}",
                display_ref(conn, &outcome.child).await?,
                changed_text(outcome.changed),
                display_ref(conn, &outcome.epic).await?,
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
    let items = query::list_task_items_in_workspace(
        conn,
        &workspace.id,
        filters,
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
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
        print_changed_bulk_update(conn, &outcome.task).await?;
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

fn validate_status(status: &str) -> Result<()> {
    TaskStatus::parse(status).map(|_| ())
}

fn validate_priority(priority: &str) -> Result<()> {
    TaskPriority::parse(priority).map(|_| ())
}

fn validate_optional_status(status: Option<&str>) -> Result<()> {
    if let Some(status) = status {
        validate_status(status)?;
    }
    Ok(())
}

fn validate_optional_priority(priority: Option<&str>) -> Result<()> {
    if let Some(priority) = priority {
        validate_priority(priority)?;
    }
    Ok(())
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

async fn print_changed_bulk_update(conn: &mut SqliteConnection, task: &Task) -> Result<()> {
    let line = KvLine::new(format!("bulk-updated {}", display_ref(conn, task).await?))
        .field("changed", changed_text(true))
        .field("status", task.status)
        .field("priority", task.priority)
        .quoted("title", &task.title)
        .finish();
    println!("{line}");
    Ok(())
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
    task_id: &str,
    field: &str,
) -> Result<()> {
    if conflict_exists(conn, workspace_id, task_id, field).await? {
        bail!("error bulk-update-conflicted-field ref={display_ref} field={field}");
    }
    Ok(())
}

fn list_task_filters(args: &ListArgs) -> TaskFilters {
    let terminal_status = matches!(args.status.as_deref(), Some("done" | "canceled"));
    let availability = if args.upcoming {
        TaskAvailabilityFilter::Upcoming
    } else if args.deleted || terminal_status {
        TaskAvailabilityFilter::All
    } else {
        TaskAvailabilityFilter::Available
    };
    TaskFilters {
        ready_only: args.ready,
        blocked_only: args.blocked,
        epics_only: args.epics,
        exclude_epics: args.ready,
        overdue_only: args.overdue,
        availability,
        label: args.label.clone(),
        ..TaskFilters::default()
            .with_project(args.project.clone())
            .with_status(args.status.clone())
            .with_priority(args.priority.clone())
            .include_deleted(args.all || args.deleted)
            .deleted_only(args.deleted)
    }
}

fn parse_epic_switch(value: Option<String>) -> Result<Option<bool>> {
    value
        .map(|value| match value.as_str() {
            "on" | "true" | "1" => Ok(true),
            "off" | "false" | "0" => Ok(false),
            _ => bail!("error invalid-epic value={value}"),
        })
        .transpose()
}

pub(crate) async fn cmd_edit(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: TaskEditArgs,
) -> Result<()> {
    let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?;
    validate_optional_status(args.status.as_deref())?;
    validate_optional_priority(args.priority.as_deref())?;
    if args.available_at.is_some() && args.clear_available_at {
        bail!(
            "error available-at-conflict hint=\"use --available-at or --clear-available-at, not both\""
        );
    }
    let available_at = if args.clear_available_at {
        Some(String::new())
    } else {
        args.available_at
            .as_deref()
            .map(crate::time_input::parse_available_at_input)
            .transpose()?
    };
    let due_on = if args.clear_due {
        if args.due.is_some() {
            bail!("error due-conflict hint=\"use --due or --clear-due, not both\"");
        }
        Some(String::new())
    } else {
        args.due
            .as_deref()
            .map(crate::time_input::parse_due_on_input)
            .transpose()?
    };
    let is_epic = parse_epic_switch(args.epic)?;
    let outcome = update_task(
        conn,
        workspace,
        &task.id,
        TaskUpdate {
            title: args.title,
            description,
            project: args.project,
            status: args.status,
            priority: args.priority,
            available_at,
            due_on,
            is_epic,
            add_labels: args.label,
            remove_labels: args.remove_label,
        },
    )
    .await?;
    let task = outcome.task;
    println!(
        "updated {} changed={} status={} priority={}{}{} title={}",
        display_ref(conn, &task).await?,
        changed_text(outcome.changed),
        task.status,
        task.priority,
        available_at_display(&task.available_at),
        due_on_display(&task.due_on),
        quote(&task.title)
    );
    Ok(())
}

pub(crate) async fn cmd_note(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: NoteArgs,
) -> Result<()> {
    let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
    let body = read_required_text(args.text, args.file.as_deref(), args.stdin, "note")?;
    let outcome = add_note(conn, workspace, &task.id, body).await?;
    println!(
        "noted {} note={}",
        display_ref(conn, &task).await?,
        outcome.note_id
    );
    Ok(())
}

pub(crate) async fn cmd_note_delete(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: NoteDeleteArgs,
) -> Result<()> {
    let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
    let outcome = delete_note(conn, workspace, &task.id, &args.note_id).await?;
    println!(
        "deleted-note {} note={} changed={}",
        display_ref(conn, &task).await?,
        outcome.note_id,
        changed_text(outcome.changed),
    );
    Ok(())
}

fn ensure_description_field(field: &str) -> Result<TaskField> {
    match TaskField::parse(field) {
        Some(TaskField::Description) => Ok(TaskField::Description),
        Some(_) | None => {
            bail!("error unsupported-text-field field={field} hint=\"supported: description\"")
        }
    }
}

fn sha256_hex(value: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

pub(crate) async fn cmd_text(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: TextCommand,
) -> Result<()> {
    match args.command {
        TextSubcommand::Get(args) => {
            ensure_description_field(&args.field)?;
            let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
            let value = TaskField::Description.current_value(&task);
            let hash = sha256_hex(&value);
            let task_ref = display_ref(conn, &task).await?;
            if let Some(path) = args.output {
                fs::write(&path, value.as_bytes())?;
                println!(
                    "exported ref={task_ref} field=description sha256={hash} path={}",
                    quote(&path.display().to_string())
                );
            } else if args.raw {
                print!("{value}");
            } else {
                println!("ref={task_ref} field=description sha256={hash}");
                print_multiline_block("description", &value);
            }
        }
        TextSubcommand::Diff(args) => {
            ensure_description_field(&args.field)?;
            let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
            let current = TaskField::Description.current_value(&task);
            let candidate = fs::read_to_string(&args.file)?;
            print_text_diff("current", &current, "candidate", &candidate);
        }
        TextSubcommand::Set(args) => {
            ensure_description_field(&args.field)?;
            let value = read_required_text(None, args.file.as_deref(), args.stdin, "text")?;
            let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
            let current = TaskField::Description.current_value(&task);
            let actual = sha256_hex(&current);
            if actual != args.if_sha256 {
                bail!(
                    "error text-hash-mismatch field=description expected={} actual={}",
                    args.if_sha256,
                    actual
                );
            }
            let outcome = update_task(
                conn,
                workspace,
                &task.id,
                TaskUpdate {
                    description: Some(value),
                    ..Default::default()
                },
            )
            .await?;
            println!(
                "updated {} field=description sha256={}",
                display_ref(conn, &outcome.task).await?,
                sha256_hex(&outcome.task.description)
            );
        }
    }
    Ok(())
}

pub(crate) async fn cmd_labels(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: SearchArgs,
) -> Result<()> {
    let mut labels = list_labels_in_workspace(conn, &workspace.id, args.search.as_deref()).await?;
    if let Some(limit) = args.limit {
        labels.truncate(limit);
    }
    if args.json {
        print_json_pretty(&labels)?;
    } else {
        for label in labels {
            println!("{label}");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_label(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: LabelCommand,
) -> Result<()> {
    match args.command {
        LabelSubcommand::Create { name } => {
            let outcome = create_label_operation(conn, workspace, &name).await?;
            println!("created-label {}", outcome.name);
        }
        LabelSubcommand::Delete { name } => {
            let outcome = delete_label_operation(conn, workspace, &name).await?;
            println!(
                "deleted-label {} changed={}",
                outcome.name,
                changed_text(outcome.changed),
            );
        }
        LabelSubcommand::List(args) => cmd_labels(conn, workspace, args).await?,
    }
    Ok(())
}

pub(crate) async fn cmd_delete_restore(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: RefArgs,
    delete: bool,
) -> Result<()> {
    let task = resolve_task_ref_in_workspace(conn, workspace, &args.task_ref).await?;
    let outcome = set_task_deleted(conn, workspace, &task.id, delete).await?;
    let task = outcome.task;
    if delete {
        println!("deleted {}", display_ref(conn, &task).await?);
    } else {
        println!("restored {}", display_ref(conn, &task).await?);
    }
    Ok(())
}

pub(crate) async fn cmd_skill() -> Result<()> {
    print!("{}", include_str!("skill.md"));
    Ok(())
}

fn format_optional_i64(value: Option<i64>) -> String {
    value
        .map(|value| value.to_string())
        .unwrap_or_else(|| "none".to_string())
}

pub(crate) async fn cmd_doctor(
    conn: &mut SqliteConnection,
    config: &AppConfig,
    db_path: &Path,
    db_flag_set: bool,
    workspace_flag: Option<&str>,
    integrity: bool,
    json: bool,
) -> Result<()> {
    let config_file = app_config::config_file_path();
    let db_source = if db_flag_set {
        "--db"
    } else if std::env::var_os("AVEN_DB").is_some() {
        "AVEN_DB"
    } else if config.local.db_path.is_some() {
        "config local.db_path"
    } else {
        "default"
    };
    let client_id = get_meta(conn, "client_id").await?;
    let sync_cursor = get_meta(conn, "sync_cursor").await?;
    let local_seq = get_meta(conn, "local_seq").await?;
    let pinned_server = get_meta(conn, "sync_server_url").await?;
    let cwd = std::env::current_dir()?;
    let workspace = resolve_active_workspace(conn, workspace_flag, config, &cwd).await;
    let counts = match &workspace {
        Ok(workspace) => Some(workspace_counts(conn, &workspace.id).await?),
        Err(_) => None,
    };
    let sync_history = query::sync_history_stats(conn).await?;
    let unresolved_conflicts: i64 =
        sqlx::query_scalar("SELECT count(*) FROM conflicts WHERE resolved = 0")
            .fetch_one(&mut *conn)
            .await?;
    let sync_server = app_config::resolve_sync_server(None, config);
    let wake_addr = config.wake_addr();

    let mut report = DoctorReport::new();
    let config_section = report.section("Configuration");
    match config_file {
        Ok(path) if path.exists() => {
            config_section.check("config file", true, path.display().to_string());
        }
        Ok(path) => {
            config_section.info(
                "config file",
                format!("{} (using defaults)", path.display()),
            );
        }
        Err(error) => {
            config_section.check("config file", false, format!("{error:#}"));
        }
    }
    config_section.info("database source", db_source);
    config_section.info("database path", db_path.display().to_string());

    let database_section = report.section("Database");
    database_section.check("sqlite", true, "opened successfully");
    database_section.check(
        "client id",
        client_id.is_some(),
        client_id.as_deref().unwrap_or("missing"),
    );
    database_section.info("sync cursor", sync_cursor.as_deref().unwrap_or("missing"));
    database_section.info("local sequence", local_seq.as_deref().unwrap_or("missing"));
    database_section.info("pinned server", pinned_server.as_deref().unwrap_or("none"));
    database_section.info("change rows", sync_history.total_change_rows.to_string());
    database_section.info(
        "pending changes",
        sync_history.pending_change_rows.to_string(),
    );
    database_section.info(
        "synced changes",
        sync_history.synced_change_rows.to_string(),
    );
    database_section.info(
        "min server_seq",
        format_optional_i64(sync_history.min_server_seq),
    );
    database_section.info(
        "max server_seq",
        format_optional_i64(sync_history.max_server_seq),
    );
    database_section.info("payload bytes", sync_history.payload_bytes.to_string());
    database_section.info("conflicts", unresolved_conflicts.to_string());

    let workspace_section = report.section("Workspace");
    match workspace {
        Ok(workspace) => {
            workspace_section.check(
                "active workspace",
                true,
                format!("{} ({})", workspace.name, workspace.key),
            );
            if let Some((visible_count, all_count)) = counts {
                workspace_section.info(
                    "tasks",
                    format!("{visible_count} visible, {all_count} total"),
                );
            }
        }
        Err(error) => {
            workspace_section.check("active workspace", false, format!("{error:#}"));
        }
    }

    let sync_section = report.section("Sync");
    sync_section.info("enabled", if config.sync.enabled { "yes" } else { "no" });
    match sync_server {
        Ok(server) => {
            sync_section.check("server", sync_server_url_is_valid(&server), &server);
            if let Some(pinned) = pinned_server.as_deref() {
                let normalized = server.trim_end_matches('/');
                sync_section.check(
                    "server match",
                    pinned == normalized,
                    format!("pinned={pinned} configured={normalized}"),
                );
            }
        }
        Err(error) => {
            if config.sync.enabled {
                sync_section.check("server", false, format!("{error:#}"));
            } else {
                sync_section.info("server", "not configured");
            }
        }
    }
    match config.sync.server_url.as_deref() {
        Some(server) => {
            sync_section.check("daemon server", sync_server_url_is_valid(server), server)
        }
        None if config.sync.enabled => sync_section.check("daemon server", false, "not configured"),
        None => sync_section.info("daemon server", "not configured"),
    }
    sync_section.info(
        "auth token",
        if config.sync_auth_token().is_some() {
            "configured"
        } else {
            "not configured"
        },
    );
    sync_section.info(
        "interval",
        format!("{} seconds", config.sync_interval_seconds()),
    );
    match wake_addr {
        Ok(addr) => sync_section.check("daemon wake", true, addr.to_string()),
        Err(error) => sync_section.check("daemon wake", false, format!("{error:#}")),
    }

    let daemon_status = crate::daemon::status_snapshot()?;
    let daemon_section = report.section("Daemon");
    daemon_section.info(
        "installed",
        if daemon_status.installed { "yes" } else { "no" },
    );
    match daemon_status.loaded {
        Some(loaded) => daemon_section.check("loaded", loaded, if loaded { "yes" } else { "no" }),
        None => daemon_section.info("loaded", "unknown"),
    }
    daemon_section.info("plist", daemon_status.plist_path.display().to_string());
    daemon_section.info(
        "program",
        daemon_status
            .program
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "missing".to_string()),
    );
    daemon_section.info(
        "current exe",
        daemon_status.current_executable.display().to_string(),
    );
    match daemon_status.program_matches_current {
        Some(matches) => {
            daemon_section.check("program match", matches, if matches { "yes" } else { "no" })
        }
        None => daemon_section.info("program match", "unknown"),
    }

    if integrity {
        let integrity_report = database_integrity_report(conn).await?;
        let integrity_section = report.section("Integrity");
        integrity_section.check(
            "quick check",
            integrity_report.quick_check_ok,
            &integrity_report.quick_check_value,
        );
        for check in &integrity_report.checks {
            integrity_section.check(check.label, check.ok, &check.value);
        }
        if let Err(error) = ensure_integrity_ok(&integrity_report) {
            integrity_section.check("result", false, format!("{error:#}"));
        }
    }

    if json {
        print_json_pretty(&report)?;
    } else {
        DoctorRenderer::auto().print(&report);
    }
    Ok(())
}
