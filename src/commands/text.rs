use std::fs;

use anyhow::{Result, bail};
use aven_core::db::Database;
use sha2::{Digest, Sha256};

use crate::cli::{TextCommand, TextSubcommand};
use crate::input::read_required_text;
use crate::operations::TaskUpdate;
use crate::render::{print_multiline_block, print_text_diff, quote};
use crate::task_fields::TaskField;
use crate::workspaces::Workspace;

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
    database: &Database,
    workspace: &Workspace,
    args: TextCommand,
) -> Result<()> {
    match args.command {
        TextSubcommand::Get(args) => {
            ensure_description_field(&args.field)?;
            let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
            let value = TaskField::Description.current_value(&task);
            let hash = sha256_hex(&value);
            let display_refs = database.display_ref_context(&workspace.id).await?;
            let task_ref = display_refs.display_ref(&task);
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
            let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
            let current = TaskField::Description.current_value(&task);
            let candidate = fs::read_to_string(&args.file)?;
            print_text_diff("current", &current, "candidate", &candidate);
        }
        TextSubcommand::Set(args) => {
            ensure_description_field(&args.field)?;
            let value = read_required_text(None, args.file.as_deref(), args.stdin, "text")?;
            let task = database.resolve_task_ref(workspace, &args.task_ref).await?;
            let current = TaskField::Description.current_value(&task);
            let actual = sha256_hex(&current);
            if actual != args.if_sha256 {
                bail!(
                    "error text-hash-mismatch field=description expected={} actual={}",
                    args.if_sha256,
                    actual
                );
            }
            let outcome = database
                .update_task(
                    workspace,
                    &task.id,
                    TaskUpdate {
                        description: Some(value),
                        ..Default::default()
                    },
                )
                .await?;
            let display_refs = database.display_ref_context(&workspace.id).await?;
            println!(
                "updated {} field=description sha256={}",
                display_refs.display_ref(&outcome.task),
                sha256_hex(&outcome.task.description)
            );
        }
    }
    Ok(())
}
