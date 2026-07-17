use anyhow::Result;
use sqlx::SqliteConnection;

use crate::cli::{LabelCommand, LabelListArgs, LabelSubcommand};
use crate::labels::list_labels_in_workspace;
use crate::operations::{create_label_operation, delete_label_operation};
use crate::render::{changed_text, print_json_pretty};
use crate::workspaces::Workspace;

pub(crate) async fn cmd_labels(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: LabelListArgs,
) -> Result<()> {
    let mut labels = list_labels_in_workspace(conn, &workspace.id, args.search.as_deref()).await?;
    if let Some(limit) = args.limit {
        labels.truncate(limit);
    }
    if args.json {
        print_json_pretty(&labels)?;
    } else {
        for label in labels {
            println!("{label}");
        }
    }
    Ok(())
}

pub(crate) async fn cmd_label(
    conn: &mut SqliteConnection,
    workspace: &Workspace,
    args: LabelCommand,
) -> Result<()> {
    match args.command {
        LabelSubcommand::Create { name } => {
            let outcome = create_label_operation(conn, workspace, &name).await?;
            println!("created-label {}", outcome.name);
        }
        LabelSubcommand::Delete { name } => {
            let outcome = delete_label_operation(conn, workspace, &name).await?;
            println!(
                "deleted-label {} changed={}",
                outcome.name,
                changed_text(outcome.changed),
            );
        }
        LabelSubcommand::List(args) => cmd_labels(conn, workspace, args).await?,
    }
    Ok(())
}
