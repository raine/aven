use std::env;
use std::fs;
use std::io::{self, Read};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use axum::Json;
use axum::Router;
use axum::extract::State;
use axum::routing::post;
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{Args, Parser, Subcommand};
use rand::RngCore;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default());

const STATUSES: &[&str] = &["inbox", "backlog", "todo", "active", "done", "canceled"];
const PRIORITIES: &[&str] = &["none", "low", "medium", "high", "urgent"];
const BASE32: &[u8] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Parser)]
#[command(name = "atm")]
#[command(about = "Local-first task manager")]
#[command(styles = STYLES)]
pub struct Cli {
    #[arg(long, global = true)]
    db: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Add(AddArgs),
    Show(ShowArgs),
    List(ListArgs),
    Update(UpdateArgs),
    Note(NoteArgs),
    Projects(SearchArgs),
    Labels(SearchArgs),
    Label(LabelCommand),
    Project(ProjectCommand),
    Delete(RefArgs),
    Restore(RefArgs),
    Conflict(ConflictCommand),
    Server(ServerArgs),
    Sync(SyncArgs),
}

#[derive(Args)]
struct AddArgs {
    title: String,
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    description_file: Option<PathBuf>,
    #[arg(long)]
    description_stdin: bool,
    #[arg(long, default_value = "none")]
    priority: String,
    #[arg(long)]
    label: Vec<String>,
}

#[derive(Args)]
struct ShowArgs {
    task_ref: String,
    #[arg(long)]
    full: bool,
}

#[derive(Args)]
struct ListArgs {
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    priority: Option<String>,
    #[arg(long)]
    label: Option<String>,
    #[arg(long)]
    all: bool,
}

#[derive(Args)]
struct UpdateArgs {
    task_ref: String,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    description: Option<String>,
    #[arg(long)]
    description_file: Option<PathBuf>,
    #[arg(long)]
    description_stdin: bool,
    #[arg(long)]
    project: Option<String>,
    #[arg(long)]
    status: Option<String>,
    #[arg(long)]
    priority: Option<String>,
    #[arg(long)]
    label: Vec<String>,
    #[arg(long)]
    remove_label: Vec<String>,
}

#[derive(Args)]
struct NoteArgs {
    task_ref: String,
    text: Option<String>,
    #[arg(long)]
    file: Option<PathBuf>,
    #[arg(long)]
    stdin: bool,
}

#[derive(Args)]
struct SearchArgs {
    #[arg(long)]
    search: Option<String>,
}

#[derive(Args)]
struct RefArgs {
    task_ref: String,
}

#[derive(Args)]
struct LabelCommand {
    #[command(subcommand)]
    command: LabelSubcommand,
}

#[derive(Subcommand)]
enum LabelSubcommand {
    Create { name: String },
}

#[derive(Args)]
struct ProjectCommand {
    #[command(subcommand)]
    command: ProjectSubcommand,
}

#[derive(Subcommand)]
enum ProjectSubcommand {
    Create {
        name: String,
        #[arg(long)]
        path: Option<PathBuf>,
    },
    Path {
        #[command(subcommand)]
        command: ProjectPathSubcommand,
    },
}

#[derive(Subcommand)]
enum ProjectPathSubcommand {
    Add { project: String, path: PathBuf },
    Remove { project: String, path: PathBuf },
}

#[derive(Args)]
struct ConflictCommand {
    #[command(subcommand)]
    command: ConflictSubcommand,
}

#[derive(Subcommand)]
enum ConflictSubcommand {
    List {
        #[arg(long)]
        project: Option<String>,
        #[arg(long)]
        field: Option<String>,
    },
    Show {
        task_ref: String,
        #[arg(long)]
        field: Option<String>,
    },
    Resolve {
        task_ref: String,
        field: String,
        #[arg(long)]
        #[arg(long = "use")]
        use_variant: Option<String>,
        #[arg(long)]
        value: Option<String>,
        #[arg(long)]
        value_file: Option<PathBuf>,
        #[arg(long)]
        value_stdin: bool,
    },
}

#[derive(Args)]
struct ServerArgs {
    #[arg(long, default_value = "127.0.0.1:0")]
    bind: SocketAddr,
    #[arg(long)]
    data: PathBuf,
    #[arg(long)]
    unsafe_public_bind: bool,
}

#[derive(Args)]
struct SyncArgs {
    #[arg(long)]
    server: String,
}

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

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChangeWire {
    change_id: String,
    client_id: String,
    local_seq: i64,
    entity_type: String,
    entity_id: String,
    field: Option<String>,
    op_type: String,
    payload: Value,
    base_version: Option<String>,
    created_at: String,
    server_seq: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncRequest {
    client_id: String,
    after: i64,
    changes: Vec<ChangeWire>,
}

#[derive(Debug, Serialize, Deserialize)]
struct SyncResponse {
    cursor: i64,
    changes: Vec<ChangeWire>,
}

#[derive(Clone)]
struct ServerState {
    db_path: Arc<PathBuf>,
}

pub async fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Server(args) => run_server(args).await,
        command => {
            let db_path = db_path(cli.db)?;
            let conn = open_db(&db_path)?;
            match command {
                Commands::Add(args) => cmd_add(&conn, args),
                Commands::Show(args) => cmd_show(&conn, args),
                Commands::List(args) => cmd_list(&conn, args),
                Commands::Update(args) => cmd_update(&conn, args),
                Commands::Note(args) => cmd_note(&conn, args),
                Commands::Projects(args) => cmd_projects(&conn, args),
                Commands::Labels(args) => cmd_labels(&conn, args),
                Commands::Label(args) => cmd_label(&conn, args),
                Commands::Project(args) => cmd_project(&conn, args),
                Commands::Delete(args) => cmd_delete_restore(&conn, args, true),
                Commands::Restore(args) => cmd_delete_restore(&conn, args, false),
                Commands::Conflict(args) => cmd_conflict(&conn, args),
                Commands::Sync(args) => sync_client(&conn, args).await,
                Commands::Server(_) => unreachable!(),
            }
        }
    }
}

