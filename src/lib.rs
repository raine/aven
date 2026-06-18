use anyhow::{Context, Result, bail};
use clap::Parser;
use rand::RngCore;
use serde_json::json;
use sqlx::{Connection as _, QueryBuilder, Sqlite, SqliteConnection};
use std::env;
use std::fs;
use std::io::{self, Read};
use std::path::Path;
use std::process::Command;

mod cli;
mod config;
mod daemon;
mod db;
mod sync;

pub use cli::Cli;

use cli::{
    AddArgs, Commands, ConfigCommand, ConfigSubcommand, ConflictCommand, ConflictSubcommand,
    DaemonSubcommand, LabelCommand, LabelSubcommand, ListArgs, NoteArgs, ProjectCommand,
    ProjectPathSubcommand, ProjectSubcommand, RefArgs, SearchArgs, ShowArgs, SyncArgs, UpdateArgs,
};
use db::{
    conflict_exists, field_version, insert_change, open_db, set_field_version, task_from_row,
    task_has_conflict,
};
use sync::{run_server, sync_client};

const STATUSES: &[&str] = &["inbox", "backlog", "todo", "active", "done", "canceled"];
const PRIORITIES: &[&str] = &["none", "low", "medium", "high", "urgent"];
const BASE32: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, Clone)]
struct Task {
    id: String,
    title: String,
    description: String,
    project_key: String,
    project_prefix: String,
    status: String,
    priority: String,
    created_at: String,
    updated_at: String,
    deleted: bool,
}

pub async fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Server(args) => run_server(args).await,
        Commands::Config(args) => cmd_config(args).await,
        Commands::Daemon(args) => {
            let config = config::AppConfig::load()?;
            let db_path = config::resolve_db_path(cli.db, &config)?;
            match args.command {
                DaemonSubcommand::Run => {
                    daemon::run(daemon::DaemonRunArgs { db_path, config }).await
                }
            }
        }
        command => {
            let config = load_config_for_command(cli.db.is_some(), &command)?;
            let db_path = config::resolve_db_path(cli.db, &config)?;
            let pool = open_db(&db_path).await?;
            let mut conn = pool.acquire().await?;
            let should_wake = command_should_wake(&command);
            let result = match command {
                Commands::Add(args) => cmd_add(&mut conn, args).await,
                Commands::Show(args) => cmd_show(&mut conn, args).await,
                Commands::List(args) => cmd_list(&mut conn, args).await,
                Commands::Update(args) => cmd_update(&mut conn, args).await,
                Commands::Note(args) => cmd_note(&mut conn, args).await,
                Commands::Projects(args) => cmd_projects(&mut conn, args).await,
                Commands::Labels(args) => cmd_labels(&mut conn, args).await,
                Commands::Label(args) => cmd_label(&mut conn, args).await,
                Commands::Project(args) => cmd_project(&mut conn, args).await,
                Commands::Delete(args) => cmd_delete_restore(&mut conn, args, true).await,
                Commands::Restore(args) => cmd_delete_restore(&mut conn, args, false).await,
                Commands::Conflict(args) => cmd_conflict(&mut conn, args).await,
                Commands::Sync(args) => sync_client(&mut conn, args, &config).await,
                Commands::Config(_) | Commands::Daemon(_) | Commands::Server(_) => unreachable!(),
            };
            if result.is_ok()
                && should_wake
                && config.sync.enabled
                && let Ok(addr) = config.wake_addr()
            {
                daemon::wake(addr);
            }
            result
        }
    }
}

fn load_config_for_command(db_flag_set: bool, command: &Commands) -> Result<config::AppConfig> {
    if db_flag_set && !matches!(command, Commands::Sync(SyncArgs { server: None, .. })) {
        Ok(config::AppConfig::default())
    } else {
        config::AppConfig::load()
    }
}

fn command_should_wake(command: &Commands) -> bool {
    matches!(
        command,
        Commands::Add(_)
            | Commands::Update(_)
            | Commands::Note(_)
            | Commands::Label(_)
            | Commands::Project(_)
            | Commands::Delete(_)
            | Commands::Restore(_)
            | Commands::Conflict(ConflictCommand {
                command: ConflictSubcommand::Resolve { .. }
            })
    )
}

