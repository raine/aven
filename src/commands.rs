use std::path::Path;

use anyhow::Result;
use sqlx::SqliteConnection;

use crate::config::{self, AppConfig};
use crate::db::get_meta;
use crate::workspaces::resolve_active_workspace;

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::cli::{
    AddArgs, ConfigCommand, ConfigSubcommand, ConflictCommand, ConflictSubcommand, LabelCommand,
    LabelSubcommand, ListArgs, NoteArgs, ProjectCommand, ProjectPathSubcommand, ProjectSubcommand,
    RefArgs, SearchArgs, ShowArgs, UpdateArgs, WorkspaceCommand, WorkspaceSubcommand,
};
use crate::input::{read_optional_text, read_required_text};
use crate::labels::list_labels;
use crate::operations::{
    TaskDraft, TaskUpdate, add_note, add_project_path_operation, conflict_variant_value,
    create_label_operation, create_project_operation, create_task, init_config, list_conflicts,
    remove_project_path_operation, resolve_conflict, set_task_deleted, show_config, task_conflicts,
    update_task,
};
use crate::projects::{list_projects, resolve_existing_project_in_workspace};
use crate::query::{self, SortDirection, TaskFilters, TaskSort};
use crate::refs::{display_ref, display_suffix, resolve_task_ref};
use crate::render::quote;
use crate::task_render::{print_task, print_task_line_item};
use crate::workspaces::{create_workspace, list_workspaces, rename_workspace};

pub(crate) async fn cmd_add(conn: &mut SqliteConnection, args: AddArgs) -> Result<()> {
    validate_choice("priority", &args.priority, PRIORITIES)?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?
    .unwrap_or_default();
    let outcome = create_task(
        conn,
        TaskDraft {
            title: args.title,
            description,
            project: args.project,
            priority: args.priority,
            labels: args.label,
        },
    )
    .await?;
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
                    "added-project-path {} path={}",
                    outcome.project.key,
                    quote(&outcome.path)
                );
            }
            ProjectPathSubcommand::Remove { project, path } => {
                let outcome = remove_project_path_operation(conn, &project, &path).await?;
                println!(
                    "removed-project-path {} path={}",
                    outcome.project.key,
                    quote(&outcome.path)
                );
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
    } else if std::env::var_os("ATM_DB").is_some() {
        "ATM_DB"
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
    let pending_changes: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM changes WHERE server_seq IS NULL",
    )
    .fetch_one(&mut *conn)
    .await?;
    let unresolved_conflicts: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM conflicts WHERE resolved = 0",
    )
    .fetch_one(&mut *conn)
    .await?;
    let sync_server = config::resolve_sync_server(None, config);
    let wake_addr = config.wake_addr();

    println!("atm doctor");
    println!();
    print_section("Configuration");
    match config_file {
        Ok(path) if path.exists() => print_check("config file", true, &path.display().to_string()),
        Ok(path) => print_info("config file", &format!("{} (using defaults)", path.display())),
        Err(error) => print_check("config file", false, &format!("{error:#}")),
    }
    print_info("database source", db_source);
    print_info("database path", &db_path.display().to_string());
    println!();
    print_section("Database");
    print_check("sqlite", true, "opened successfully");
    print_check("client id", client_id.is_some(), client_id.as_deref().unwrap_or("missing"));
    print_info("sync cursor", sync_cursor.as_deref().unwrap_or("missing"));
    print_info("local sequence", local_seq.as_deref().unwrap_or("missing"));
    print_info("pinned server", pinned_server.as_deref().unwrap_or("none"));
    print_info("pending changes", &pending_changes.to_string());
    print_info("conflicts", &unresolved_conflicts.to_string());
    println!();
    print_section("Workspace");
    match workspace {
        Ok(workspace) => {
            print_check(
                "active workspace",
                true,
                &format!("{} ({})", workspace.name, workspace.key),
            );
            if let Some((visible_count, all_count)) = counts {
                print_info("tasks", &format!("{visible_count} visible, {all_count} total"));
            }
        }
        Err(error) => print_check("active workspace", false, &format!("{error:#}")),
    }
    println!();
    print_section("Sync");
    print_info("enabled", if config.sync.enabled { "yes" } else { "no" });
    match sync_server {
        Ok(server) => {
            print_check("server", sync_server_url_is_valid(&server), &server);
            if let Some(pinned) = pinned_server.as_deref() {
                let normalized = server.trim_end_matches('/');
                print_check(
                    "server match",
                    pinned == normalized,
                    &format!("pinned={pinned} configured={normalized}"),
                );
            }
        }
        Err(error) => {
            if config.sync.enabled {
                print_check("server", false, &format!("{error:#}"));
            } else {
                print_info("server", "not configured");
            }
        }
    }
    match config.sync.server_url.as_deref() {
        Some(server) => print_check("daemon server", sync_server_url_is_valid(server), server),
        None if config.sync.enabled => print_check("daemon server", false, "not configured"),
        None => print_info("daemon server", "not configured"),
    }
    print_info(
        "auth token",
        if config.sync_auth_token().is_some() {
            "configured"
        } else {
            "not configured"
        },
    );
    print_info("interval", &format!("{} seconds", config.sync_interval_seconds()));
    match wake_addr {
        Ok(addr) => print_check("daemon wake", true, &addr.to_string()),
        Err(error) => print_check("daemon wake", false, &format!("{error:#}")),
    }
    Ok(())
}

fn sync_server_url_is_valid(server: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(server) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
}

async fn workspace_counts(
    conn: &mut SqliteConnection,
    workspace_id: &str,
) -> Result<(i64, i64)> {
    let active = sqlx::query_scalar(
        "SELECT count(*) FROM tasks WHERE workspace_id = ? AND deleted = 0",
    )
    .bind(workspace_id)
    .fetch_one(&mut *conn)
    .await?;
    let all = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ?")
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok((active, all))
}

fn print_section(title: &str) {
    println!("{title}");
    println!("{}", "-".repeat(title.len()));
}

fn print_check(label: &str, ok: bool, value: &str) {
    let marker = if ok { "ok" } else { "!!" };
    println!("  {marker} {label:<18} {value}");
}

fn print_info(label: &str, value: &str) {
    println!("  .. {label:<18} {value}");
}