fn db_path(flag: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = flag {
        return Ok(path);
    }
    if let Ok(path) = env::var("ATM_DB") {
        return Ok(PathBuf::from(path));
    }
    let mut dir = dirs::data_dir().context("could not find app data directory")?;
    dir.push("atm");
    dir.push("db.sqlite");
    Ok(dir)
}

fn open_db(path: &Path) -> Result<Connection> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
    }
    let conn =
        Connection::open(path).with_context(|| format!("could not open {}", path.display()))?;
    ensure_schema(&conn)?;
    Ok(conn)
}

fn ensure_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        PRAGMA foreign_keys = ON;
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS projects (
            key TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            prefix TEXT NOT NULL UNIQUE,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS project_paths (
            project_key TEXT NOT NULL,
            path TEXT NOT NULL UNIQUE,
            PRIMARY KEY (project_key, path)
        );
        CREATE TABLE IF NOT EXISTS labels (
            name TEXT PRIMARY KEY,
            created_at TEXT NOT NULL
        );
        CREATE TABLE IF NOT EXISTS tasks (
            id TEXT PRIMARY KEY,
            title TEXT NOT NULL,
            description TEXT NOT NULL,
            project_key TEXT NOT NULL,
            status TEXT NOT NULL,
            priority TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL,
            deleted INTEGER NOT NULL DEFAULT 0
        );
        CREATE TABLE IF NOT EXISTS task_labels (
            task_id TEXT NOT NULL,
            label TEXT NOT NULL,
            PRIMARY KEY (task_id, label)
        );
        CREATE TABLE IF NOT EXISTS notes (
            id TEXT PRIMARY KEY,
            task_id TEXT NOT NULL,
            body TEXT NOT NULL,
            created_at TEXT NOT NULL,
            change_id TEXT NOT NULL UNIQUE
        );
        CREATE TABLE IF NOT EXISTS changes (
            change_id TEXT PRIMARY KEY,
            client_id TEXT NOT NULL,
            local_seq INTEGER NOT NULL,
            entity_type TEXT NOT NULL,
            entity_id TEXT NOT NULL,
            field TEXT,
            op_type TEXT NOT NULL,
            payload TEXT NOT NULL,
            base_version TEXT,
            created_at TEXT NOT NULL,
            server_seq INTEGER
        );
        CREATE TABLE IF NOT EXISTS field_versions (
            entity_id TEXT NOT NULL,
            field TEXT NOT NULL,
            version TEXT NOT NULL,
            PRIMARY KEY (entity_id, field)
        );
        CREATE TABLE IF NOT EXISTS conflicts (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id TEXT NOT NULL,
            field TEXT NOT NULL,
            base_version TEXT,
            local_value TEXT NOT NULL,
            remote_value TEXT NOT NULL,
            local_change_id TEXT,
            remote_change_id TEXT NOT NULL,
            variant_a TEXT NOT NULL,
            variant_b TEXT NOT NULL,
            created_at TEXT NOT NULL,
            resolved INTEGER NOT NULL DEFAULT 0,
            UNIQUE (task_id, field, remote_change_id)
        );
        CREATE INDEX IF NOT EXISTS idx_changes_server_seq ON changes(server_seq);
        CREATE INDEX IF NOT EXISTS idx_tasks_project ON tasks(project_key);
        ",
    )?;
    if get_meta(conn, "client_id")?.is_none() {
        set_meta(conn, "client_id", &new_id())?;
    }
    if get_meta(conn, "sync_cursor")?.is_none() {
        set_meta(conn, "sync_cursor", "0")?;
    }
    if get_meta(conn, "local_seq")?.is_none() {
        set_meta(conn, "local_seq", "0")?;
    }
    Ok(())
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

fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT value FROM meta WHERE key = ?", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn next_local_seq(conn: &Connection) -> Result<i64> {
    let seq = get_meta(conn, "local_seq")?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?
        + 1;
    set_meta(conn, "local_seq", &seq.to_string())?;
    Ok(seq)
}

fn insert_change(
    conn: &Connection,
    entity_type: &str,
    entity_id: &str,
    field: Option<&str>,
    op_type: &str,
    payload: Value,
    base_version: Option<&str>,
) -> Result<String> {
    let change_id = new_id();
    let client_id = get_meta(conn, "client_id")?.context("missing client id")?;
    let local_seq = next_local_seq(conn)?;
    let created_at = now();
    conn.execute(
        "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            change_id,
            client_id,
            local_seq,
            entity_type,
            entity_id,
            field,
            op_type,
            payload.to_string(),
            base_version,
            created_at
        ],
    )?;
    Ok(change_id)
}

fn cmd_add(conn: &Connection, args: AddArgs) -> Result<()> {
    validate_choice("priority", &args.priority, PRIORITIES)?;
    let description = read_optional_text(
        args.description,
        args.description_file.as_deref(),
        args.description_stdin,
        "description",
    )?
    .unwrap_or_default();
    let project = resolve_project_for_add(conn, args.project.as_deref())?;
    let labels = resolve_labels(conn, &args.label)?;
    let id = new_id();
    let ts = now();
    conn.execute(
        "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES (?, ?, ?, ?, 'inbox', ?, ?, ?)",
        params![id, args.title, description, project.key, args.priority, ts, ts],
    )?;
    for label in &labels {
        conn.execute(
            "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
            params![id, label],
        )?;
    }
    let change_id = insert_change(
        conn,
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
    )?;
    for field in [
        "title",
        "description",
        "project",
        "status",
        "priority",
        "deleted",
    ] {
        set_field_version(conn, &id, field, &change_id)?;
    }
    let task = get_task(conn, &id)?;
    println!(
        "created {} ref={} project={} status={} priority={} title={}",
        display_ref(conn, &task)?,
        display_suffix(conn, &task.id)?,
        task.project_key,
        task.status,
        task.priority,
        quote(&task.title)
    );
    Ok(())
}

fn cmd_show(conn: &Connection, args: ShowArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref)?;
    print_task(conn, &task, args.full)
}