fn now() -> String {
    let output = Command::new("date")
        .arg("-u")
        .arg("+%Y-%m-%dT%H:%M:%SZ")
        .output();
    output
        .ok()
        .and_then(|out| String::from_utf8(out.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".to_string())
}

fn new_id() -> String {
    let mut bytes = [0u8; 10];
    rand::rng().fill_bytes(&mut bytes);
    encode_crockford(&bytes)
}

fn encode_crockford(bytes: &[u8; 10]) -> String {
    let mut value = u128::from_be_bytes([
        0, 0, 0, 0, 0, 0, bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6],
        bytes[7], bytes[8], bytes[9],
    ]);
    let mut chars = [b'0'; 16];
    for i in (0..16).rev() {
        chars[i] = BASE32[(value & 31) as usize];
        value >>= 5;
    }
    String::from_utf8(chars.to_vec()).expect("base32 is utf8")
}

async fn cmd_add(conn: &mut SqliteConnection, args: AddArgs) -> Result<()> {
    validate_choice("priority", &args.priority, PRIORITIES)?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?
    .unwrap_or_default();
    let id = new_id();
    let ts = now();
    let mut tx = conn.begin().await?;
    let project = resolve_project_for_add(&mut tx, args.project.as_deref()).await?;
    let labels = resolve_labels(&mut tx, &args.label).await?;
    sqlx::query!(
        "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'inbox', ?, ?, ?)",
        id,
        args.title,
        description,
        project.key,
        args.priority,
        ts,
        ts,
    )
    .execute(&mut *tx)
    .await?;
    for label in &labels {
        sqlx::query!(
            "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
            id,
            label,
        )
        .execute(&mut *tx)
        .await?;
    }
    let change_id = insert_change(
        &mut tx,
        "task",
        &id,
        None,
        "create_task",
        json!({
            "title": args.title,
            "description": description,
            "project_key": project.key,
            "project_name": project.name,
            "project_prefix": project.prefix,
            "status": "inbox",
            "priority": args.priority,
            "labels": labels,
            "created_at": ts,
        }),
        None,
    )
    .await?;
    for field in [
        "title",
        "description",
        "project",
        "status",
        "priority",
        "deleted",
    ] {
        set_field_version(&mut tx, &id, field, &change_id).await?;
    }
    tx.commit().await?;
    let task = get_task(conn, &id).await?;
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

async fn cmd_show(conn: &mut SqliteConnection, args: ShowArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    print_task(conn, &task, args.full).await
}

async fn cmd_list(conn: &mut SqliteConnection, args: ListArgs) -> Result<()> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "SELECT t.id, t.title, t.description, t.project_key, p.prefix, t.status, t.priority,
         t.created_at, t.updated_at, t.deleted
         FROM tasks t JOIN projects p ON p.key = t.project_key",
    );
    let mut filters = 0;
    if !args.all {
        push_filter_prefix(&mut query, &mut filters);
        query.push("t.deleted = 0");
    }
    let project_key = if let Some(project) = args.project.as_deref() {
        Some(resolve_existing_project(conn, project).await?.key)
    } else {
        None
    };
    if let Some(project_key) = project_key {
        push_filter_prefix(&mut query, &mut filters);
        query.push("t.project_key = ");
        query.push_bind(project_key);
    }
    if let Some(status) = args.status.as_deref() {
        validate_choice("status", status, STATUSES)?;
        push_filter_prefix(&mut query, &mut filters);
        query.push("t.status = ");
        query.push_bind(status.to_string());
    }
    if let Some(priority) = args.priority.as_deref() {
        validate_choice("priority", priority, PRIORITIES)?;
        push_filter_prefix(&mut query, &mut filters);
        query.push("t.priority = ");
        query.push_bind(priority.to_string());
    }
    if let Some(label) = args.label.as_deref() {
        let label = ensure_label_exists(conn, label).await?;
        push_filter_prefix(&mut query, &mut filters);
        query.push("EXISTS (SELECT 1 FROM task_labels tl WHERE tl.task_id = t.id AND tl.label = ");
        query.push_bind(label);
        query.push(")");
    }
    query.push(" ORDER BY t.updated_at DESC, t.created_at DESC");

    let rows = query.build().fetch_all(&mut *conn).await?;
    for row in rows {
        print_task_line(conn, &task_from_row(&row)?).await?;
    }
    Ok(())
}

fn push_filter_prefix(query: &mut QueryBuilder<'_, Sqlite>, filters: &mut usize) {
    if *filters == 0 {
        query.push(" WHERE ");
    } else {
        query.push(" AND ");
    }
    *filters += 1;
}

async fn cmd_update(conn: &mut SqliteConnection, args: UpdateArgs) -> Result<()> {
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
    let mut changed = Vec::new();
    let mut tx = conn.begin().await?;
    if let Some(title) = args.title {
        set_task_field(&mut tx, &task.id, "title", &title).await?;
        changed.push("title");
    }
    if let Some(description) = description {
        set_task_field(&mut tx, &task.id, "description", &description).await?;
        changed.push("description");
    }
    if let Some(project) = args.project {
        let project = resolve_project_for_add(&mut tx, Some(&project)).await?;
        set_task_field(&mut tx, &task.id, "project", &project.key).await?;
        changed.push("project");
    }
    if let Some(status) = args.status {
        set_task_field(&mut tx, &task.id, "status", &status).await?;
        changed.push("status");
    }
    if let Some(priority) = args.priority {
        set_task_field(&mut tx, &task.id, "priority", &priority).await?;
        changed.push("priority");
    }
    for label in resolve_labels(&mut tx, &args.label).await? {
        sqlx::query!(
            "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
            task.id,
            label,
        )
        .execute(&mut *tx)
        .await?;
        insert_change(
            &mut tx,
            "task",
            &task.id,
            Some("labels"),
            "label_add",
            json!({ "label": label }),
            None,
        )
        .await?;
        changed.push("label");
    }
    for label in resolve_labels(&mut tx, &args.remove_label).await? {
        sqlx::query!(
            "DELETE FROM task_labels WHERE task_id = ? AND label = ?",
            task.id,
            label,
        )
        .execute(&mut *tx)
        .await?;
        insert_change(
            &mut tx,
            "task",
            &task.id,
            Some("labels"),
            "label_remove",
            json!({ "label": label }),
            None,
        )
        .await?;
        changed.push("label");
    }
    tx.commit().await?;
    let task = get_task(conn, &task.id).await?;
    println!(
        "updated {} changed={} status={} priority={} title={}",
        display_ref(conn, &task).await?,
        if changed.is_empty() { "none" } else { "yes" },
        task.status,
        task.priority,
        quote(&task.title)
    );
    Ok(())
}

