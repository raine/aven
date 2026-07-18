use anyhow::Result;
use aven_core::db::Database;

use crate::cli::{LabelCommand, LabelListArgs, LabelSubcommand};
use crate::render::{changed_text, print_json_pretty};
use crate::workspaces::Workspace;

pub(crate) async fn cmd_labels(
    database: &Database,
    workspace: &Workspace,
    args: LabelListArgs,
) -> Result<()> {
    let mut labels = database
        .list_labels(&workspace.id, args.search.as_deref())
        .await?;
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
    database: &Database,
    workspace: &Workspace,
    args: LabelCommand,
) -> Result<()> {
    match args.command {
        LabelSubcommand::Create { name } => {
            let outcome = database.create_label(workspace, &name).await?;
            println!("created-label {}", outcome.name);
        }
        LabelSubcommand::Delete { name } => {
            let outcome = database.delete_label(workspace, &name).await?;
            println!(
                "deleted-label {} changed={}",
                outcome.name,
                changed_text(outcome.changed),
            );
        }
        LabelSubcommand::List(args) => cmd_labels(database, workspace, args).await?,
    }
    Ok(())
}
