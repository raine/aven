use std::io::{self, IsTerminal};
use std::path::Path;

use anyhow::Result;
use crossterm::style::{Color, Stylize};
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
    list_project_paths_operation, remove_project_path_operation, resolve_conflict,
    set_task_deleted, show_config, task_conflicts, update_task,
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

async fn workspace_counts(conn: &mut SqliteConnection, workspace_id: &str) -> Result<(i64, i64)> {
    let active =
        sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ? AND deleted = 0")
            .bind(workspace_id)
            .fetch_one(&mut *conn)
            .await?;
    let all = sqlx::query_scalar("SELECT count(*) FROM tasks WHERE workspace_id = ?")
        .bind(workspace_id)
        .fetch_one(&mut *conn)
        .await?;
    Ok((active, all))
}

struct DoctorReport {
    sections: Vec<DoctorSection>,
}

impl DoctorReport {
    fn new() -> Self {
        Self {
            sections: Vec::new(),
        }
    }

    fn section(&mut self, title: &'static str) -> &mut DoctorSection {
        self.sections.push(DoctorSection {
            title,
            rows: Vec::new(),
        });
        self.sections.last_mut().expect("section was pushed")
    }
}

struct DoctorSection {
    title: &'static str,
    rows: Vec<DoctorRow>,
}

impl DoctorSection {
    fn check(&mut self, label: &'static str, ok: bool, value: impl Into<String>) {
        self.rows.push(DoctorRow {
            status: if ok {
                DoctorStatus::Ok
            } else {
                DoctorStatus::Error
            },
            label,
            value: value.into(),
        });
    }

    fn info(&mut self, label: &'static str, value: impl Into<String>) {
        self.rows.push(DoctorRow {
            status: DoctorStatus::Info,
            label,
            value: value.into(),
        });
    }
}

struct DoctorRow {
    status: DoctorStatus,
    label: &'static str,
    value: String,
}

#[derive(Clone, Copy)]
enum DoctorStatus {
    Ok,
    Error,
    Info,
}

struct DoctorRenderer {
    styled: bool,
}

impl DoctorRenderer {
    fn auto() -> Self {
        Self {
            styled: io::stdout().is_terminal() && std::env::var_os("NO_COLOR").is_none(),
        }
    }

    fn print(&self, report: &DoctorReport) {
        if self.styled {
            println!(
                "{}",
                "aven doctor"
                    .with(Color::Rgb {
                        r: 45,
                        g: 174,
                        b: 135
                    })
                    .bold()
            );
        } else {
            println!("aven doctor");
        }
        for section in &report.sections {
            println!();
            self.print_section(section.title);
            let label_width = section
                .rows
                .iter()
                .map(|row| row.label.chars().count())
                .max()
                .unwrap_or(0);
            for row in &section.rows {
                self.print_row(row, label_width);
            }
        }
    }

    fn print_section(&self, title: &str) {
        if self.styled {
            println!("{}", title.with(Color::Cyan).bold());
        } else {
            println!("{title}");
            println!("{}", "-".repeat(title.len()));
        }
    }

    fn print_row(&self, row: &DoctorRow, label_width: usize) {
        if self.styled {
            self.print_styled_row(row, label_width);
        } else {
            let marker = row.status.marker();
            println!("  {marker} {:<18} {}", row.label, row.value);
        }
    }

    fn print_styled_row(&self, row: &DoctorRow, label_width: usize) {
        let label = format!("{:<label_width$}", row.label);
        println!(
            "  {} {}  {}",
            row.status.icon().with(row.status.color()).bold(),
            label.with(row.status.label_color()),
            row.value.as_str().with(Color::Rgb {
                r: 150,
                g: 150,
                b: 150,
            })
        );
    }
}

impl DoctorStatus {
    fn marker(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "!!",
            Self::Info => "..",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Ok => "✓",
            Self::Error => "✗",
            Self::Info => "·",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Ok => Color::Green,
            Self::Error => Color::Red,
            Self::Info => Color::DarkGrey,
        }
    }

    fn label_color(self) -> Color {
        match self {
            Self::Ok | Self::Error => Color::White,
            Self::Info => Color::Grey,
        }
    }
}