async fn cmd_note(conn: &mut SqliteConnection, args: NoteArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    let body = read_required_text(args.text, args.file.as_deref(), args.stdin, "note")?;
    let note_id = new_id();
    let ts = now();
    let mut tx = conn.begin().await?;
    let change_id = insert_change(
        &mut tx,
        "task",
        &task.id,
        Some("notes"),
        "note_add",
        json!({ "note_id": note_id, "body": body, "created_at": ts }),
        None,
    )
    .await?;
    sqlx::query!(
        "INSERT INTO notes(id, task_id, body, created_at, change_id) VALUES (?, ?, ?, ?, ?)",
        note_id,
        task.id,
        body,
        ts,
        change_id,
    )
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    println!("noted {} note={}", display_ref(conn, &task).await?, note_id);
    Ok(())
}

async fn cmd_projects(conn: &mut SqliteConnection, args: SearchArgs) -> Result<()> {
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

async fn cmd_labels(conn: &mut SqliteConnection, args: SearchArgs) -> Result<()> {
    let labels = list_labels(conn, args.search.as_deref()).await?;
    for label in labels {
        println!("{label}");
    }
    Ok(())
}

async fn cmd_label(conn: &mut SqliteConnection, args: LabelCommand) -> Result<()> {
    match args.command {
        LabelSubcommand::Create { name } => {
            let name = normalize_label(&name);
            if name.is_empty() {
                bail!("error invalid-label");
            }
            let created_at = now();
            sqlx::query!(
                "INSERT OR IGNORE INTO labels(name, created_at) VALUES (?, ?)",
                name,
                created_at,
            )
            .execute(&mut *conn)
            .await?;
            insert_change(
                conn,
                "label",
                &name,
                None,
                "create_label",
                json!({ "name": name, "created_at": created_at }),
                None,
            )
            .await?;
            println!("created-label {name}");
        }
    }
    Ok(())
}

async fn cmd_project(conn: &mut SqliteConnection, args: ProjectCommand) -> Result<()> {
    match args.command {
        ProjectSubcommand::Create { name, path } => {
            let project = create_project(conn, &name).await?;
            if let Some(path) = path {
                add_project_path(conn, &project.key, &path).await?;
            }
            println!(
                "created-project {} prefix={} name={}",
                project.key,
                project.prefix,
                quote(&project.name)
            );
        }
        ProjectSubcommand::Path { command } => match command {
            ProjectPathSubcommand::Add { project, path } => {
                let project = resolve_existing_project(conn, &project).await?;
                add_project_path(conn, &project.key, &path).await?;
                println!(
                    "added-project-path {} path={}",
                    project.key,
                    quote(&path.display().to_string())
                );
            }
            ProjectPathSubcommand::Remove { project, path } => {
                let project = resolve_existing_project(conn, &project).await?;
                let path_display = path.display().to_string();
                sqlx::query!(
                    "DELETE FROM project_paths WHERE project_key = ? AND path = ?",
                    project.key,
                    path_display,
                )
                .execute(&mut *conn)
                .await?;
                println!(
                    "removed-project-path {} path={}",
                    project.key,
                    quote(&path.display().to_string())
                );
            }
        },
    }
    Ok(())
}

async fn cmd_delete_restore(
    conn: &mut SqliteConnection,
    args: RefArgs,
    delete: bool,
) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref).await?;
    set_task_field(conn, &task.id, "deleted", if delete { "1" } else { "0" }).await?;
    let task = get_task(conn, &task.id).await?;
    if delete {
        println!("deleted {}", display_ref(conn, &task).await?);
    } else {
        println!("restored {}", display_ref(conn, &task).await?);
    }
    Ok(())
}

async fn cmd_config(args: ConfigCommand) -> Result<()> {
    match args.command {
        ConfigSubcommand::Init => {
            let path = config::config_file_path()?;
            config::write_default_config(&path)?;
            println!("created-config path={}", quote(&path.display().to_string()));
        }
        ConfigSubcommand::Show => {
            let path = config::config_file_path()?;
            let config = config::AppConfig::load()?;
            println!("config path={}", quote(&path.display().to_string()));
            println!("{}", toml::to_string_pretty(&config)?);
        }
    }
    Ok(())
}

