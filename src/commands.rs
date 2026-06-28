mod config;
mod conflicts;
mod context;
mod data_safety;
mod doctor;
mod projects;
mod workspaces;

use std::fs;
use std::path::Path;

use std::collections::{BTreeMap, HashSet};

use anyhow::{Result, bail};
use serde::Serialize;
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
pub(crate) use self::projects::cmd_project;
pub(crate) use self::workspaces::cmd_workspace;
use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::cli::{
    AddArgs, BulkUpdateArgs, DepCommand, DepSubcommand, InternalNaturalAddArgs, LabelCommand,
    LabelSubcommand, ListArgs, NoteArgs, NoteDeleteArgs, PrimeArgs, RefArgs, SearchArgs, ShowArgs,
    TaskSearchArgs, TextCommand, TextSubcommand, TmuxAddTaskPopupArgs, UpdateArgs,
};
use crate::config::{self as app_config, AppConfig};
use crate::db::{conflict_exists, get_meta};
use crate::input::{read_optional_text, read_required_text};
use crate::labels::list_labels;
use crate::labels::resolve_labels_in_workspace;
use crate::operations::{
    TaskDraft, TaskUpdate, add_note, add_task_dependency, create_label_operation, create_task,
    create_task_in_workspace, delete_label_operation, delete_note, remove_task_dependency,
    set_task_deleted, update_task,
};
use crate::projects::{
    find_project_in_workspace, inferred_project_key_for_add_in_workspace,
    resolve_existing_project_in_workspace,
};
use crate::query::{
    self, SortDirection, TaskFilters, TaskQueryMode, TaskSearchQuery, TaskSearchResult, TaskSort,
};
use crate::refs::{display_ref, display_suffix, resolve_task_ref};
use crate::render::{changed_text, print_multiline_block, print_text_diff, quote};
use crate::task_fields::TaskField;
use crate::task_render::{print_task, print_task_dependency_summary, print_task_line_item};
use crate::types::Task;
use crate::workspaces::{resolve_active_workspace, set_active_workspace, workspace_for_id};

pub(crate) async fn cmd_add(
    conn: &mut SqliteConnection,
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
        {
            bail!(
                "error natural-add-exclusive hint=\"use plain add flags or --natural, not both\""
            );
        }
        crate::task_intake::parse_task_intake(conn, &config.agent.task_intake, &args.title).await?
    } else {
        TaskDraft {
            title: args.title,
            description,
            project: args.project,
            status: "inbox".to_string(),
            priority: args.priority,
            labels: args.label,
        }
    };
    let outcome = create_task(conn, draft).await?;
    let task = outcome.task;
    println!(
        "created {} ref={} project={} status={} priority={} title={}",
        display_ref(conn, &task).await?,
        display_suffix(conn, &task.id).await?,
        task.project_key,
        task.status,
        task.priority,
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
    set_active_workspace(workspace.clone());
    let outcome = async {
        let draft = crate::task_intake::parse_task_intake_in_workspace(
            conn,
            &config.agent.task_intake,
            &args.input,
            &workspace,
            args.project.as_deref(),
        )
        .await?;
        create_task_in_workspace(conn, &args.workspace_id, draft).await
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
        "created {} ref={} project={} status={} priority={} title={}",
        display_ref(conn, &task).await?,
        display_suffix(conn, &task.id).await?,
        task.project_key,
        task.status,
        task.priority,
        quote(&task.title)
    );
    if config.sync.enabled
        && let Ok(addr) = config.wake_addr()
    {
        tracing::debug!(wake_addr = %addr, "waking daemon after internal natural add");
        crate::daemon::wake(addr);
    }
    Ok(())
}

