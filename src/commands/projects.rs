use anyhow::Result;
use sqlx::SqliteConnection;

use crate::cli::{ProjectCommand, ProjectPathSubcommand, ProjectSubcommand, SearchArgs};
use crate::operations::{
    add_project_path_operation, create_project_operation, list_project_paths_operation,
    remove_project_path_operation, rename_project_operation,
};
use crate::projects::list_projects;
use crate::render::{changed_text, quote};

pub(crate) async fn cmd_project(conn: &mut SqliteConnection, args: ProjectCommand) -> Result<()> {
    match args.command {
        ProjectSubcommand::Create { name, path } => {
            let outcome = create_project_operation(conn, &name, path.as_deref()).await?;
            let project = outcome.project;
            println!(
                "created-project {} prefix={} name={}",
                project.key,
                project.prefix,
                quote(&project.name)
            );
        }
        ProjectSubcommand::List(args) => cmd_projects(conn, args).await?,
        ProjectSubcommand::Rename {
            project,
            new_name,
            prefix,
        } => {
            let workspace = crate::workspaces::active_workspace();
            let outcome =
                rename_project_operation(conn, &workspace, &project, &new_name, prefix.as_deref())
                    .await?;
            println!(
                "renamed-project {} changed={} old={} old_prefix={} prefix={} name={}",
                outcome.project.key,
                changed_text(outcome.changed),
                outcome.previous.key,
                outcome.previous.prefix,
                outcome.project.prefix,
                quote(&outcome.project.name)
            );
            if outcome.changed && outcome.config_mapping {
                println!("updated-config-project-mapping {}", outcome.project.key);
            }
        }
        ProjectSubcommand::Path { command } => match command {
            ProjectPathSubcommand::Add { project, path } => {
                let outcome = add_project_path_operation(conn, &project, &path).await?;
                println!(
                    "added-project-path {} path={} config={}",
                    outcome.project.key,
                    quote(&outcome.path),
                    quote(&outcome.config_path.display().to_string())
                );
            }
            ProjectPathSubcommand::Remove { project, path } => {
                let outcome = remove_project_path_operation(conn, &project, &path).await?;
                println!(
                    "removed-project-path {} path={} config={}",
                    outcome.project.key,
                    quote(&outcome.path),
                    quote(&outcome.config_path.display().to_string())
                );
            }
            ProjectPathSubcommand::List { project } => {
                let paths = list_project_paths_operation(conn, project.as_deref()).await?;
                for item in paths {
                    println!("{} path={}", item.project.key, quote(&item.path));
                }
            }
        },
    }
    Ok(())
}

async fn cmd_projects(conn: &mut SqliteConnection, args: SearchArgs) -> Result<()> {
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