async fn cmd_conflict(conn: &mut SqliteConnection, args: ConflictCommand) -> Result<()> {
    match args.command {
        ConflictSubcommand::List { project, field } => {
            let project_key = if let Some(project) = project {
                Some(resolve_existing_project(conn, &project).await?.key)
            } else {
                None
            };
            let rows = sqlx::query!(
                r#"SELECT c.task_id AS "task_id!: String", c.field AS "field!: String",
                 c.variant_a AS "variant_a!: String", c.variant_b AS "variant_b!: String",
                 t.title AS "title!: String", p.prefix AS "prefix!: String",
                 t.project_key AS "project_key!: String"
                 FROM conflicts c
                 JOIN tasks t ON t.id = c.task_id
                 JOIN projects p ON p.key = t.project_key
                 WHERE c.resolved = 0
                 AND (?1 IS NULL OR t.project_key = ?1)
                 AND (?2 IS NULL OR c.field = ?2)
                 ORDER BY c.created_at"#,
                project_key,
                field,
            )
            .fetch_all(&mut *conn)
            .await?;
            for row in rows {
                let task = Task {
                    id: row.task_id,
                    title: row.title,
                    description: String::new(),
                    project_key: row.project_key,
                    project_prefix: row.prefix,
                    status: String::new(),
                    priority: String::new(),
                    created_at: String::new(),
                    updated_at: String::new(),
                    deleted: false,
                };
                println!(
                    "{} conflict field={} variants={},{} title={}",
                    display_ref(conn, &task).await?,
                    row.field,
                    row.variant_a,
                    row.variant_b,
                    quote(&task.title)
                );
            }
        }
        ConflictSubcommand::Show { task_ref, field } => {
            let task = resolve_task_ref(conn, &task_ref).await?;
            print_conflicts(conn, &task, field.as_deref()).await?;
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
            let mut tx = conn.begin().await?;
            apply_field_value(&mut tx, &task.id, &field, &value).await?;
            sqlx::query!(
                "UPDATE conflicts SET resolved = 1 WHERE task_id = ? AND field = ? AND resolved = 0",
                task.id,
                field,
            )
            .execute(&mut *tx)
            .await?;
            let change_id = insert_change(
                &mut tx,
                "task",
                &task.id,
                Some(&field),
                "resolve_field",
                json!({ "value": value }),
                None,
            )
            .await?;
            set_field_version(&mut tx, &task.id, &field, &change_id).await?;
            tx.commit().await?;
            let task = get_task(conn, &task.id).await?;
            println!(
                "resolved {} field={}",
                display_ref(conn, &task).await?,
                field
            );
        }
    }
    Ok(())
}

async fn set_task_field(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    if conflict_exists(conn, task_id, field).await? {
        bail!(
            "error conflicted-field ref={} field={} hint=\"use conflict resolve\"",
            task_id,
            field
        );
    }
    let base = field_version(conn, task_id, field).await?;
    apply_field_value(conn, task_id, field, value).await?;
    let change_id = insert_change(
        conn,
        "task",
        task_id,
        Some(field),
        "set_field",
        json!({ "value": value }),
        base.as_deref(),
    )
    .await?;
    set_field_version(conn, task_id, field, &change_id).await?;
    Ok(())
}

async fn apply_field_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    value: &str,
) -> Result<()> {
    let ts = now();
    let deleted_value = value.parse::<i64>().unwrap_or(0);
    match field {
        "title" => sqlx::query!(
            "UPDATE tasks SET title = ?, updated_at = ? WHERE id = ?",
            value,
            ts,
            task_id,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        "description" => sqlx::query!(
            "UPDATE tasks SET description = ?, updated_at = ? WHERE id = ?",
            value,
            ts,
            task_id,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        "project" => {
            let project = resolve_project_for_add(conn, Some(value)).await?;
            sqlx::query!(
                "UPDATE tasks SET project_key = ?, updated_at = ? WHERE id = ?",
                project.key,
                ts,
                task_id,
            )
            .execute(&mut *conn)
            .await?
            .rows_affected()
        }
        "status" => {
            validate_choice("status", value, STATUSES)?;
            sqlx::query!(
                "UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?",
                value,
                ts,
                task_id,
            )
            .execute(&mut *conn)
            .await?
            .rows_affected()
        }
        "priority" => {
            validate_choice("priority", value, PRIORITIES)?;
            sqlx::query!(
                "UPDATE tasks SET priority = ?, updated_at = ? WHERE id = ?",
                value,
                ts,
                task_id,
            )
            .execute(&mut *conn)
            .await?
            .rows_affected()
        }
        "deleted" => sqlx::query!(
            "UPDATE tasks SET deleted = ?, updated_at = ? WHERE id = ?",
            deleted_value,
            ts,
            task_id,
        )
        .execute(&mut *conn)
        .await?
        .rows_affected(),
        _ => bail!("error unknown-field field={field}"),
    };
    Ok(())
}

fn read_optional_text(
    inline: Option<String>,
    file: Option<&Path>,
    stdin_flag: bool,
    name: &str,
) -> Result<Option<String>> {
    let count = inline.is_some() as u8 + file.is_some() as u8 + stdin_flag as u8;
    if count > 1 {
        bail!("error multiple-{name}-sources");
    }
    if let Some(text) = inline {
        Ok(Some(text))
    } else if let Some(path) = file {
        Ok(Some(fs::read_to_string(path).with_context(|| {
            format!("could not read {}", path.display())
        })?))
    } else if stdin_flag {
        let mut text = String::new();
        io::stdin().read_to_string(&mut text)?;
        Ok(Some(text))
    } else {
        Ok(None)
    }
}

fn read_required_text(
    inline: Option<String>,
    file: Option<&Path>,
    stdin_flag: bool,
    name: &str,
) -> Result<String> {
    read_optional_text(inline, file, stdin_flag, name)?
        .with_context(|| format!("error missing-{name}"))
}

fn validate_choice(name: &str, value: &str, choices: &[&str]) -> Result<()> {
    if choices.contains(&value) {
        Ok(())
    } else {
        bail!(
            "error invalid-{name} input={} choices={}",
            value,
            choices.join(",")
        );
    }
}

#[derive(Debug, Clone)]
struct Project {
    key: String,
    name: String,
    prefix: String,
}

fn normalize_key(input: &str) -> String {
    let mut out = String::new();
    let mut last_dash = false;
    for ch in input.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
            last_dash = false;
        } else if !last_dash && !out.is_empty() {
            out.push('-');
            last_dash = true;
        }
    }
    out.trim_matches('-').to_string()
}