fn cmd_list(conn: &Connection, args: ListArgs) -> Result<()> {
    let mut query = String::from(
        "SELECT t.id, t.title, t.description, t.project_key, p.prefix, t.status, t.priority,
         t.created_at, t.updated_at, t.deleted
         FROM tasks t JOIN projects p ON p.key = t.project_key",
    );
    let mut filters = Vec::new();
    let mut values = Vec::new();
    if !args.all {
        filters.push("t.deleted = 0".to_string());
    }
    let project_key = if let Some(project) = args.project.as_deref() {
        Some(resolve_existing_project(conn, project)?.key)
    } else {
        None
    };
    if let Some(project_key) = project_key {
        filters.push("t.project_key = ?".to_string());
        values.push(project_key);
    }
    if let Some(status) = args.status.as_deref() {
        validate_choice("status", status, STATUSES)?;
        filters.push("t.status = ?".to_string());
        values.push(status.to_string());
    }
    if let Some(priority) = args.priority.as_deref() {
        validate_choice("priority", priority, PRIORITIES)?;
        filters.push("t.priority = ?".to_string());
        values.push(priority.to_string());
    }
    if let Some(label) = args.label.as_deref() {
        let label = ensure_label_exists(conn, label)?;
        filters.push(
            "EXISTS (SELECT 1 FROM task_labels tl WHERE tl.task_id = t.id AND tl.label = ?)"
                .to_string(),
        );
        values.push(label);
    }
    if !filters.is_empty() {
        query.push_str(" WHERE ");
        query.push_str(&filters.join(" AND "));
    }
    query.push_str(" ORDER BY t.updated_at DESC, t.created_at DESC");

    let mut stmt = conn.prepare(&query)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(values), task_from_row)?;
    for row in rows {
        print_task_line(conn, &row?)?;
    }
    Ok(())
}

fn cmd_update(conn: &Connection, args: UpdateArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref)?;
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
    if let Some(title) = args.title {
        set_task_field(conn, &task.id, "title", &title)?;
        changed.push("title");
    }
    if let Some(description) = description {
        set_task_field(conn, &task.id, "description", &description)?;
        changed.push("description");
    }
    if let Some(project) = args.project {
        let project = resolve_project_for_add(conn, Some(&project))?;
        set_task_field(conn, &task.id, "project", &project.key)?;
        changed.push("project");
    }
    if let Some(status) = args.status {
        set_task_field(conn, &task.id, "status", &status)?;
        changed.push("status");
    }
    if let Some(priority) = args.priority {
        set_task_field(conn, &task.id, "priority", &priority)?;
        changed.push("priority");
    }
    for label in resolve_labels(conn, &args.label)? {
        conn.execute(
            "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
            params![task.id, label],
        )?;
        insert_change(
            conn,
            "task",
            &task.id,
            Some("labels"),
            "label_add",
            json!({ "label": label }),
            None,
        )?;
        changed.push("label");
    }
    for label in resolve_labels(conn, &args.remove_label)? {
        conn.execute(
            "DELETE FROM task_labels WHERE task_id = ? AND label = ?",
            params![task.id, label],
        )?;
        insert_change(
            conn,
            "task",
            &task.id,
            Some("labels"),
            "label_remove",
            json!({ "label": label }),
            None,
        )?;
        changed.push("label");
    }
    let task = get_task(conn, &task.id)?;
    println!(
        "updated {} changed={} status={} priority={} title={}",
        display_ref(conn, &task)?,
        if changed.is_empty() { "none" } else { "yes" },
        task.status,
        task.priority,
        quote(&task.title)
    );
    Ok(())
}

fn cmd_note(conn: &Connection, args: NoteArgs) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref)?;
    let body = read_required_text(args.text, args.file.as_deref(), args.stdin, "note")?;
    let note_id = new_id();
    let ts = now();
    let change_id = insert_change(
        conn,
        "task",
        &task.id,
        Some("notes"),
        "note_add",
        json!({ "note_id": note_id, "body": body, "created_at": ts }),
        None,
    )?;
    conn.execute(
        "INSERT INTO notes(id, task_id, body, created_at, change_id) VALUES (?, ?, ?, ?, ?)",
        params![note_id, task.id, body, ts, change_id],
    )?;
    println!("noted {} note={}", display_ref(conn, &task)?, note_id);
    Ok(())
}

