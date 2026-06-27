mod doctor;

use std::fs;
use std::path::Path;

use std::collections::HashSet;

use anyhow::{Result, bail};
use serde::Serialize;
use sha2::{Digest, Sha256};
use similar::TextDiff;
use sqlx::{Row, SqliteConnection};

use doctor::{DoctorRenderer, DoctorReport, sync_server_url_is_valid, workspace_counts};

use crate::config::{self, AppConfig};
use crate::db::{conflict_exists, get_meta};

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::cli::{
    AddArgs, BulkUpdateArgs, ConfigCommand, ConfigSubcommand, ConflictCommand, ConflictSubcommand,
    ContextArgs, DepCommand, DepSubcommand, InternalNaturalAddArgs, LabelCommand, LabelSubcommand,
    ListArgs, NoteArgs, PrimeArgs, ProjectCommand, ProjectPathSubcommand, ProjectSubcommand,
    RefArgs, SearchArgs, ShowArgs, TextCommand, TextSubcommand, TmuxAddTaskPopupArgs, UpdateArgs,
    WorkspaceCommand, WorkspaceSubcommand,
};
use crate::input::{read_optional_text, read_required_text};
use crate::labels::list_labels;
use crate::labels::resolve_labels_in_workspace;
use crate::operations::{
    ConflictDetail, TaskDraft, TaskUpdate, add_note, add_project_path_operation,
    add_task_dependency, conflict_variant_value, create_label_operation, create_project_operation,
    create_task, create_task_in_workspace, init_config, list_conflicts,
    list_project_paths_operation, remove_project_path_operation, remove_task_dependency,
    rename_project_operation, resolve_conflict, set_task_deleted, show_config, task_conflicts,
    update_task,
};
use crate::projects::{
    find_project_in_workspace, inferred_project_key_for_add_in_workspace, list_projects,
    resolve_existing_project_in_workspace,
};
use crate::query::{
    self, SortDirection, TaskDependencyItem, TaskFilters, TaskQueryMode, TaskSort,
    task_dependency_summary,
};
use crate::refs::{display_ref, display_suffix, resolve_task_ref};
use crate::render::{print_multiline_block, quote};
use crate::task_fields::TaskField;
use crate::task_render::{
    labels_for_task_in_workspace, print_task, print_task_dependency_summary, print_task_line_item,
};
use crate::types::Task;
use crate::workspaces::{
    active_workspace, create_workspace, list_workspaces, rename_workspace,
    resolve_active_workspace, set_active_workspace, workspace_for_id,
};

