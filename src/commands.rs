mod doctor;

use std::path::Path;

use std::collections::HashSet;

use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use doctor::{DoctorRenderer, DoctorReport, sync_server_url_is_valid, workspace_counts};

use crate::config::{self, AppConfig};
use crate::db::{conflict_exists, get_meta};
use crate::workspaces::resolve_active_workspace;

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::cli::{
    AddArgs, BulkUpdateArgs, ConfigCommand, ConfigSubcommand, ConflictCommand, ConflictSubcommand,
    LabelCommand, LabelSubcommand, ListArgs, NoteArgs, PrimeArgs, ProjectCommand,
    ProjectPathSubcommand, ProjectSubcommand, RefArgs, SearchArgs, ShowArgs, TmuxAddTaskPopupArgs,
    UpdateArgs, WorkspaceCommand, WorkspaceSubcommand,
};
use crate::input::{read_optional_text, read_required_text};
use crate::labels::list_labels;
use crate::labels::resolve_labels_in_workspace;
use crate::operations::{
    TaskDraft, TaskUpdate, add_note, add_project_path_operation, conflict_variant_value,
    create_label_operation, create_project_operation, create_task, init_config, list_conflicts,
    list_project_paths_operation, remove_project_path_operation, resolve_conflict,
    set_task_deleted, show_config, task_conflicts, update_task,
};
use crate::projects::{
    find_project_in_workspace, inferred_project_key_for_add_in_workspace, list_projects,
    resolve_existing_project_in_workspace,
};
use crate::query::{self, SortDirection, TaskFilters, TaskSort};
use crate::refs::{display_ref, display_suffix, resolve_task_ref};
use crate::render::quote;
use crate::task_render::{print_task, print_task_line_item};
use crate::workspaces::{create_workspace, list_workspaces, rename_workspace};

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

pub(crate) fn cmd_tmux_add_task_popup(args: TmuxAddTaskPopupArgs) -> Result<()> {
    let mut aven_args = vec![
        "aven".to_string(),
        "tui".to_string(),
        "--add-task".to_string(),
    ];
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
    let filters = TaskFilters {
        project: args.project,
        status: args.status,
        priority: args.priority,
        label: args.label,
        include_deleted: args.all,
        hide_done: false,
        conflicts_only: false,
        search: None,
    };
    for item in
        query::list_task_items(conn, filters, TaskSort::Updated, SortDirection::Desc).await?
    {
        print_task_line_item(&item).await?;
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
        priority: args.priority.clone(),
        label: args.filter_label.clone(),
        include_deleted: args.include_deleted,
        hide_done: false,
        conflicts_only: false,
        search: None,
    };
    let items =
        query::list_task_items(conn, filters, TaskSort::Updated, SortDirection::Desc).await?;
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
        priority: None,
        label: None,
        include_deleted: false,
        hide_done: true,
        conflicts_only: false,
        search: None,
    };
    let items =
        query::list_task_items(conn, filters, TaskSort::Updated, SortDirection::Desc).await?;
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
                println!(
                    "variant {} value={}",
                    detail.variant_a,
                    quote(&detail.local_value)
                );
                println!(
                    "variant {} value={}",
                    detail.variant_b,
                    quote(&detail.remote_value)
                );
            }
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