fn normalize_label(input: &str) -> String {
    normalize_key(input)
}

async fn resolve_project_for_add(
    conn: &mut SqliteConnection,
    project: Option<&str>,
) -> Result<Project> {
    if let Some(project) = project {
        if let Some(existing) = find_project(conn, project).await? {
            return Ok(existing);
        }
        let choices = near_projects(conn, project).await?;
        if !choices.is_empty() {
            print_near_error("project", project, &choices);
            bail!("near-match project");
        }
        return create_project(conn, project).await;
    }
    if let Some(project) = project_from_path_mapping(conn).await? {
        return Ok(project);
    }
    if let Some(root_name) = git_root_name()? {
        if let Some(existing) = find_project(conn, &root_name).await? {
            return Ok(existing);
        }
        let choices = near_projects(conn, &root_name).await?;
        if !choices.is_empty() {
            print_near_error("project", &root_name, &choices);
            bail!("near-match project");
        }
        return create_project(conn, &root_name).await;
    }
    bail!("error project-required");
}

async fn resolve_existing_project(conn: &mut SqliteConnection, project: &str) -> Result<Project> {
    if let Some(project) = find_project(conn, project).await? {
        return Ok(project);
    }
    let choices = near_projects(conn, project).await?;
    if !choices.is_empty() {
        print_near_error("project", project, &choices);
    } else {
        eprintln!("error unknown-project input={}", project);
    }
    bail!("unknown project");
}

async fn find_project(conn: &mut SqliteConnection, input: &str) -> Result<Option<Project>> {
    let key = normalize_key(input);
    let row = sqlx::query!(
        r#"SELECT key AS "key!: String", name AS "name!: String", prefix AS "prefix!: String"
         FROM projects
         WHERE deleted = 0 AND (key = ? OR lower(name) = lower(?))"#,
        key,
        input,
    )
    .fetch_optional(&mut *conn)
    .await?;
    Ok(row.map(|row| Project {
        key: row.key,
        name: row.name,
        prefix: row.prefix,
    }))
}

async fn create_project(conn: &mut SqliteConnection, name: &str) -> Result<Project> {
    let key = normalize_key(name);
    if key.is_empty() {
        bail!("error invalid-project input={}", quote(name));
    }
    if let Some(project) = find_project(conn, &key).await? {
        return Ok(project);
    }
    let prefix = unique_project_prefix(conn, &key).await?;
    let ts = now();
    sqlx::query!(
        "INSERT INTO projects(key, name, prefix, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        key,
        name,
        prefix,
        ts,
        ts,
    )
    .execute(&mut *conn)
    .await?;
    insert_change(
        conn,
        "project",
        &key,
        None,
        "create_project",
        json!({ "key": key, "name": name, "prefix": prefix, "created_at": ts }),
        None,
    )
    .await?;
    Ok(Project {
        key,
        name: name.to_string(),
        prefix,
    })
}

async fn unique_project_prefix(conn: &mut SqliteConnection, key: &str) -> Result<String> {
    let base = prefix_base(key);
    let mut candidate = base.clone();
    let mut n = 2;
    while sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!: i64" FROM projects WHERE prefix = ?"#,
        candidate
    )
    .fetch_one(&mut *conn)
    .await?
        > 0
    {
        candidate = format!("{}{}", base.chars().take(2).collect::<String>(), n);
        n += 1;
    }
    Ok(candidate)
}

fn prefix_base(key: &str) -> String {
    let words: Vec<&str> = key.split('-').filter(|word| !word.is_empty()).collect();
    if words.len() >= 2 {
        return words
            .iter()
            .filter_map(|word| word.chars().next())
            .take(3)
            .collect::<String>()
            .to_ascii_uppercase();
    }
    let key = words.first().copied().unwrap_or(key);
    let mut out = String::new();
    let mut chars = key.chars();
    if let Some(first) = chars.next() {
        out.push(first);
    }
    for ch in chars {
        if !"aeiou".contains(ch) {
            out.push(ch);
        }
        if out.len() >= 3 {
            break;
        }
    }
    for ch in key.chars() {
        if out.len() >= 3 {
            break;
        }
        if !out.contains(ch) {
            out.push(ch);
        }
    }
    while out.len() < 3 {
        out.push('X');
    }
    out.to_ascii_uppercase()
}

