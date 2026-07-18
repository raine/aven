use anyhow::Result;
use aven_core::db::Database;

use crate::cli::{WorkspaceCommand, WorkspaceSubcommand};
use crate::render::quote;

pub(crate) async fn cmd_workspace(database: &Database, args: WorkspaceCommand) -> Result<()> {
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
    }
    Ok(())
}