pub(crate) async fn cmd_add(
    conn: &mut SqliteConnection,
    config: &AppConfig,
    args: AddArgs,
) -> Result<()> {
    validate_choice("priority", &args.priority, PRIORITIES)?;
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

pub(crate) async fn cmd_list(conn: &mut SqliteConnection, args: ListArgs) -> Result<()> {
    if args.ready && args.blocked {
        bail!(
            "error list-dependency-filter-conflict hint=\"pass at most one of --ready or --blocked\""
        );
    }
    if (args.ready || args.blocked) && args.all {
        bail!(
            "error list-dependency-filter-all-conflict hint=\"dependency filters only include open tasks\""
        );
    }
    let filters = TaskFilters {
        project: args.project,
        status: args.status,
        statuses: Vec::new(),
        priority: args.priority,
        label: args.label,
        include_deleted: args.all,
        hide_done: false,
        conflicts_only: false,
        ready_only: args.ready,
        blocked_only: args.blocked,
        search: None,
    };
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

pub(crate) async fn cmd_dep(conn: &mut SqliteConnection, args: DepCommand) -> Result<()> {
    match args.command {
        DepSubcommand::Add(args) => {
            let task = resolve_task_ref(conn, &args.task_ref).await?;
            let depends_on = resolve_task_ref(conn, &args.depends_on_ref).await?;
            let outcome = add_task_dependency(conn, &task.id, &depends_on.id).await?;
            println!(
                "dependency-added {} changed={} depends_on={}",
                display_ref(conn, &outcome.task).await?,
                if outcome.changed { "yes" } else { "none" },
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
                if outcome.changed { "yes" } else { "none" },
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
    let add_labels =
        dedup_labels(resolve_labels_in_workspace(conn, &workspace_id, &args.label).await?);
    let remove_labels =
        dedup_labels(resolve_labels_in_workspace(conn, &workspace_id, &args.remove_label).await?);
    ensure_disjoint_labels(&add_labels, &remove_labels)?;
    let set_project_key = if let Some(project) = args.set_project.as_deref() {
        Some(
            resolve_existing_project_in_workspace(conn, &workspace_id, project)
                .await?
                .key,
        )
    } else {
        None
    };

    let filters = TaskFilters {
        project: args.project.clone(),
        status: args.status.clone(),
        statuses: Vec::new(),
        priority: args.priority.clone(),
        label: args.filter_label.clone(),
        include_deleted: args.include_deleted,
        hide_done: false,
        conflicts_only: false,
        ready_only: false,
        blocked_only: false,
        search: None,
    };
    let items = query::list_task_items(
        conn,
        filters,
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
    )
    .await?;
    let matched = items.len();
    let mut planned = Vec::with_capacity(matched);
    for item in items {
        let update = bulk_update_for_item(
            &item,
            &args,
            &add_labels,
            &remove_labels,
            set_project_key.as_deref(),
        );
        let will_change = bulk_update_has_changes(&update);
        preflight_bulk_update_item(conn, &workspace_id, &item, &update).await?;
        planned.push((item, update, will_change));
    }

    let would_change = planned
        .iter()
        .filter(|(_, _, will_change)| *will_change)
        .count();
    let mut changed = 0;
    let mut unchanged = 0;
    for (item, update, will_change) in planned {
        if args.dry_run {
            println!(
                "would-update {} changed={} status={} priority={} labels={} title={}",
                item.display_ref,
                if will_change { "yes" } else { "none" },
                item.task.status,
                item.task.priority,
                item.labels.join(","),
                quote(&item.task.title)
            );
            continue;
        }
        if !will_change {
            unchanged += 1;
            println!(
                "bulk-updated {} changed=none status={} priority={} title={}",
                item.display_ref,
                item.task.status,
                item.task.priority,
                quote(&item.task.title)
            );
            continue;
        }
        let outcome = update_task(conn, &item.task.id, update).await?;
        changed += 1;
        println!(
            "bulk-updated {} changed=yes status={} priority={} title={}",
            display_ref(conn, &outcome.task).await?,
            outcome.task.status,
            outcome.task.priority,
            quote(&outcome.task.title)
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
    {
        return Ok(());
    }
    bail!("error bulk-update-requires-mutation hint=\"add a mutation flag\"");
}

fn validate_bulk_update_args(args: &BulkUpdateArgs) -> Result<()> {
    if let Some(status) = args.status.as_deref() {
        validate_choice("status", status, STATUSES)?;
    }
    if let Some(priority) = args.priority.as_deref() {
        validate_choice("priority", priority, PRIORITIES)?;
    }
    if let Some(status) = args.set_status.as_deref() {
        validate_choice("status", status, STATUSES)?;
    }
    if let Some(priority) = args.set_priority.as_deref() {
        validate_choice("priority", priority, PRIORITIES)?;
    }
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
    println!("## Open Issues");
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

    println!("project={project}");
    let filters = TaskFilters {
        project: Some(project),
        status: None,
        statuses: Vec::new(),
        priority: None,
        label: None,
        include_deleted: false,
        hide_done: true,
        conflicts_only: false,
        ready_only: false,
        blocked_only: false,
        search: None,
    };
    let items = query::list_task_items(
        conn,
        filters,
        TaskQueryMode::Flat,
        TaskSort::Updated,
        SortDirection::Desc,
    )
    .await?;
    if items.is_empty() {
        println!("No open issues.");
        return Ok(());
    }
    for item in items {
        print_task_line_item(&item).await?;
    }
    Ok(())
}

pub(crate) async fn cmd_update(conn: &mut SqliteConnection, args: UpdateArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?;
    if let Some(status) = args.status.as_deref() {
        validate_choice("status", status, STATUSES)?;
    }
    if let Some(priority) = args.priority.as_deref() {
        validate_choice("priority", priority, PRIORITIES)?;
    }
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
        if outcome.changed { "yes" } else { "none" },
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

fn print_text_diff(from_label: &str, old: &str, to_label: &str, new: &str) {
    let diff = TextDiff::from_lines(old, new);
    let unified = diff
        .unified_diff()
        .context_radius(3)
        .header(from_label, to_label)
        .to_string();
    if unified.is_empty() {
        println!(" no changes");
    } else {
        print!("{unified}");
    }
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

pub(crate) async fn cmd_projects(conn: &mut SqliteConnection, args: SearchArgs) -> Result<()> {
    let projects = list_projects(conn, args.search.as_deref()).await?;
    for project in projects {
        println!(
            "{} prefix={} name={}",
            project.key,
            project.prefix,
            quote(&project.name)
        );
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
        LabelSubcommand::List(args) => cmd_labels(conn, args).await?,
    }
    Ok(())
}

pub(crate) async fn cmd_project(conn: &mut SqliteConnection, args: ProjectCommand) -> Result<()> {
    match args.command {
        ProjectSubcommand::Create { name, path } => {
            let outcome = create_project_operation(conn, &name, path.as_deref()).await?;
            let project = outcome.project;
            println!(
                "created-project {} prefix={} name={}",
                project.key,
                project.prefix,
                quote(&project.name)
            );
        }
        ProjectSubcommand::List(args) => cmd_projects(conn, args).await?,
        ProjectSubcommand::Rename {
            project,
            new_name,
            prefix,
        } => {
            let workspace = crate::workspaces::active_workspace();
            let outcome =
                rename_project_operation(conn, &workspace, &project, &new_name, prefix.as_deref())
                    .await?;
            println!(
                "renamed-project {} changed={} old={} old_prefix={} prefix={} name={}",
                outcome.project.key,
                if outcome.changed { "yes" } else { "none" },
                outcome.previous.key,
                outcome.previous.prefix,
                outcome.project.prefix,
                quote(&outcome.project.name)
            );
            if outcome.changed && outcome.config_mapping {
                println!("updated-config-project-mapping {}", outcome.project.key);
            }
        }
        ProjectSubcommand::Path { command } => match command {
            ProjectPathSubcommand::Add { project, path } => {
                let outcome = add_project_path_operation(conn, &project, &path).await?;
                println!(
                    "added-project-path {} path={} config={}",
                    outcome.project.key,
                    quote(&outcome.path),
                    quote(&outcome.config_path.display().to_string())
                );
            }
            ProjectPathSubcommand::Remove { project, path } => {
                let outcome = remove_project_path_operation(conn, &project, &path).await?;
                println!(
                    "removed-project-path {} path={} config={}",
                    outcome.project.key,
                    quote(&outcome.path),
                    quote(&outcome.config_path.display().to_string())
                );
            }
            ProjectPathSubcommand::List { project } => {
                let paths = list_project_paths_operation(conn, project.as_deref()).await?;
                for item in paths {
                    println!("{} path={}", item.project.key, quote(&item.path));
                }
            }
        },
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

pub(crate) async fn cmd_config(args: ConfigCommand) -> Result<()> {
    match args.command {
        ConfigSubcommand::Init => {
            let outcome = init_config()?;
            println!(
                "created-config path={}",
                quote(&outcome.path.display().to_string())
            );
        }
        ConfigSubcommand::Show => {
            let outcome = show_config()?;
            println!("config path={}", quote(&outcome.path.display().to_string()));
            println!("{}", outcome.text);
        }
    }
    Ok(())
}

pub(crate) async fn cmd_workspace(
    conn: &mut SqliteConnection,
    args: WorkspaceCommand,
) -> Result<()> {
    match args.command {
        WorkspaceSubcommand::List => {
            for workspace in list_workspaces(conn).await? {
                println!("{} name={}", workspace.key, quote(&workspace.name));
            }
        }
        WorkspaceSubcommand::Create { name } => {
            let workspace = create_workspace(conn, &name).await?;
            println!(
                "created-workspace {} name={}",
                workspace.key,
                quote(&workspace.name)
            );
        }
        WorkspaceSubcommand::Rename {
            workspace,
            new_name,
        } => {
            let workspace = rename_workspace(conn, &workspace, &new_name).await?;
            println!(
                "renamed-workspace {} name={}",
                workspace.key,
                quote(&workspace.name)
            );
        }
    }
    Ok(())
}

pub(crate) async fn cmd_skill() -> Result<()> {
    print!("{}", include_str!("skill.md"));
    Ok(())
}

pub(crate) async fn cmd_conflict(conn: &mut SqliteConnection, args: ConflictCommand) -> Result<()> {
    match args.command {
        ConflictSubcommand::List { project, field } => {
            let project_key = if let Some(project) = project {
                Some(
                    resolve_existing_project_in_workspace(
                        conn,
                        crate::workspaces::active_workspace_id().as_str(),
                        &project,
                    )
                    .await?
                    .key,
                )
            } else {
                None
            };
            let items = list_conflicts(conn, project_key.as_deref(), field.as_deref()).await?;
            for item in items {
                let display = format!(
                    "{}-{}",
                    item.project_prefix,
                    display_suffix(conn, &item.task_id).await?
                );
                println!(
                    "{} conflict field={} variants={},{} title={}",
                    display,
                    item.field,
                    item.variant_a,
                    item.variant_b,
                    quote(&item.title)
                );
            }
        }
        ConflictSubcommand::Show { task_ref, field } => {
            let task = resolve_task_ref(conn, &task_ref).await?;
            let details = task_conflicts(conn, &task.id, field.as_deref()).await?;
            for detail in details {
                println!(
                    "conflict {} field={}",
                    display_ref(conn, &task).await?,
                    detail.field
                );
                let local_value = conflict_display_value(
                    conn,
                    &task.workspace_id,
                    &detail.field,
                    &detail.local_value,
                )
                .await?;
                let remote_value = conflict_display_value(
                    conn,
                    &task.workspace_id,
                    &detail.field,
                    &detail.remote_value,
                )
                .await?;
                println!("variant {}", detail.variant_a);
                print_multiline_block("value", &local_value);
                println!("variant {}", detail.variant_b);
                print_multiline_block("value", &remote_value);
            }
        }
        ConflictSubcommand::Diff { task_ref, field } => {
            let task = resolve_task_ref(conn, &task_ref).await?;
            let detail = single_conflict(
                task_conflicts(conn, &task.id, Some(&field)).await?,
                &task.id,
                &field,
            )?;
            print_text_diff("local", &detail.local_value, "remote", &detail.remote_value);
        }
        ConflictSubcommand::Export {
            task_ref,
            field,
            dir,
        } => {
            let task = resolve_task_ref(conn, &task_ref).await?;
            fs::create_dir_all(&dir)?;
            let detail = single_conflict(
                task_conflicts(conn, &task.id, Some(&field)).await?,
                &task.id,
                &field,
            )?;
            let path_a = dir.join(format!("{}-{}.md", detail.field, detail.variant_a));
            fs::write(&path_a, &detail.local_value)?;
            println!(
                "exported variant={} path={}",
                detail.variant_a,
                quote(&path_a.display().to_string())
            );
            let path_b = dir.join(format!("{}-{}.md", detail.field, detail.variant_b));
            fs::write(&path_b, &detail.remote_value)?;
            println!(
                "exported variant={} path={}",
                detail.variant_b,
                quote(&path_b.display().to_string())
            );
        }
        ConflictSubcommand::Resolve {
            task_ref,
            field,
            use_variant,
            value,
            value_file,
            value_stdin,
        } => {
            let task = resolve_task_ref(conn, &task_ref).await?;
            let value = if let Some(token) = use_variant {
                conflict_variant_value(conn, &task.id, &field, &token).await?
            } else {
                read_required_text(value, value_file.as_deref(), value_stdin, "value")?
            };
            let outcome = resolve_conflict(conn, &task.id, &field, &value).await?;
            println!(
                "resolved {} field={}",
                display_ref(conn, &outcome.task).await?,
                outcome.field
            );
        }
    }
    Ok(())
}

async fn conflict_display_value(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    field: &str,
    value: &str,
) -> Result<String> {
    if field != TaskField::Project.as_str() {
        return Ok(value.to_string());
    }
    if let Some((key, prefix)) = sqlx::query_as::<_, (String, String)>(
        "SELECT key, prefix FROM projects WHERE workspace_id = ? AND id = ?",
    )
    .bind(workspace_id)
    .bind(value)
    .fetch_optional(&mut *conn)
    .await?
    {
        return Ok(format!("{key} prefix={prefix}"));
    }
    Ok(value.to_string())
}

fn single_conflict(
    details: Vec<ConflictDetail>,
    task_id: &str,
    field: &str,
) -> Result<ConflictDetail> {
    let mut iter = details.into_iter();
    let Some(detail) = iter.next() else {
        bail!("error conflict-not-found task_id={task_id} field={field}");
    };
    if iter.next().is_some() {
        bail!(
            "error multiple-conflicts task_id={task_id} field={field} hint=\"use export to view all variants\""
        );
    }
    Ok(detail)
}

pub(crate) async fn cmd_doctor(
    conn: &mut SqliteConnection,
    config: &AppConfig,
    db_path: &Path,
    db_flag_set: bool,
    workspace_flag: Option<&str>,
) -> Result<()> {
    let config_file = config::config_file_path();
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
    let sync_server = config::resolve_sync_server(None, config);
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

    DoctorRenderer::auto().print(&report);
    Ok(())
}