async fn project_from_path_mapping(conn: &mut SqliteConnection) -> Result<Option<Project>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let rows = sqlx::query!(
        r#"SELECT p.key AS "key!: String", p.name AS "name!: String",
         p.prefix AS "prefix!: String", pp.path AS "path!: String"
         FROM project_paths pp JOIN projects p ON p.key = pp.project_key
         ORDER BY length(pp.path) DESC"#,
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        let project = Project {
            key: row.key,
            name: row.name,
            prefix: row.prefix,
        };
        if cwd.starts_with(Path::new(&row.path)) {
            return Ok(Some(project));
        }
    }
    Ok(None)
}

fn git_root_name() -> Result<Option<String>> {
    let output = Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output();
    let Ok(output) = output else {
        return Ok(None);
    };
    if !output.status.success() {
        return Ok(None);
    }
    let root = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(Path::new(&root)
        .file_name()
        .map(|name| name.to_string_lossy().to_string()))
}

async fn add_project_path(
    conn: &mut SqliteConnection,
    project_key: &str,
    path: &Path,
) -> Result<()> {
    let path =
        fs::canonicalize(path).with_context(|| format!("could not resolve {}", path.display()))?;
    let path = path.display().to_string();
    sqlx::query!(
        "INSERT OR IGNORE INTO project_paths(project_key, path) VALUES (?, ?)",
        project_key,
        path,
    )
    .execute(&mut *conn)
    .await?;
    Ok(())
}

async fn near_projects(conn: &mut SqliteConnection, input: &str) -> Result<Vec<String>> {
    let needle = normalize_key(input);
    let projects = list_projects(conn, None).await?;
    Ok(projects
        .into_iter()
        .filter(|project| is_near(&needle, &project.key))
        .map(|project| {
            format!(
                "{} prefix={} name={}",
                project.key,
                project.prefix,
                quote(&project.name)
            )
        })
        .collect())
}

async fn near_labels(conn: &mut SqliteConnection, input: &str) -> Result<Vec<String>> {
    let needle = normalize_label(input);
    Ok(list_labels(conn, None)
        .await?
        .into_iter()
        .filter(|label| is_near(&needle, label))
        .collect())
}

fn print_near_error(kind: &str, input: &str, choices: &[String]) {
    eprintln!("error unknown-{kind} input={}", input);
    for choice in choices {
        eprintln!("choice {choice}");
    }
    eprintln!("hint \"retry with an exact {kind} or create it explicitly\"");
}

fn is_near(a: &str, b: &str) -> bool {
    a.contains(b) || b.contains(a) || levenshtein(a, b) <= 2
}

fn levenshtein(a: &str, b: &str) -> usize {
    let mut costs: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut prev = i;
        costs[0] = i + 1;
        for (j, cb) in b.chars().enumerate() {
            let old = costs[j + 1];
            costs[j + 1] = if ca == cb {
                prev
            } else {
                1 + prev.min(costs[j]).min(costs[j + 1])
            };
            prev = old;
        }
    }
    costs[b.len()]
}

async fn list_projects(conn: &mut SqliteConnection, search: Option<&str>) -> Result<Vec<Project>> {
    let search = search.map(normalize_key);
    let rows = sqlx::query!(
        r#"SELECT key AS "key!: String", name AS "name!: String", prefix AS "prefix!: String"
         FROM projects
         WHERE deleted = 0
         ORDER BY key"#,
    )
    .fetch_all(&mut *conn)
    .await?;
    let projects = rows
        .into_iter()
        .map(|row| Project {
            key: row.key,
            name: row.name,
            prefix: row.prefix,
        })
        .collect::<Vec<_>>();
    Ok(projects
        .into_iter()
        .filter(|project| {
            search.as_deref().is_none_or(|search| {
                project.key.contains(search) || project.name.to_lowercase().contains(search)
            })
        })
        .collect())
}

