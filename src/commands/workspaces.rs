use anyhow::Result;
use sqlx::SqliteConnection;

use crate::cli::{WorkspaceCommand, WorkspaceSubcommand};
use crate::render::quote;
use crate::workspaces::{create_workspace, list_workspaces, rename_workspace};

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
