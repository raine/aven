use std::fs;
use std::path::Path;

use anyhow::{Result, bail};
use sqlx::SqliteConnection;

use crate::cli::{ConflictCommand, ConflictSubcommand};
use crate::input::read_required_text;
use crate::operations::{
    ConflictDetail, conflict_variant_value, list_conflicts, resolve_conflict, task_conflicts,
};
use crate::projects::resolve_existing_project_in_workspace;
use crate::refs::{display_ref, display_suffix, resolve_task_ref};
use crate::render::{print_multiline_block, print_text_diff, quote};
use crate::task_fields::TaskField;
use crate::types::Task;

pub(crate) async fn cmd_conflict(conn: &mut SqliteConnection, args: ConflictCommand) -> Result<()> {
    match args.command {
        ConflictSubcommand::List { project, field } => {
            let project_key = resolve_conflict_project_filter(
                conn,
                crate::workspaces::active_workspace_id().as_str(),
                project,
            )
            .await?;
            let items = list_conflicts(conn, project_key.as_deref(), field.as_deref()).await?;
            for item in items {
                print_conflict_list_item(conn, item).await?;
            }
        }
        ConflictSubcommand::Show { task_ref, field } => {
            let task = resolve_task_ref(conn, &task_ref).await?;
            let details = task_conflicts(conn, &task.id, field.as_deref()).await?;
            for detail in details {
                print_conflict_detail(conn, &task, detail).await?;
            }
        }
        ConflictSubcommand::Diff { task_ref, field } => {
            let task = resolve_task_ref(conn, &task_ref).await?;
            let detail = load_single_conflict_detail(conn, &task, &field).await?;
            print_text_diff("local", &detail.local_value, "remote", &detail.remote_value);
        }
        ConflictSubcommand::Export {
            task_ref,
            field,
            dir,
        } => {
            let task = resolve_task_ref(conn, &task_ref).await?;
            fs::create_dir_all(&dir)?;
            let detail = load_single_conflict_detail(conn, &task, &field).await?;
            export_conflict_variant(&dir, &detail.field, &detail.variant_a, &detail.local_value)?;
            export_conflict_variant(&dir, &detail.field, &detail.variant_b, &detail.remote_value)?;
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

async fn resolve_conflict_project_filter(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    project: Option<String>,
) -> Result<Option<String>> {
    if let Some(project) = project {
        return Ok(Some(
            resolve_existing_project_in_workspace(conn, workspace_id, &project)
                .await?
                .key,
        ));
    }
    Ok(None)
}

async fn print_conflict_list_item(
    conn: &mut SqliteConnection,
    item: crate::operations::ConflictListItem,
) -> Result<()> {
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
    Ok(())
}

async fn print_conflict_detail(
    conn: &mut SqliteConnection,
    task: &Task,
    detail: ConflictDetail,
) -> Result<()> {
    println!(
        "conflict {} field={}",
        display_ref(conn, task).await?,
        detail.field
    );
    let local_value =
        conflict_display_value(conn, &task.workspace_id, &detail.field, &detail.local_value)
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
    Ok(())
}

async fn load_single_conflict_detail(
    conn: &mut SqliteConnection,
    task: &Task,
    field: &str,
) -> Result<ConflictDetail> {
    single_conflict(
        task_conflicts(conn, &task.id, Some(field)).await?,
        &task.id,
        field,
    )
}

fn export_conflict_variant(dir: &Path, field: &str, variant: &str, value: &str) -> Result<()> {
    let path = dir.join(format!("{field}-{variant}.md"));
    fs::write(&path, value)?;
    println!(
        "exported variant={} path={}",
        variant,
        quote(&path.display().to_string())
    );
    Ok(())
}

pub(super) async fn conflict_display_value(
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