async fn list_labels(conn: &mut SqliteConnection, search: Option<&str>) -> Result<Vec<String>> {
    let search = search.map(normalize_label);
    let labels = sqlx::query_scalar!(r#"SELECT name AS "name!: String" FROM labels ORDER BY name"#)
        .fetch_all(&mut *conn)
        .await?;
    Ok(labels
        .into_iter()
        .filter(|label| {
            search
                .as_deref()
                .is_none_or(|search| label.contains(search))
        })
        .collect())
}

async fn ensure_label_exists(conn: &mut SqliteConnection, label: &str) -> Result<String> {
    let label = normalize_label(label);
    if sqlx::query_scalar!(
        r#"SELECT count(*) AS "count!: i64" FROM labels WHERE name = ?"#,
        label
    )
    .fetch_one(&mut *conn)
    .await?
        > 0
    {
        Ok(label)
    } else {
        let choices = near_labels(conn, &label).await?;
        eprintln!("error unknown-label input={}", label);
        for choice in choices {
            eprintln!("choice {choice}");
        }
        eprintln!("hint \"create the label explicitly\"");
        bail!("unknown label");
    }
}

async fn resolve_labels(conn: &mut SqliteConnection, labels: &[String]) -> Result<Vec<String>> {
    let mut resolved = Vec::with_capacity(labels.len());
    for label in labels {
        resolved.push(ensure_label_exists(conn, label).await?);
    }
    Ok(resolved)
}

async fn get_task(conn: &mut SqliteConnection, id: &str) -> Result<Task> {
    let row = sqlx::query!(
        r#"SELECT t.id AS "id!: String", t.title AS "title!: String",
         t.description AS "description!: String", t.project_key AS "project_key!: String",
         p.prefix AS "project_prefix!: String", t.status AS "status!: String",
         t.priority AS "priority!: String", t.created_at AS "created_at!: String",
         t.updated_at AS "updated_at!: String", t.deleted AS "deleted!: i64"
         FROM tasks t JOIN projects p ON p.key = t.project_key
         WHERE t.id = ?"#,
        id,
    )
    .fetch_one(&mut *conn)
    .await?;
    Ok(Task {
        id: row.id,
        title: row.title,
        description: row.description,
        project_key: row.project_key,
        project_prefix: row.project_prefix,
        status: row.status,
        priority: row.priority,
        created_at: row.created_at,
        updated_at: row.updated_at,
        deleted: row.deleted != 0,
    })
}

async fn resolve_task_ref(conn: &mut SqliteConnection, input: &str) -> Result<Task> {
    let (hint, suffix) = split_ref(input);
    if suffix.len() < 3 {
        bail!("error ref-too-short input={} minimum=3", input);
    }
    let suffix = suffix.to_ascii_uppercase();
    let rows = sqlx::query!(
        r#"SELECT t.id AS "id!: String", t.title AS "title!: String",
         t.description AS "description!: String", t.project_key AS "project_key!: String",
         p.prefix AS "project_prefix!: String", t.status AS "status!: String",
         t.priority AS "priority!: String", t.created_at AS "created_at!: String",
         t.updated_at AS "updated_at!: String", t.deleted AS "deleted!: i64"
         FROM tasks t JOIN projects p ON p.key = t.project_key
         WHERE t.id LIKE ? || '%'
         ORDER BY t.id"#,
        suffix,
    )
    .fetch_all(&mut *conn)
    .await?;
    let matches = rows
        .into_iter()
        .map(|row| Task {
            id: row.id,
            title: row.title,
            description: row.description,
            project_key: row.project_key,
            project_prefix: row.project_prefix,
            status: row.status,
            priority: row.priority,
            created_at: row.created_at,
            updated_at: row.updated_at,
            deleted: row.deleted != 0,
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        bail!("error unknown-ref input={}", input);
    }
    if let Some(hint) = hint {
        let hinted: Vec<Task> = matches
            .iter()
            .filter(|task| task.project_prefix.eq_ignore_ascii_case(&hint))
            .cloned()
            .collect();
        if hinted.len() == 1 {
            return Ok(hinted[0].clone());
        }
    }
    if matches.len() == 1 {
        return Ok(matches[0].clone());
    }
    println!("error ambiguous-ref input={}", input);
    for task in matches {
        println!(
            "match {} title={}",
            display_ref(conn, &task).await?,
            quote(&task.title)
        );
    }
    println!("hint \"retry with longer ref\"");
    bail!("ambiguous ref");
}

fn split_ref(input: &str) -> (Option<String>, String) {
    if let Some((prefix, suffix)) = input.split_once('-') {
        (Some(prefix.to_string()), normalize_ref(suffix))
    } else {
        (None, normalize_ref(input))
    }
}

fn normalize_ref(input: &str) -> String {
    input
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| match ch.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            ch => ch,
        })
        .collect()
}

async fn display_ref(conn: &mut SqliteConnection, task: &Task) -> Result<String> {
    Ok(format!(
        "{}-{}",
        task.project_prefix,
        display_suffix(conn, &task.id).await?
    ))
}

async fn display_suffix(conn: &mut SqliteConnection, id: &str) -> Result<String> {
    for len in 7..=16 {
        let prefix = &id[..len];
        let count: i64 = sqlx::query_scalar!(
            r#"SELECT count(*) AS "count!: i64" FROM tasks WHERE id LIKE ? || '%'"#,
            prefix
        )
        .fetch_one(&mut *conn)
        .await?;
        if count <= 1 {
            return Ok(prefix.to_string());
        }
    }
    Ok(id.to_string())
}

async fn labels_for_task(conn: &mut SqliteConnection, task_id: &str) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar!(
        r#"SELECT label AS "label!: String" FROM task_labels WHERE task_id = ? ORDER BY label"#,
        task_id
    )
    .fetch_all(&mut *conn)
    .await?)
}

async fn print_task_line(conn: &mut SqliteConnection, task: &Task) -> Result<()> {
    let labels = labels_for_task(conn, &task.id).await?.join(",");
    let conflict = if task_has_conflict(conn, &task.id).await? {
        " conflicts=yes"
    } else {
        ""
    };
    let deleted = if task.deleted { " deleted=yes" } else { "" };
    println!(
        "{} status={} priority={} labels={}{}{} title={}",
        display_ref(conn, task).await?,
        task.status,
        task.priority,
        labels,
        conflict,
        deleted,
        quote(&task.title)
    );
    Ok(())
}

