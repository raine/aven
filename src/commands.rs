use anyhow::{Result, bail};
use serde_json::json;
use sqlx::{Connection as _, SqliteConnection};

use crate::choices::{PRIORITIES, STATUSES, validate_choice};
use crate::cli::{
    AddArgs, ConfigCommand, ConfigSubcommand, ConflictCommand, ConflictSubcommand, LabelCommand,
    LabelSubcommand, ListArgs, NoteArgs, ProjectCommand, ProjectPathSubcommand, ProjectSubcommand,
    RefArgs, SearchArgs, ShowArgs, UpdateArgs,
};
use crate::config;
use crate::db::{insert_change, set_field_version};
use crate::ids::{new_id, now};
use crate::input::{read_optional_text, read_required_text};
use crate::labels::{list_labels, normalize_label, resolve_labels};
use crate::mutation::{apply_field_value, set_task_field};
use crate::projects::{
    add_project_path, create_project, list_projects, resolve_existing_project,
    resolve_project_for_add,
};
use crate::query::{self, TaskFilters, TaskSort};
use crate::refs::{display_ref, display_suffix, get_task, resolve_task_ref};
use crate::render::quote;
use crate::task_render::{
    conflict_variant_value, print_conflicts, print_task, print_task_line_item,
};
use crate::types::Task;

pub(crate) async fn cmd_add(conn: &mut SqliteConnection, args: AddArgs) -> Result<()> {
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
        search: None,
    };
    for item in query::list_task_items(conn, filters, TaskSort::Updated).await? {
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

pub(crate) async fn cmd_note(conn: &mut SqliteConnection, args: NoteArgs) -> Result<()> {
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

pub(crate) async fn cmd_project(conn: &mut SqliteConnection, args: ProjectCommand) -> Result<()> {
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

pub(crate) async fn cmd_delete_restore(
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

pub(crate) async fn cmd_config(args: ConfigCommand) -> Result<()> {
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

pub(crate) async fn cmd_conflict(conn: &mut SqliteConnection, args: ConflictCommand) -> Result<()> {
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
