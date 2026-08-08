use anyhow::{Context, Result};
use aven_core::db::Database;
use serde::Serialize;

use crate::cli::{MetadataCommand, MetadataSubcommand};
use crate::render::print_json_pretty;
use crate::workspaces::Workspace;

#[derive(Serialize)]
struct MetadataFieldJson {
    id: String,
    key: String,
    task_count: usize,
    series_count: usize,
}

pub(crate) async fn cmd_metadata(
    database: &Database,
    workspace: &Workspace,
    args: MetadataCommand,
) -> Result<()> {
    match args.command {
        MetadataSubcommand::List { json } => {
            let fields = database.list_metadata_fields(&workspace.id).await?;
            if json {
                let fields = fields
                    .into_iter()
                    .map(|usage| MetadataFieldJson {
                        id: usage.field.id.to_string(),
                        key: usage.field.key,
                        task_count: usage.task_count,
                        series_count: usage.series_count,
                    })
                    .collect::<Vec<_>>();
                print_json_pretty(&fields)?;
            } else {
                for usage in fields {
                    println!(
                        "{} id={} tasks={} series={}",
                        usage.field.key, usage.field.id, usage.task_count, usage.series_count
                    );
                }
            }
        }
        MetadataSubcommand::Show { key, json } => {
            let field = database
                .find_metadata_field(&workspace.id, &key)
                .await?
                .context("error unknown-metadata-field")?;
            let usage = database
                .list_metadata_fields(&workspace.id)
                .await?
                .into_iter()
                .find(|usage| usage.field.id == field.id)
                .context("error unknown-metadata-field")?;
            let output = MetadataFieldJson {
                id: field.id.to_string(),
                key: field.key,
                task_count: usage.task_count,
                series_count: usage.series_count,
            };
            if json {
                print_json_pretty(&output)?;
            } else {
                println!(
                    "{} id={} tasks={} series={}",
                    output.key, output.id, output.task_count, output.series_count
                );
            }
        }
        MetadataSubcommand::Rename { key, new_key } => {
            let field = database
                .rename_metadata_field(workspace, &key, &new_key)
                .await?;
            println!("renamed metadata field id={} key={}", field.id, field.key);
        }
    }
    Ok(())
}