pub(crate) fn cmd_tmux_add_task_popup(args: TmuxAddTaskPopupArgs) -> Result<()> {
    let mut aven_args = vec![
        "aven".to_string(),
        "tui".to_string(),
        "--add-task-only".to_string(),
    ];
    if args.natural {
        aven_args.push("--natural".to_string());
    }
    if let Some(project) = args.project {
        aven_args.push("--project".to_string());
        if !project.is_empty() {
            aven_args.push(project);
        }
    }
    let command = format!(
        "tmux display-popup -E -d '#{{pane_current_path}}' -w {} -h {} -T 'Aven add task' {}",
        shell_quote(&args.width),
        shell_quote(&args.height),
        shell_quote(&aven_args.join(" ")),
    );
    if args.print_binding {
        println!("bind-key A {command}");
    } else {
        println!("{command}");
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub(crate) async fn cmd_show(conn: &mut SqliteConnection, args: ShowArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    print_task(conn, &task, args.full).await
}

pub(crate) async fn cmd_list(conn: &mut SqliteConnection, args: ListArgs) -> Result<()> {
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
    let filters = list_task_filters(args);
    for item in query::list_task_items(
        conn,
        filters,
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
    )
    .await?
    {
        print_task_line_item(&item).await?;
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

pub(crate) async fn cmd_search(conn: &mut SqliteConnection, args: TaskSearchArgs) -> Result<()> {
    let text = args.query.join(" ");
    if text.trim().is_empty() {
        bail!("error search-query-required hint=\"pass one or more search terms\"");
    }
    let results = query::search_task_items(
        conn,
        TaskSearchQuery {
            text,
            include_deleted: args.all,
            limit: args.limit,
        },
    )
    .await?;
    if args.json {
        let items = results.iter().map(search_json_item).collect::<Vec<_>>();
        serde_json::to_writer_pretty(std::io::stdout(), &items)?;
        println!();
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
    let deleted = if item.task.deleted {
        " deleted=yes"
    } else {
        ""
    };
    println!(
        "{} status={} priority={} project={} labels={} match={} score={}{} title={}",
        item.display_ref,
        item.task.status,
        item.task.priority,
        item.task.project_key,
        labels,
        result.matched_field.as_str(),
        result.score,
        deleted,
        quote(&item.task.title)
    );
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
        status: result.item.task.status.clone(),
        priority: result.item.task.priority.clone(),
        labels: result.item.labels.clone(),
        deleted: result.item.task.deleted,
        score: result.score,
        matched_field: result.matched_field,
        snippet: result.snippet.clone(),
    }
}

pub(crate) async fn cmd_dep(conn: &mut SqliteConnection, args: DepCommand) -> Result<()> {
    match args.command {
        DepSubcommand::Add(args) => {
            let task = resolve_task_ref(conn, &args.task_ref).await?;
            let depends_on = resolve_task_ref(conn, &args.depends_on_ref).await?;
            let outcome = add_task_dependency(conn, &task.id, &depends_on.id).await?;
            println!(
                "dependency-added {} changed={} depends_on={}",
                display_ref(conn, &outcome.task).await?,
                changed_text(outcome.changed),
                display_ref(conn, &outcome.depends_on).await?,
            );
        }
        DepSubcommand::Remove(args) => {
            let task = resolve_task_ref(conn, &args.task_ref).await?;
            let depends_on = resolve_task_ref(conn, &args.depends_on_ref).await?;
            let outcome = remove_task_dependency(conn, &task.id, &depends_on.id).await?;
            println!(
                "dependency-removed {} changed={} depends_on={}",
                display_ref(conn, &outcome.task).await?,
                changed_text(outcome.changed),
                display_ref(conn, &outcome.depends_on).await?,
            );
        }
        DepSubcommand::List(args) => {
            let task = resolve_task_ref(conn, &args.task_ref).await?;
            let summary =
                query::task_dependency_summary(conn, &task.workspace_id, &task.id).await?;
            print_task_dependency_summary(&summary);
        }
    }
    Ok(())
}

pub(crate) async fn cmd_bulk_update(
    conn: &mut SqliteConnection,
    args: BulkUpdateArgs,
) -> Result<()> {
    ensure_bulk_update_has_selector(&args)?;
    ensure_bulk_update_has_mutation(&args)?;
    validate_bulk_update_args(&args)?;

    let workspace_id = crate::workspaces::active_workspace_id();
    let labels = resolve_bulk_label_mutations(conn, &workspace_id, &args).await?;
    ensure_disjoint_labels(&labels.add, &labels.remove)?;
    let set_project_key = resolve_bulk_project_mutation(conn, &workspace_id, &args).await?;

    let filters = bulk_update_filters(&args);
    let items = query::list_task_items(
        conn,
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
        let outcome = update_task(conn, &item.task.id, update).await?;
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
    validate_choice("status", status, STATUSES)
}

fn validate_priority(priority: &str) -> Result<()> {
    validate_choice("priority", priority, PRIORITIES)
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
    workspace_id: &str,
    args: &BulkUpdateArgs,
) -> Result<BulkLabelMutations> {
    let add = dedup_labels(resolve_labels_in_workspace(conn, workspace_id, &args.label).await?);
    let remove =
        dedup_labels(resolve_labels_in_workspace(conn, workspace_id, &args.remove_label).await?);
    Ok(BulkLabelMutations { add, remove })
}

async fn resolve_bulk_project_mutation(
    conn: &mut SqliteConnection,
    workspace_id: &str,
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
        project: args.project.clone(),
        status: args.status.clone(),
        statuses: Vec::new(),
        priority: args.priority.clone(),
        label: args.filter_label.clone(),
        include_deleted: args.include_deleted,
        deleted_only: false,
        hide_done: false,
        conflicts_only: false,
        ready_only: false,
        blocked_only: false,
        search: None,
        task_ids: Vec::new(),
    }
}

async fn plan_bulk_updates(
    conn: &mut SqliteConnection,
    workspace_id: &str,
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
    println!(
        "would-update {} changed={} status={} priority={} labels={} title={}",
        item.display_ref,
        changed_text(will_change),
        item.task.status,
        item.task.priority,
        item.labels.join(","),
        quote(&item.task.title)
    );
}

fn print_unchanged_bulk_update(item: &query::TaskListItem) {
    println!(
        "bulk-updated {} changed={} status={} priority={} title={}",
        item.display_ref,
        changed_text(false),
        item.task.status,
        item.task.priority,
        quote(&item.task.title)
    );
}

async fn print_changed_bulk_update(conn: &mut SqliteConnection, task: &Task) -> Result<()> {
    println!(
        "bulk-updated {} changed={} status={} priority={} title={}",
        display_ref(conn, task).await?,
        changed_text(true),
        task.status,
        task.priority,
        quote(&task.title)
    );
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
            .filter(|status| *status != item.task.status)
            .map(str::to_string),
        priority: args
            .set_priority
            .as_deref()
            .filter(|priority| *priority != item.task.priority)
            .map(str::to_string),
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
    workspace_id: &str,
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
    workspace_id: &str,
    display_ref: &str,
    task_id: &str,
    field: &str,
) -> Result<()> {
    if conflict_exists(conn, workspace_id, task_id, field).await? {
        bail!("error bulk-update-conflicted-field ref={display_ref} field={field}");
    }
    Ok(())
}

pub(crate) async fn cmd_prime(conn: &mut SqliteConnection, args: PrimeArgs) -> Result<()> {
    print!("{}", include_str!("skill.md"));
    if !include_str!("skill.md").ends_with('\n') {
        println!();
    }
    println!();
    println!("## Issue Workflow");
    println!();
    println!("- Inspect an issue with `aven show <ref> --full` before changing it.");
    println!(
        "- Mark picked-up work with `aven update <ref> --status active` before making changes."
    );
    println!(
        "- Add durable handoff context with `aven note <ref> ...` for blockers, decisions, or partial progress."
    );
    println!("- Leave blocked or unfinished work open and report the current state.");
    println!(
        "- Mark complete with `aven update <ref> --status done` only after the requested work is complete and required code changes are committed."
    );
    println!("- Use `canceled` only when the user says the issue is no longer needed.");
    println!();

    let workspace_id = crate::workspaces::active_workspace_id();
    let project = if let Some(project) = args.project {
        Some(
            resolve_existing_project_in_workspace(conn, workspace_id.as_str(), &project)
                .await?
                .key,
        )
    } else {
        inferred_project_key_for_add_in_workspace(conn, workspace_id.as_str()).await?
    };

    let Some(project) = project else {
        println!("No current project could be inferred. Run with --project <project>.");
        return Ok(());
    };
    if find_project_in_workspace(conn, workspace_id.as_str(), &project)
        .await?
        .is_none()
    {
        println!("No open issues.");
        return Ok(());
    }

    let filters = prime_task_filters(project.clone());
    let items = query::list_task_items(
        conn,
        filters,
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
    )
    .await?;
    print_prime_conventions(&project, &items);
    print_prime_open_issues(&items);
    Ok(())
}

fn print_prime_open_issues(items: &[query::TaskListItem]) {
    println!("## Open Issues");
    println!();
    if items.is_empty() {
        println!("No open issues.");
        return;
    }

    let active = items
        .iter()
        .filter(|item| item.task.status == "active")
        .collect::<Vec<_>>();
    let ready = items
        .iter()
        .filter(|item| item.task.status != "active" && item.unresolved_blocker_count == 0)
        .collect::<Vec<_>>();
    let blocked = items
        .iter()
        .filter(|item| item.task.status != "active" && item.unresolved_blocker_count > 0)
        .collect::<Vec<_>>();

    println!(
        "Summary: total={} active={} ready={} blocked={}",
        items.len(),
        active.len(),
        ready.len(),
        blocked.len()
    );
    print_prime_top_blockers(items);
    println!();
    print_prime_issue_section("Active", &active);
    print_prime_issue_section("Ready", &ready);
    print_prime_issue_section("Blocked", &blocked);
}

fn print_prime_top_blockers(items: &[query::TaskListItem]) {
    let mut blockers = items
        .iter()
        .filter(|item| item.dependent_count > 0)
        .collect::<Vec<_>>();
    blockers.sort_by(|left, right| {
        right
            .dependent_count
            .cmp(&left.dependent_count)
            .then_with(|| left.display_ref.cmp(&right.display_ref))
    });
    let summary = blockers
        .into_iter()
        .take(3)
        .map(|item| format!("{} blocks={}", item.display_ref, item.dependent_count))
        .collect::<Vec<_>>();
    if summary.is_empty() {
        println!("Top blockers: none.");
    } else {
        println!("Top blockers: {}", summary.join(", "));
    }
}

fn print_prime_issue_section(label: &str, items: &[&query::TaskListItem]) {
    println!("### {label}");
    if items.is_empty() {
        println!("(none)");
        println!();
        return;
    }
    for item in items {
        println!("{}", format_prime_issue_line(item));
    }
    println!();
}

fn format_prime_issue_line(item: &query::TaskListItem) -> String {
    let mut fields = vec![format!("{} status={}", item.display_ref, item.task.status)];
    if item.task.priority != "none" {
        fields.push(format!("priority={}", item.task.priority));
    }
    if !item.labels.is_empty() {
        fields.push(format!("labels={}", item.labels.join(",")));
    }
    if let Some(dependencies) = format_prime_dependency_refs(&item.depends_on) {
        fields.push(format!("blocked_by={dependencies}"));
    }
    if let Some(dependents) = format_prime_dependency_refs(&item.blocks) {
        fields.push(format!("blocks={dependents}"));
    }
    fields.push(format!("title={}", quote(&item.task.title)));
    fields.join(" ")
}

fn format_prime_dependency_refs(links: &[query::TaskDependencyLink]) -> Option<String> {
    const PRIME_DEPENDENCY_REF_LIMIT: usize = 3;

    let refs = links
        .iter()
        .filter(|link| link.unresolved)
        .map(|link| link.display_ref.as_str())
        .collect::<Vec<_>>();
    if refs.is_empty() {
        return None;
    }

    let mut parts = refs
        .iter()
        .take(PRIME_DEPENDENCY_REF_LIMIT)
        .map(|ref_text| (*ref_text).to_string())
        .collect::<Vec<_>>();
    if refs.len() > PRIME_DEPENDENCY_REF_LIMIT {
        parts.push(format!("+{}", refs.len() - PRIME_DEPENDENCY_REF_LIMIT));
    }
    Some(format!("[{}]", parts.join(",")))
}

fn print_prime_conventions(project: &str, items: &[query::TaskListItem]) {
    println!("## Local Conventions");
    println!();
    println!("Project: {project}");
    println!("Open issue sample: {}", items.len());
    if items.is_empty() {
        println!("No open issues are available for convention summaries.");
    } else {
        print_prime_title_conventions(items);
        print_prime_status_conventions(items);
        print_prime_label_conventions(items);
    }
    println!();
}

fn print_prime_title_conventions(items: &[query::TaskListItem]) {
    let lowercase = items
        .iter()
        .filter(|item| starts_with_lowercase(&item.task.title))
        .count();
    let uppercase = items
        .iter()
        .filter(|item| starts_with_uppercase(&item.task.title))
        .count();
    let style = if lowercase > uppercase {
        "mostly lower-case starts"
    } else if uppercase > lowercase {
        "mostly capitalized starts"
    } else if lowercase > 0 {
        "mixed lower-case and capitalized starts"
    } else {
        "no alphabetic title starts in sample"
    };
    println!("Task titles: {style}.");
}

fn starts_with_lowercase(value: &str) -> bool {
    value
        .chars()
        .find(|ch| ch.is_alphabetic())
        .is_some_and(char::is_lowercase)
}

fn starts_with_uppercase(value: &str) -> bool {
    value
        .chars()
        .find(|ch| ch.is_alphabetic())
        .is_some_and(char::is_uppercase)
}

fn print_prime_status_conventions(items: &[query::TaskListItem]) {
    let counts = count_values(items.iter().map(|item| item.task.status.as_str()));
    println!("Common statuses: {}.", format_counts(&counts, 4));
}

fn print_prime_label_conventions(items: &[query::TaskListItem]) {
    let counts = count_values(
        items
            .iter()
            .flat_map(|item| item.labels.iter().map(String::as_str)),
    );
    if counts.is_empty() {
        println!("Common labels: none in open issue sample.");
    } else {
        println!("Common labels: {}.", format_counts(&counts, 6));
    }
}

fn count_values<'a>(values: impl Iterator<Item = &'a str>) -> Vec<(String, usize)> {
    let mut counts = BTreeMap::<String, usize>::new();
    for value in values {
        *counts.entry(value.to_string()).or_default() += 1;
    }
    let mut counts = counts.into_iter().collect::<Vec<_>>();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counts
}

fn format_counts(counts: &[(String, usize)], limit: usize) -> String {
    counts
        .iter()
        .take(limit)
        .map(|(value, count)| format!("{value}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn list_task_filters(args: ListArgs) -> TaskFilters {
    TaskFilters {
        project: args.project,
        status: args.status,
        statuses: Vec::new(),
        priority: args.priority,
        label: args.label,
        include_deleted: args.all || args.deleted,
        deleted_only: args.deleted,
        hide_done: false,
        conflicts_only: false,
        ready_only: args.ready,
        blocked_only: args.blocked,
        search: None,
        task_ids: Vec::new(),
    }
}

fn prime_task_filters(project: String) -> TaskFilters {
    TaskFilters {
        project: Some(project),
        status: None,
        statuses: Vec::new(),
        priority: None,
        label: None,
        include_deleted: false,
        deleted_only: false,
        hide_done: true,
        conflicts_only: false,
        ready_only: false,
        blocked_only: false,
        search: None,
        task_ids: Vec::new(),
    }
}

pub(crate) async fn cmd_update(conn: &mut SqliteConnection, args: UpdateArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?;
    validate_optional_status(args.status.as_deref())?;
    validate_optional_priority(args.priority.as_deref())?;
    let outcome = update_task(
        conn,
        &task.id,
        TaskUpdate {
            title: args.title,
            description,
            project: args.project,
            status: args.status,
            priority: args.priority,
            add_labels: args.label,
            remove_labels: args.remove_label,
        },
    )
    .await?;
    let task = outcome.task;
    println!(
        "updated {} changed={} status={} priority={} title={}",
        display_ref(conn, &task).await?,
        changed_text(outcome.changed),
        task.status,
        task.priority,
        quote(&task.title)
    );
    Ok(())
}

pub(crate) async fn cmd_note(conn: &mut SqliteConnection, args: NoteArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    let body = read_required_text(args.text, args.file.as_deref(), args.stdin, "note")?;
    let outcome = add_note(conn, &task.id, body).await?;
    println!(
        "noted {} note={}",
        display_ref(conn, &task).await?,
        outcome.note_id
    );
    Ok(())
}

pub(crate) async fn cmd_note_delete(
    conn: &mut SqliteConnection,
    args: NoteDeleteArgs,
) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    let outcome = delete_note(conn, &task.id, &args.note_id).await?;
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
    format!("{:x}", hasher.finalize())
}

pub(crate) async fn cmd_text(conn: &mut SqliteConnection, args: TextCommand) -> Result<()> {
    match args.command {
        TextSubcommand::Get(args) => {
            ensure_description_field(&args.field)?;
            let task = resolve_task_ref(conn, &args.task_ref).await?;
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
            let task = resolve_task_ref(conn, &args.task_ref).await?;
            let current = TaskField::Description.current_value(&task);
            let candidate = fs::read_to_string(&args.file)?;
            print_text_diff("current", &current, "candidate", &candidate);
        }
        TextSubcommand::Set(args) => {
            ensure_description_field(&args.field)?;
            let value = read_required_text(None, args.file.as_deref(), args.stdin, "text")?;
            let task = resolve_task_ref(conn, &args.task_ref).await?;
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

pub(crate) async fn cmd_labels(conn: &mut SqliteConnection, args: SearchArgs) -> Result<()> {
    let labels = list_labels(conn, args.search.as_deref()).await?;
    for label in labels {
        println!("{label}");
    }
    Ok(())
}

pub(crate) async fn cmd_label(conn: &mut SqliteConnection, args: LabelCommand) -> Result<()> {
    match args.command {
        LabelSubcommand::Create { name } => {
            let outcome = create_label_operation(conn, &name).await?;
            println!("created-label {}", outcome.name);
        }
        LabelSubcommand::Delete { name } => {
            let outcome = delete_label_operation(conn, &name).await?;
            println!(
                "deleted-label {} changed={}",
                outcome.name,
                changed_text(outcome.changed),
            );
        }
        LabelSubcommand::List(args) => cmd_labels(conn, args).await?,
    }
    Ok(())
}

pub(crate) async fn cmd_delete_restore(
    conn: &mut SqliteConnection,
    args: RefArgs,
    delete: bool,
) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    let outcome = set_task_deleted(conn, &task.id, delete).await?;
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

pub(crate) async fn cmd_doctor(
    conn: &mut SqliteConnection,
    config: &AppConfig,
    db_path: &Path,
    db_flag_set: bool,
    workspace_flag: Option<&str>,
    integrity: bool,
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
    let pending_changes: i64 =
        sqlx::query_scalar("SELECT count(*) FROM changes WHERE server_seq IS NULL")
            .fetch_one(&mut *conn)
            .await?;
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
    database_section.info("pending changes", pending_changes.to_string());
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

    DoctorRenderer::auto().print(&report);
    Ok(())
}
