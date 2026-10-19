use anyhow::Result;
use aven_core::db::Database;

use crate::cli::{WorkspaceCommand, WorkspaceSubcommand};
use crate::config::AppConfig;
use crate::render::quote;

pub(crate) async fn cmd_workspace(
    database: &Database,
    config: &AppConfig,
    args: WorkspaceCommand,
) -> Result<()> {
    match args.command {
        WorkspaceSubcommand::List => {
            for workspace in database.list_workspaces().await? {
                println!("{} name={}", workspace.key, quote(&workspace.name));
            }
        }
        WorkspaceSubcommand::Create { name } => {
            let workspace = database.create_workspace(&name).await?;
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
            let workspace = database.rename_workspace(&workspace, &new_name).await?;
            println!(
                "renamed-workspace {} name={}",
                workspace.key,
                quote(&workspace.name)
            );
        }
        WorkspaceSubcommand::Delete { workspace, force } => {
            let workspace = database.resolve_workspace(&workspace).await?;
            let entries =
                crate::workspaces::config_entries_for_workspace(database, config, &workspace)
                    .await?;
            let (workspace, contents) = database
                .delete_workspace(&workspace.id, &entries, force)
                .await?;
            println!(
                "deleted-workspace {} tasks={} open={}",
                workspace.key, contents.tasks, contents.open_tasks
            );
        }
    }
    Ok(())
}