fn cmd_projects(conn: &Connection, args: SearchArgs) -> Result<()> {
    let projects = list_projects(conn, args.search.as_deref())?;
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

fn cmd_labels(conn: &Connection, args: SearchArgs) -> Result<()> {
    let labels = list_labels(conn, args.search.as_deref())?;
    for label in labels {
        println!("{label}");
    }
    Ok(())
}

fn cmd_label(conn: &Connection, args: LabelCommand) -> Result<()> {
    match args.command {
        LabelSubcommand::Create { name } => {
            let name = normalize_label(&name);
            if name.is_empty() {
                bail!("error invalid-label");
            }
            let created_at = now();
            conn.execute(
                "INSERT OR IGNORE INTO labels(name, created_at) VALUES (?, ?)",
                params![name, created_at],
            )?;
            insert_change(
                conn,
                "label",
                &name,
                None,
                "create_label",
                json!({ "name": name, "created_at": created_at }),
                None,
            )?;
            println!("created-label {name}");
        }
    }
    Ok(())
}

fn cmd_project(conn: &Connection, args: ProjectCommand) -> Result<()> {
    match args.command {
        ProjectSubcommand::Create { name, path } => {
            let project = create_project(conn, &name)?;
            if let Some(path) = path {
                add_project_path(conn, &project.key, &path)?;
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
                let project = resolve_existing_project(conn, &project)?;
                add_project_path(conn, &project.key, &path)?;
                println!(
                    "added-project-path {} path={}",
                    project.key,
                    quote(&path.display().to_string())
                );
            }
            ProjectPathSubcommand::Remove { project, path } => {
                let project = resolve_existing_project(conn, &project)?;
                conn.execute(
                    "DELETE FROM project_paths WHERE project_key = ? AND path = ?",
                    params![project.key, path.display().to_string()],
                )?;
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

fn cmd_delete_restore(conn: &Connection, args: RefArgs, delete: bool) -> Result<()> {
    let task = resolve_task_ref(conn, &args.task_ref)?;
    set_task_field(conn, &task.id, "deleted", if delete { "1" } else { "0" })?;
    let task = get_task(conn, &task.id)?;
    if delete {
        println!("deleted {}", display_ref(conn, &task)?);
    } else {
        println!("restored {}", display_ref(conn, &task)?);
    }
    Ok(())
}

fn cmd_conflict(conn: &Connection, args: ConflictCommand) -> Result<()> {
    match args.command {
        ConflictSubcommand::List { project, field } => {
            let project_key = if let Some(project) = project {
                Some(resolve_existing_project(conn, &project)?.key)
            } else {
                None
            };
            let mut stmt = conn.prepare(
                "SELECT c.task_id, c.field, c.variant_a, c.variant_b, t.title, p.prefix, t.project_key
                 FROM conflicts c
                 JOIN tasks t ON t.id = c.task_id
                 JOIN projects p ON p.key = t.project_key
                 WHERE c.resolved = 0
                 AND (?1 IS NULL OR t.project_key = ?1)
                 AND (?2 IS NULL OR c.field = ?2)
                 ORDER BY c.created_at",
            )?;
            let rows = stmt.query_map(params![project_key, field], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?;
            for row in rows {
                let (id, field, a, b, title, prefix, project_key) = row?;
                let task = Task {
                    id,
                    title,
                    description: String::new(),
                    project_key,
                    project_prefix: prefix,
                    status: String::new(),
                    priority: String::new(),
                    created_at: String::new(),
                    updated_at: String::new(),
                    deleted: false,
                };
                println!(
                    "{} conflict field={} variants={},{} title={}",
                    display_ref(conn, &task)?,
                    field,
                    a,
                    b,
                    quote(&task.title)
                );
            }
        }
        ConflictSubcommand::Show { task_ref, field } => {
            let task = resolve_task_ref(conn, &task_ref)?;
            print_conflicts(conn, &task, field.as_deref())?;
        }
        ConflictSubcommand::Resolve {
            task_ref,
            field,
            use_variant,
            value,
            value_file,
            value_stdin,
        } => {
            let task = resolve_task_ref(conn, &task_ref)?;
            let value = if let Some(token) = use_variant {
                conflict_variant_value(conn, &task.id, &field, &token)?
            } else {
                read_required_text(value, value_file.as_deref(), value_stdin, "value")?
            };
            apply_field_value(conn, &task.id, &field, &value)?;
            conn.execute(
                "UPDATE conflicts SET resolved = 1 WHERE task_id = ? AND field = ? AND resolved = 0",
                params![task.id, field],
            )?;
            let change_id = insert_change(
                conn,
                "task",
                &task.id,
                Some(&field),
                "resolve_field",
                json!({ "value": value }),
                None,
            )?;
            set_field_version(conn, &task.id, &field, &change_id)?;
            let task = get_task(conn, &task.id)?;
            println!("resolved {} field={}", display_ref(conn, &task)?, field);
        }
    }
    Ok(())
}

fn set_task_field(conn: &Connection, task_id: &str, field: &str, value: &str) -> Result<()> {
    if conflict_exists(conn, task_id, field)? {
        bail!(
            "error conflicted-field ref={} field={} hint=\"use conflict resolve\"",
            task_id,
            field
        );
    }
    let base = field_version(conn, task_id, field)?;
    apply_field_value(conn, task_id, field, value)?;
    let change_id = insert_change(
        conn,
        "task",
        task_id,
        Some(field),
        "set_field",
        json!({ "value": value }),
        base.as_deref(),
    )?;
    set_field_version(conn, task_id, field, &change_id)?;
    Ok(())
}

fn apply_field_value(conn: &Connection, task_id: &str, field: &str, value: &str) -> Result<()> {
    match field {
        "title" => conn.execute(
            "UPDATE tasks SET title = ?, updated_at = ? WHERE id = ?",
            params![value, now(), task_id],
        )?,
        "description" => conn.execute(
            "UPDATE tasks SET description = ?, updated_at = ? WHERE id = ?",
            params![value, now(), task_id],
        )?,
        "project" => {
            let project = resolve_project_for_add(conn, Some(value))?;
            conn.execute(
                "UPDATE tasks SET project_key = ?, updated_at = ? WHERE id = ?",
                params![project.key, now(), task_id],
            )?
        }
        "status" => {
            validate_choice("status", value, STATUSES)?;
            conn.execute(
                "UPDATE tasks SET status = ?, updated_at = ? WHERE id = ?",
                params![value, now(), task_id],
            )?
        }
        "priority" => {
            validate_choice("priority", value, PRIORITIES)?;
            conn.execute(
                "UPDATE tasks SET priority = ?, updated_at = ? WHERE id = ?",
                params![value, now(), task_id],
            )?
        }
        "deleted" => conn.execute(
            "UPDATE tasks SET deleted = ?, updated_at = ? WHERE id = ?",
            params![value.parse::<i64>().unwrap_or(0), now(), task_id],
        )?,
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

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Project> {
    Ok(Project {
        key: row.get(0)?,
        name: row.get(1)?,
        prefix: row.get(2)?,
    })
}

fn resolve_project_for_add(conn: &Connection, project: Option<&str>) -> Result<Project> {
    if let Some(project) = project {
        if let Some(existing) = find_project(conn, project)? {
            return Ok(existing);
        }
        let choices = near_projects(conn, project)?;
        if !choices.is_empty() {
            print_near_error("project", project, &choices);
            bail!("near-match project");
        }
        return create_project(conn, project);
    }
    if let Some(project) = project_from_path_mapping(conn)? {
        return Ok(project);
    }
    if let Some(root_name) = git_root_name()? {
        if let Some(existing) = find_project(conn, &root_name)? {
            return Ok(existing);
        }
        let choices = near_projects(conn, &root_name)?;
        if !choices.is_empty() {
            print_near_error("project", &root_name, &choices);
            bail!("near-match project");
        }
        return create_project(conn, &root_name);
    }
    bail!("error project-required");
}

fn resolve_existing_project(conn: &Connection, project: &str) -> Result<Project> {
    if let Some(project) = find_project(conn, project)? {
        return Ok(project);
    }
    let choices = near_projects(conn, project)?;
    if !choices.is_empty() {
        print_near_error("project", project, &choices);
    } else {
        eprintln!("error unknown-project input={}", project);
    }
    bail!("unknown project");
}

fn find_project(conn: &Connection, input: &str) -> Result<Option<Project>> {
    let key = normalize_key(input);
    Ok(conn
        .query_row(
            "SELECT key, name, prefix FROM projects
             WHERE deleted = 0 AND (key = ? OR lower(name) = lower(?))",
            params![key, input],
            project_from_row,
        )
        .optional()?)
}

fn create_project(conn: &Connection, name: &str) -> Result<Project> {
    let key = normalize_key(name);
    if key.is_empty() {
        bail!("error invalid-project input={}", quote(name));
    }
    if let Some(project) = find_project(conn, &key)? {
        return Ok(project);
    }
    let prefix = unique_project_prefix(conn, &key)?;
    let ts = now();
    conn.execute(
        "INSERT INTO projects(key, name, prefix, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
        params![key, name, prefix, ts, ts],
    )?;
    insert_change(
        conn,
        "project",
        &key,
        None,
        "create_project",
        json!({ "key": key, "name": name, "prefix": prefix, "created_at": ts }),
        None,
    )?;
    Ok(Project {
        key,
        name: name.to_string(),
        prefix,
    })
}

fn unique_project_prefix(conn: &Connection, key: &str) -> Result<String> {
    let base = prefix_base(key);
    let mut candidate = base.clone();
    let mut n = 2;
    while conn
        .query_row(
            "SELECT 1 FROM projects WHERE prefix = ?",
            [&candidate],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
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

fn project_from_path_mapping(conn: &Connection) -> Result<Option<Project>> {
    let cwd = fs::canonicalize(env::current_dir()?)?;
    let mut stmt = conn.prepare(
        "SELECT p.key, p.name, p.prefix, pp.path
         FROM project_paths pp JOIN projects p ON p.key = pp.project_key
         ORDER BY length(pp.path) DESC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            Project {
                key: row.get(0)?,
                name: row.get(1)?,
                prefix: row.get(2)?,
            },
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (project, path) = row?;
        if cwd.starts_with(Path::new(&path)) {
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

fn add_project_path(conn: &Connection, project_key: &str, path: &Path) -> Result<()> {
    let path =
        fs::canonicalize(path).with_context(|| format!("could not resolve {}", path.display()))?;
    conn.execute(
        "INSERT OR IGNORE INTO project_paths(project_key, path) VALUES (?, ?)",
        params![project_key, path.display().to_string()],
    )?;
    Ok(())
}

fn near_projects(conn: &Connection, input: &str) -> Result<Vec<String>> {
    let needle = normalize_key(input);
    let projects = list_projects(conn, None)?;
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

fn near_labels(conn: &Connection, input: &str) -> Result<Vec<String>> {
    let needle = normalize_label(input);
    Ok(list_labels(conn, None)?
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

fn list_projects(conn: &Connection, search: Option<&str>) -> Result<Vec<Project>> {
    let search = search.map(normalize_key);
    let mut stmt = conn.prepare(
        "SELECT key, name, prefix FROM projects
         WHERE deleted = 0
         ORDER BY key",
    )?;
    let projects = stmt
        .query_map([], project_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(projects
        .into_iter()
        .filter(|project| {
            search.as_deref().is_none_or(|search| {
                project.key.contains(search) || project.name.to_lowercase().contains(search)
            })
        })
        .collect())
}

fn list_labels(conn: &Connection, search: Option<&str>) -> Result<Vec<String>> {
    let search = search.map(normalize_label);
    let mut stmt = conn.prepare("SELECT name FROM labels ORDER BY name")?;
    let labels = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(labels
        .into_iter()
        .filter(|label| {
            search
                .as_deref()
                .is_none_or(|search| label.contains(search))
        })
        .collect())
}

fn ensure_label_exists(conn: &Connection, label: &str) -> Result<String> {
    let label = normalize_label(label);
    if conn
        .query_row("SELECT 1 FROM labels WHERE name = ?", [&label], |_| Ok(()))
        .optional()?
        .is_some()
    {
        Ok(label)
    } else {
        let choices = near_labels(conn, &label)?;
        eprintln!("error unknown-label input={}", label);
        for choice in choices {
            eprintln!("choice {choice}");
        }
        eprintln!("hint \"create the label explicitly\"");
        bail!("unknown label");
    }
}

fn resolve_labels(conn: &Connection, labels: &[String]) -> Result<Vec<String>> {
    labels
        .iter()
        .map(|label| ensure_label_exists(conn, label))
        .collect()
}

fn task_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: row.get(0)?,
        title: row.get(1)?,
        description: row.get(2)?,
        project_key: row.get(3)?,
        project_prefix: row.get(4)?,
        status: row.get(5)?,
        priority: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        deleted: row.get::<_, i64>(9)? != 0,
    })
}

fn get_task(conn: &Connection, id: &str) -> Result<Task> {
    Ok(conn.query_row(
        "SELECT t.id, t.title, t.description, t.project_key, p.prefix, t.status, t.priority,
         t.created_at, t.updated_at, t.deleted
         FROM tasks t JOIN projects p ON p.key = t.project_key
         WHERE t.id = ?",
        [id],
        task_from_row,
    )?)
}

fn resolve_task_ref(conn: &Connection, input: &str) -> Result<Task> {
    let (hint, suffix) = split_ref(input);
    if suffix.len() < 3 {
        bail!("error ref-too-short input={} minimum=3", input);
    }
    let suffix = suffix.to_ascii_uppercase();
    let mut stmt = conn.prepare(
        "SELECT t.id, t.title, t.description, t.project_key, p.prefix, t.status, t.priority,
         t.created_at, t.updated_at, t.deleted
         FROM tasks t JOIN projects p ON p.key = t.project_key
         WHERE t.id LIKE ? || '%'
         ORDER BY t.id",
    )?;
    let matches = stmt
        .query_map([suffix], task_from_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
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
            display_ref(conn, &task)?,
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

fn display_ref(conn: &Connection, task: &Task) -> Result<String> {
    Ok(format!(
        "{}-{}",
        task.project_prefix,
        display_suffix(conn, &task.id)?
    ))
}

fn display_suffix(conn: &Connection, id: &str) -> Result<String> {
    for len in 7..=16 {
        let prefix = &id[..len];
        let count: i64 = conn.query_row(
            "SELECT count(*) FROM tasks WHERE id LIKE ? || '%'",
            [prefix],
            |row| row.get(0),
        )?;
        if count <= 1 {
            return Ok(prefix.to_string());
        }
    }
    Ok(id.to_string())
}

fn labels_for_task(conn: &Connection, task_id: &str) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT label FROM task_labels WHERE task_id = ? ORDER BY label")?;
    Ok(stmt
        .query_map([task_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn print_task_line(conn: &Connection, task: &Task) -> Result<()> {
    let labels = labels_for_task(conn, &task.id)?.join(",");
    let conflict = if task_has_conflict(conn, &task.id)? {
        " conflicts=yes"
    } else {
        ""
    };
    let deleted = if task.deleted { " deleted=yes" } else { "" };
    println!(
        "{} status={} priority={} labels={}{}{} title={}",
        display_ref(conn, task)?,
        task.status,
        task.priority,
        labels,
        conflict,
        deleted,
        quote(&task.title)
    );
    Ok(())
}

fn print_task(conn: &Connection, task: &Task, full: bool) -> Result<()> {
    print_task_line(conn, task)?;
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
        let mut stmt = conn.prepare(
            "SELECT body, created_at FROM notes WHERE task_id = ? ORDER BY created_at, id",
        )?;
        let notes = stmt.query_map([&task.id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for note in notes {
            let (body, created_at) = note?;
            println!("note created={created_at} body={}", quote(&body));
        }
        print_conflicts(conn, task, None)?;
    }
    Ok(())
}

fn quote(input: &str) -> String {
    serde_json::to_string(input).unwrap_or_else(|_| "\"\"".to_string())
}

fn field_version(conn: &Connection, entity_id: &str, field: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT version FROM field_versions WHERE entity_id = ? AND field = ?",
            params![entity_id, field],
            |row| row.get(0),
        )
        .optional()?)
}

fn set_field_version(conn: &Connection, entity_id: &str, field: &str, version: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO field_versions(entity_id, field, version) VALUES (?, ?, ?)
         ON CONFLICT(entity_id, field) DO UPDATE SET version = excluded.version",
        params![entity_id, field, version],
    )?;
    Ok(())
}

fn task_has_conflict(conn: &Connection, task_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM conflicts WHERE task_id = ? AND resolved = 0 LIMIT 1",
            [task_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn conflict_exists(conn: &Connection, task_id: &str, field: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM conflicts WHERE task_id = ? AND field = ? AND resolved = 0 LIMIT 1",
            params![task_id, field],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn print_conflicts(conn: &Connection, task: &Task, field: Option<&str>) -> Result<()> {
    let mut stmt = conn.prepare(
        "SELECT field, variant_a, local_value, variant_b, remote_value
         FROM conflicts
         WHERE task_id = ? AND resolved = 0 AND (? IS NULL OR field = ?)
         ORDER BY field, id",
    )?;
    let rows = stmt.query_map(params![task.id, field, field], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
        ))
    })?;
    for row in rows {
        let (field, a, local, b, remote) = row?;
        println!("conflict {} field={}", display_ref(conn, task)?, field);
        println!("variant {} value={}", a, quote(&local));
        println!("variant {} value={}", b, quote(&remote));
    }
    Ok(())
}

fn conflict_variant_value(
    conn: &Connection,
    task_id: &str,
    field: &str,
    token: &str,
) -> Result<String> {
    let mut stmt = conn.prepare(
        "SELECT variant_a, local_value, variant_b, remote_value
         FROM conflicts WHERE task_id = ? AND field = ? AND resolved = 0",
    )?;
    let rows = stmt.query_map(params![task_id, field], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (a, local, b, remote) = row?;
        if token == a {
            return Ok(local);
        }
        if token == b {
            return Ok(remote);
        }
    }
    bail!("error unknown-variant token={}", token);
}

async fn run_server(args: ServerArgs) -> Result<()> {
    if !args.unsafe_public_bind && !args.bind.ip().is_loopback() {
        bail!("error public-bind-requires --unsafe-public-bind");
    }
    let _conn = open_db(&args.data)?;
    let state = ServerState {
        db_path: Arc::new(args.data),
    };
    let app = Router::new()
        .route("/sync", post(sync_handler))
        .with_state(state);
    let listener = TcpListener::bind(args.bind).await?;
    let addr = listener.local_addr()?;
    println!("listening url=http://{}", addr);
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
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

async fn sync_handler(
    State(state): State<ServerState>,
    Json(request): Json<SyncRequest>,
) -> std::result::Result<Json<SyncResponse>, String> {
    let conn = open_db(&state.db_path).map_err(|err| err.to_string())?;
    let tx = conn
        .unchecked_transaction()
        .map_err(|err| err.to_string())?;
    for change in request.changes {
        let exists = tx
            .query_row(
                "SELECT 1 FROM changes WHERE change_id = ?",
                [&change.change_id],
                |_| Ok(()),
            )
            .optional()
            .map_err(|err| err.to_string())?
            .is_some();
        if !exists {
            tx.execute(
                "INSERT INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
                 op_type, payload, base_version, created_at, server_seq)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, (SELECT COALESCE(MAX(server_seq), 0) + 1 FROM changes))",
                params![
                    change.change_id,
                    change.client_id,
                    change.local_seq,
                    change.entity_type,
                    change.entity_id,
                    change.field,
                    change.op_type,
                    change.payload.to_string(),
                    change.base_version,
                    change.created_at
                ],
            )
            .map_err(|err| err.to_string())?;
        }
    }
    tx.commit().map_err(|err| err.to_string())?;
    let changes = load_server_changes(&conn, request.after).map_err(|err| err.to_string())?;
    let cursor = changes
        .iter()
        .filter_map(|change| change.server_seq)
        .max()
        .unwrap_or(request.after);
    Ok(Json(SyncResponse { cursor, changes }))
}

async fn sync_client(conn: &Connection, args: SyncArgs) -> Result<()> {
    let client_id = get_meta(conn, "client_id")?.context("missing client id")?;
    let after = get_meta(conn, "sync_cursor")?
        .unwrap_or_else(|| "0".to_string())
        .parse::<i64>()?;
    let changes = load_unsynced_changes(conn)?;
    let url = format!("{}/sync", args.server.trim_end_matches('/'));
    let response = reqwest::Client::new()
        .post(url)
        .json(&SyncRequest {
            client_id,
            after,
            changes,
        })
        .send()
        .await?
        .error_for_status()?
        .json::<SyncResponse>()
        .await?;
    let mut applied = 0;
    for change in &response.changes {
        if change_exists(conn, &change.change_id)? {
            update_change_server_seq(conn, &change.change_id, change.server_seq)?;
            continue;
        }
        apply_remote_change(conn, change)?;
        insert_wire_change(conn, change)?;
        applied += 1;
    }
    set_meta(conn, "sync_cursor", &response.cursor.to_string())?;
    let pushed = conn.query_row(
        "SELECT count(*) FROM changes WHERE server_seq IS NOT NULL",
        [],
        |row| row.get::<_, i64>(0),
    )?;
    println!(
        "synced pushed={} pulled={} cursor={}",
        pushed, applied, response.cursor
    );
    Ok(())
}

fn load_unsynced_changes(conn: &Connection) -> Result<Vec<ChangeWire>> {
    load_changes_where(conn, "server_seq IS NULL", &[])
}

fn load_server_changes(conn: &Connection, after: i64) -> Result<Vec<ChangeWire>> {
    load_changes_where(conn, "server_seq > ?", &[&after])
}

fn load_changes_where(
    conn: &Connection,
    condition: &str,
    params_in: &[&dyn rusqlite::ToSql],
) -> Result<Vec<ChangeWire>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT change_id, client_id, local_seq, entity_type, entity_id, field, op_type,
         payload, base_version, created_at, server_seq
         FROM changes WHERE {condition} ORDER BY COALESCE(server_seq, local_seq), created_at"
    ))?;
    let changes = stmt
        .query_map(params_in, |row| {
            let payload: String = row.get(7)?;
            Ok(ChangeWire {
                change_id: row.get(0)?,
                client_id: row.get(1)?,
                local_seq: row.get(2)?,
                entity_type: row.get(3)?,
                entity_id: row.get(4)?,
                field: row.get(5)?,
                op_type: row.get(6)?,
                payload: serde_json::from_str(&payload).unwrap_or(Value::Null),
                base_version: row.get(8)?,
                created_at: row.get(9)?,
                server_seq: row.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(changes)
}

fn change_exists(conn: &Connection, change_id: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM changes WHERE change_id = ?",
            [change_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn update_change_server_seq(
    conn: &Connection,
    change_id: &str,
    server_seq: Option<i64>,
) -> Result<()> {
    if let Some(server_seq) = server_seq {
        conn.execute(
            "UPDATE changes SET server_seq = ? WHERE change_id = ?",
            params![server_seq, change_id],
        )?;
    }
    Ok(())
}

fn insert_wire_change(conn: &Connection, change: &ChangeWire) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO changes(change_id, client_id, local_seq, entity_type, entity_id, field,
         op_type, payload, base_version, created_at, server_seq)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            change.change_id,
            change.client_id,
            change.local_seq,
            change.entity_type,
            change.entity_id,
            change.field,
            change.op_type,
            change.payload.to_string(),
            change.base_version,
            change.created_at,
            change.server_seq
        ],
    )?;
    Ok(())
}

fn apply_remote_change(conn: &Connection, change: &ChangeWire) -> Result<()> {
    match change.op_type.as_str() {
        "create_project" => {
            let key = str_payload(&change.payload, "key")?;
            let name = str_payload(&change.payload, "name")?;
            let prefix = str_payload(&change.payload, "prefix")?;
            let created_at = str_payload(&change.payload, "created_at").unwrap_or_else(|_| now());
            conn.execute(
                "INSERT OR IGNORE INTO projects(key, name, prefix, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?)",
                params![key, name, prefix, created_at, created_at],
            )?;
        }
        "create_label" => {
            let name = str_payload(&change.payload, "name")?;
            let created_at = str_payload(&change.payload, "created_at").unwrap_or_else(|_| now());
            conn.execute(
                "INSERT OR IGNORE INTO labels(name, created_at) VALUES (?, ?)",
                params![name, created_at],
            )?;
        }
        "create_task" => apply_remote_create_task(conn, change)?,
        "set_field" => apply_remote_set_field(conn, change, false)?,
        "resolve_field" => apply_remote_set_field(conn, change, true)?,
        "label_add" => {
            let label = str_payload(&change.payload, "label")?;
            conn.execute(
                "INSERT OR IGNORE INTO labels(name, created_at) VALUES (?, ?)",
                params![label, change.created_at],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
                params![change.entity_id, label],
            )?;
        }
        "label_remove" => {
            let label = str_payload(&change.payload, "label")?;
            conn.execute(
                "DELETE FROM task_labels WHERE task_id = ? AND label = ?",
                params![change.entity_id, label],
            )?;
        }
        "note_add" => {
            let note_id = str_payload(&change.payload, "note_id")?;
            let body = str_payload(&change.payload, "body")?;
            let created_at = str_payload(&change.payload, "created_at")
                .unwrap_or_else(|_| change.created_at.clone());
            conn.execute(
                "INSERT OR IGNORE INTO notes(id, task_id, body, created_at, change_id)
                 VALUES (?, ?, ?, ?, ?)",
                params![
                    note_id,
                    change.entity_id,
                    body,
                    created_at,
                    change.change_id
                ],
            )?;
        }
        _ => {}
    }
    Ok(())
}

fn apply_remote_create_task(conn: &Connection, change: &ChangeWire) -> Result<()> {
    if conn
        .query_row(
            "SELECT 1 FROM tasks WHERE id = ?",
            [&change.entity_id],
            |_| Ok(()),
        )
        .optional()?
        .is_some()
    {
        return Ok(());
    }
    let project_key = str_payload(&change.payload, "project_key")?;
    if find_project(conn, &project_key)?.is_none() {
        let name =
            str_payload(&change.payload, "project_name").unwrap_or_else(|_| project_key.clone());
        let prefix = str_payload(&change.payload, "project_prefix")
            .unwrap_or_else(|_| prefix_base(&project_key));
        conn.execute(
            "INSERT OR IGNORE INTO projects(key, name, prefix, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?)",
            params![
                project_key,
                name,
                prefix,
                change.created_at,
                change.created_at
            ],
        )?;
    }
    let title = str_payload(&change.payload, "title")?;
    let description = str_payload(&change.payload, "description").unwrap_or_default();
    let status = str_payload(&change.payload, "status").unwrap_or_else(|_| "inbox".to_string());
    let priority = str_payload(&change.payload, "priority").unwrap_or_else(|_| "none".to_string());
    let created_at =
        str_payload(&change.payload, "created_at").unwrap_or_else(|_| change.created_at.clone());
    conn.execute(
        "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
        params![change.entity_id, title, description, project_key, status, priority, created_at, change.created_at],
    )?;
    if let Some(labels) = change.payload.get("labels").and_then(Value::as_array) {
        for label in labels.iter().filter_map(Value::as_str) {
            conn.execute(
                "INSERT OR IGNORE INTO labels(name, created_at) VALUES (?, ?)",
                params![label, change.created_at],
            )?;
            conn.execute(
                "INSERT OR IGNORE INTO task_labels(task_id, label) VALUES (?, ?)",
                params![change.entity_id, label],
            )?;
        }
    }
    for field in [
        "title",
        "description",
        "project",
        "status",
        "priority",
        "deleted",
    ] {
        set_field_version(conn, &change.entity_id, field, &change.change_id)?;
    }
    Ok(())
}

fn apply_remote_set_field(conn: &Connection, change: &ChangeWire, force: bool) -> Result<()> {
    let field = change
        .field
        .as_deref()
        .context("field change missing field")?;
    let value = str_payload(&change.payload, "value")?;
    if !force {
        let current = field_version(conn, &change.entity_id, field)?;
        if current != change.base_version {
            create_conflict(conn, change, field, &value, current.as_deref())?;
            return Ok(());
        }
    }
    apply_field_value(conn, &change.entity_id, field, &value)?;
    set_field_version(conn, &change.entity_id, field, &change.change_id)?;
    if force {
        conn.execute(
            "UPDATE conflicts SET resolved = 1 WHERE task_id = ? AND field = ? AND resolved = 0",
            params![change.entity_id, field],
        )?;
    }
    Ok(())
}

fn create_conflict(
    conn: &Connection,
    change: &ChangeWire,
    field: &str,
    remote_value: &str,
    local_change_id: Option<&str>,
) -> Result<()> {
    if conflict_exists(conn, &change.entity_id, field)? {
        return Ok(());
    }
    let local_value = current_field_value(conn, &change.entity_id, field)?;
    let variant_a = format!(
        "v{}",
        local_change_id
            .unwrap_or("local")
            .chars()
            .take(6)
            .collect::<String>()
    );
    let variant_b = format!("v{}", change.change_id.chars().take(6).collect::<String>());
    conn.execute(
        "INSERT OR IGNORE INTO conflicts(task_id, field, base_version, local_value, remote_value,
         local_change_id, remote_change_id, variant_a, variant_b, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            change.entity_id,
            field,
            change.base_version,
            local_value,
            remote_value,
            local_change_id,
            change.change_id,
            variant_a,
            variant_b,
            change.created_at
        ],
    )?;
    Ok(())
}

fn current_field_value(conn: &Connection, task_id: &str, field: &str) -> Result<String> {
    let task = get_task(conn, task_id)?;
    match field {
        "title" => Ok(task.title),
        "description" => Ok(task.description),
        "project" => Ok(task.project_key),
        "status" => Ok(task.status),
        "priority" => Ok(task.priority),
        "deleted" => Ok(if task.deleted { "1" } else { "0" }.to_string()),
        _ => bail!("error unknown-field field={field}"),
    }
}

fn str_payload(payload: &Value, key: &str) -> Result<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .with_context(|| format!("payload missing {key}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn memory() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ensure_schema(&conn).unwrap();
        conn
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

    #[test]
    fn resolves_short_refs_when_unambiguous() {
        let conn = memory();
        let project = create_project(&conn, "app").unwrap();
        conn.execute(
            "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', 'test', '', ?, 'inbox', 'none', 't', 't')",
            [project.key],
        )
        .unwrap();
        let task = resolve_task_ref(&conn, "7KQ").unwrap();
        assert_eq!(task.id, "7KQ9A1X4MV2P8D6R");
    }

    #[test]
    fn rejects_ambiguous_refs() {
        let conn = memory();
        let project = create_project(&conn, "app").unwrap();
        for id in ["7KQ9A1X4MV2P8D6R", "7KQZZZZZZZZZZZZZ"] {
            conn.execute(
                "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
                 VALUES (?, 'test', '', ?, 'inbox', 'none', 't', 't')",
                params![id, project.key],
            )
            .unwrap();
        }
        assert!(resolve_task_ref(&conn, "7KQ").is_err());
    }

    #[test]
    fn creates_conflict_on_same_field_version_mismatch() {
        let conn = memory();
        let project = create_project(&conn, "app").unwrap();
        conn.execute(
            "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', 'local', '', ?, 'inbox', 'none', 't', 't')",
            [project.key],
        )
        .unwrap();
        set_field_version(&conn, "7KQ9A1X4MV2P8D6R", "title", "localchange").unwrap();
        let change = ChangeWire {
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
        apply_remote_set_field(&conn, &change, false).unwrap();
        assert!(conflict_exists(&conn, "7KQ9A1X4MV2P8D6R", "title").unwrap());
    }
}