async fn print_task(conn: &mut SqliteConnection, task: &Task, full: bool) -> Result<()> {
    print_task_line(conn, task).await?;
    if full {
        println!("id={}", task.id);
        println!(
            "project={} prefix={}",
            task.project_key, task.project_prefix
        );
        println!("created={} updated={}", task.created_at, task.updated_at);
        if !task.description.is_empty() {
            println!("description<<EOF");
            print!("{}", task.description);
            if !task.description.ends_with('\n') {
                println!();
            }
            println!("EOF");
        }
        let notes = sqlx::query!(
            r#"SELECT body AS "body!: String", created_at AS "created_at!: String"
             FROM notes WHERE task_id = ? ORDER BY created_at, id"#,
            task.id,
        )
        .fetch_all(&mut *conn)
        .await?;
        for note in notes {
            println!(
                "note created={} body={}",
                note.created_at,
                quote(&note.body)
            );
        }
        print_conflicts(conn, task, None).await?;
    }
    Ok(())
}

fn quote(input: &str) -> String {
    serde_json::to_string(input).unwrap_or_else(|_| "\"\"".to_string())
}

pub(crate) async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        if let Ok(mut signal) =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        {
            signal.recv().await;
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

async fn print_conflicts(
    conn: &mut SqliteConnection,
    task: &Task,
    field: Option<&str>,
) -> Result<()> {
    let rows = sqlx::query!(
        r#"SELECT field AS "field!: String", variant_a AS "variant_a!: String",
         local_value AS "local_value!: String", variant_b AS "variant_b!: String",
         remote_value AS "remote_value!: String"
         FROM conflicts
         WHERE task_id = ? AND resolved = 0 AND (? IS NULL OR field = ?)
         ORDER BY field, id"#,
        task.id,
        field,
        field,
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        println!(
            "conflict {} field={}",
            display_ref(conn, task).await?,
            row.field
        );
        println!(
            "variant {} value={}",
            row.variant_a,
            quote(&row.local_value)
        );
        println!(
            "variant {} value={}",
            row.variant_b,
            quote(&row.remote_value)
        );
    }
    Ok(())
}

async fn conflict_variant_value(
    conn: &mut SqliteConnection,
    task_id: &str,
    field: &str,
    token: &str,
) -> Result<String> {
    let rows = sqlx::query!(
        r#"SELECT variant_a AS "variant_a!: String", local_value AS "local_value!: String",
         variant_b AS "variant_b!: String", remote_value AS "remote_value!: String"
         FROM conflicts WHERE task_id = ? AND field = ? AND resolved = 0"#,
        task_id,
        field,
    )
    .fetch_all(&mut *conn)
    .await?;
    for row in rows {
        if token == row.variant_a {
            return Ok(row.local_value);
        }
        if token == row.variant_b {
            return Ok(row.remote_value);
        }
    }
    bail!("error unknown-variant token={}", token);
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_conn() -> (tempfile::TempDir, sqlx::pool::PoolConnection<Sqlite>) {
        let temp = tempfile::tempdir().unwrap();
        let pool = open_db(&temp.path().join("test.sqlite")).await.unwrap();
        let conn = pool.acquire().await.unwrap();
        (temp, conn)
    }

    #[test]
    fn normalizes_project_keys() {
        assert_eq!(
            normalize_key("Agentic Task Manager"),
            "agentic-task-manager"
        );
    }

    #[test]
    fn encodes_80_bit_ids_as_16_chars() {
        let id = encode_crockford(&[0xff; 10]);
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|ch| BASE32.contains(&(ch as u8))));
    }

    #[tokio::test]
    async fn resolves_short_refs_when_unambiguous() {
        let (_temp, mut conn) = test_conn().await;
        let project = create_project(&mut conn, "app").await.unwrap();
        sqlx::query!(
            "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', 'test', '', ?, 'inbox', 'none', 't', 't')",
            project.key,
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        let task = resolve_task_ref(&mut conn, "7KQ").await.unwrap();
        assert_eq!(task.id, "7KQ9A1X4MV2P8D6R");
    }

    #[tokio::test]
    async fn rejects_ambiguous_refs() {
        let (_temp, mut conn) = test_conn().await;
        let project = create_project(&mut conn, "app").await.unwrap();
        for id in ["7KQ9A1X4MV2P8D6R", "7KQZZZZZZZZZZZZZ"] {
            sqlx::query!(
                "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
                 VALUES (?, 'test', '', ?, 'inbox', 'none', 't', 't')",
                id,
                project.key,
            )
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        assert!(resolve_task_ref(&mut conn, "7KQ").await.is_err());
    }

    #[tokio::test]
    async fn creates_conflict_on_same_field_version_mismatch() {
        let (_temp, mut conn) = test_conn().await;
        let project = create_project(&mut conn, "app").await.unwrap();
        sqlx::query!(
            "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', 'local', '', ?, 'inbox', 'none', 't', 't')",
            project.key,
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        set_field_version(&mut conn, "7KQ9A1X4MV2P8D6R", "title", "localchange")
            .await
            .unwrap();
        let change = sync::ChangeWire {
            change_id: "remotechange1234".to_string(),
            client_id: "remote".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: "7KQ9A1X4MV2P8D6R".to_string(),
            field: Some("title".to_string()),
            op_type: "set_field".to_string(),
            payload: json!({ "value": "remote" }),
            base_version: Some("base".to_string()),
            created_at: "t".to_string(),
            server_seq: Some(1),
        };
        sync::apply_remote_set_field(&mut conn, &change, false)
            .await
            .unwrap();
        assert!(
            conflict_exists(&mut conn, "7KQ9A1X4MV2P8D6R", "title")
                .await
                .unwrap()
        );
    }
}
