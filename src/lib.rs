use anyhow::Result;

use aven_core::{
    change_log, choices, db, ids, labels, query, queue, refs, task_fields, types, undo,
};
mod attachments;
mod cli;
mod command_metadata;
mod commands;
mod config;
mod config_edit;
mod daemon;
mod due;
mod input;
mod logging;
mod notification;
mod operations;
mod projects;
mod recurrence_input;
mod render;
mod schedule_input;
mod signals;
mod sync;
mod task_intake;
mod task_render;
mod time_input;
mod tui;
mod update;
mod workspaces;

#[cfg(test)]
mod test_support;

pub use cli::Cli;

use cli::{BackupSubcommand, Commands, DaemonSubcommand, InternalSubcommand, SkillSubcommand};
use commands::{
    cmd_add, cmd_attachment, cmd_backup, cmd_bulk_update, cmd_config, cmd_conflict, cmd_context,
    cmd_delete_restore, cmd_demo, cmd_dep, cmd_doctor, cmd_edit, cmd_epic, cmd_export, cmd_import,
    cmd_internal_demo_snapshot, cmd_internal_natural_add, cmd_label, cmd_list, cmd_metadata,
    cmd_note, cmd_note_delete, cmd_prime, cmd_project, cmd_recur, cmd_search, cmd_self_update,
    cmd_show, cmd_skill, cmd_skill_install, cmd_text, cmd_workspace,
};
use sync::{run_server, sync_client};
use workspaces::resolve_active_workspace_with_database;

pub async fn run_cli() -> Result<()> {
    let cli = cli::parse();
    let metadata = cli.command.metadata();
    logging::init(metadata.log_mode)?;

    match CliDispatch::from(cli.command) {
        CliDispatch::Standalone(command) => {
            dispatch_standalone(cli.db, cli.workspace, command).await
        }
        CliDispatch::Database(command) => {
            dispatch_database(cli.db, cli.workspace, metadata, *command).await
        }
        CliDispatch::Tui(args) => dispatch_tui(cli.db, cli.workspace, args).await,
    }
}

enum CliDispatch {
    Standalone(StandaloneCommand),
    Database(Box<DatabaseCommand>),
    Tui(cli::TuiArgs),
}

enum StandaloneCommand {
    BackupRestore(cli::BackupRestoreArgs),
    Config(cli::ConfigCommand),
    Daemon(cli::DaemonArgs),
    Demo,
    Internal(cli::InternalCommand),
    Server(cli::ServerArgs),
    Skill(cli::SkillCommand),
    Update(cli::SelfUpdateArgs),
}

enum DatabaseCommand {
    Add(cli::AddArgs),
    Attachment(cli::AttachmentCommand),
    Backup { output: Option<std::path::PathBuf> },
    BulkUpdate(cli::BulkUpdateArgs),
    Conflict(cli::ConflictCommand),
    Context(cli::ContextArgs),
    Delete(cli::RefArgs),
    Dep(cli::DepCommand),
    Doctor(cli::DoctorArgs),
    Edit(cli::TaskEditArgs),
    Epic(cli::EpicCommand),
    Export(cli::ExportArgs),
    Import(cli::ImportArgs),
    Label(cli::LabelCommand),
    Metadata(cli::MetadataCommand),
    List(cli::ListArgs),
    Note(cli::NoteArgs),
    NoteDelete(cli::NoteDeleteArgs),
    Prime(cli::PrimeArgs),
    Project(cli::ProjectCommand),
    Recur(cli::RecurCommand),
    Restore(cli::RefArgs),
    Search(cli::TaskSearchArgs),
    Show(cli::ShowArgs),
    Sync(cli::SyncArgs),
    Text(cli::TextCommand),
    Workspace(cli::WorkspaceCommand),
}

impl CliDispatch {
    fn database(command: DatabaseCommand) -> Self {
        Self::Database(Box::new(command))
    }
}

impl From<Commands> for CliDispatch {
    fn from(command: Commands) -> Self {
        match command {
            Commands::Add(args) => Self::database(DatabaseCommand::Add(args)),
            Commands::Attachment(args) => Self::database(DatabaseCommand::Attachment(args)),
            Commands::Dep(args) => Self::database(DatabaseCommand::Dep(args)),
            Commands::Epic(args) => Self::database(DatabaseCommand::Epic(args)),
            Commands::Context(args) => Self::database(DatabaseCommand::Context(args)),
            Commands::Show(args) => Self::database(DatabaseCommand::Show(args)),
            Commands::List(args) => Self::database(DatabaseCommand::List(args)),
            Commands::Search(args) => Self::database(DatabaseCommand::Search(args)),
            Commands::BulkUpdate(args) => Self::database(DatabaseCommand::BulkUpdate(args)),
            Commands::Prime(args) => Self::database(DatabaseCommand::Prime(args)),
            Commands::Edit(args) => Self::database(DatabaseCommand::Edit(args)),
            Commands::Update(args) => Self::Standalone(StandaloneCommand::Update(args)),
            Commands::Note(args) => Self::database(DatabaseCommand::Note(args)),
            Commands::NoteDelete(args) => Self::database(DatabaseCommand::NoteDelete(args)),
            Commands::Delete(args) => Self::database(DatabaseCommand::Delete(args)),
            Commands::Restore(args) => Self::database(DatabaseCommand::Restore(args)),
            Commands::Text(args) => Self::database(DatabaseCommand::Text(args)),
            Commands::Label(args) => Self::database(DatabaseCommand::Label(args)),
            Commands::Metadata(args) => Self::database(DatabaseCommand::Metadata(args)),
            Commands::Project(args) => Self::database(DatabaseCommand::Project(args)),
            Commands::Recur(args) => Self::database(DatabaseCommand::Recur(args)),
            Commands::Workspace(args) => Self::database(DatabaseCommand::Workspace(args)),
            Commands::Conflict(args) => Self::database(DatabaseCommand::Conflict(args)),
            Commands::Config(args) => Self::Standalone(StandaloneCommand::Config(args)),
            Commands::Backup(args) => match args.command {
                Some(BackupSubcommand::Restore(args)) => {
                    Self::Standalone(StandaloneCommand::BackupRestore(args))
                }
                None => Self::database(DatabaseCommand::Backup {
                    output: args.output,
                }),
            },
            Commands::Export(args) => Self::database(DatabaseCommand::Export(args)),
            Commands::Import(args) => Self::database(DatabaseCommand::Import(args)),
            Commands::Skill(args) => Self::Standalone(StandaloneCommand::Skill(args)),
            Commands::Doctor(args) => Self::database(DatabaseCommand::Doctor(args)),
            Commands::Daemon(args) => Self::Standalone(StandaloneCommand::Daemon(args)),
            Commands::Demo => Self::Standalone(StandaloneCommand::Demo),
            Commands::Server(args) => Self::Standalone(StandaloneCommand::Server(args)),
            Commands::Sync(args) => Self::database(DatabaseCommand::Sync(args)),
            Commands::Tui(args) => Self::Tui(args),
            Commands::Internal(args) => Self::Standalone(StandaloneCommand::Internal(args)),
        }
    }
}

async fn dispatch_standalone(
    db: Option<std::path::PathBuf>,
    workspace: Option<String>,
    command: StandaloneCommand,
) -> Result<()> {
    match command {
        StandaloneCommand::BackupRestore(args) => {
            let config = config::AppConfig::load()?;
            let db_path = config::resolve_db_path(db, &config)?;
            commands::cmd_backup_restore(&config, &db_path, args).await
        }
        StandaloneCommand::Server(args) => {
            let config = config::AppConfig::load()?;
            run_server(args, config).await
        }
        StandaloneCommand::Skill(args) => match args.command {
            None => cmd_skill(),
            Some(SkillSubcommand::Install(args)) => cmd_skill_install(args),
        },
        StandaloneCommand::Config(args) => cmd_config(args).await,
        StandaloneCommand::Demo => cmd_demo(db, workspace).await,
        StandaloneCommand::Update(args) => cmd_self_update(args).await,
        StandaloneCommand::Internal(args) => match args.command {
            InternalSubcommand::DemoSnapshot(args) => {
                cmd_internal_demo_snapshot(db, workspace, args).await
            }
            InternalSubcommand::NaturalAdd(args) => {
                let config = config::AppConfig::load()?;
                let db_path = config::resolve_db_path(db, &config)?;
                let database = db::Database::open(&db_path).await?;
                cmd_internal_natural_add(&database, &config, args).await
            }
        },
        StandaloneCommand::Daemon(args) => {
            let config = config::AppConfig::load()?;
            let db_path = config::resolve_db_path(db, &config)?;
            match args.command {
                None => daemon::run(daemon::DaemonRunArgs { db_path, config }).await,
                Some(DaemonSubcommand::Install(args)) => {
                    daemon::install(daemon::ServiceInstallArgs {
                        db_path,
                        config,
                        program: args.program,
                    })
                }
                Some(DaemonSubcommand::Uninstall) => daemon::uninstall(),
                Some(DaemonSubcommand::Restart) => daemon::restart(),
                Some(DaemonSubcommand::Repair(args)) => daemon::repair(daemon::ServiceRepairArgs {
                    db_path,
                    config,
                    program: args.program,
                    if_installed: args.if_installed,
                }),
            }
        }
    }
}

async fn dispatch_tui(
    db: Option<std::path::PathBuf>,
    workspace: Option<String>,
    args: cli::TuiArgs,
) -> Result<()> {
    let config = config::AppConfig::load()?;
    let db_path = config::resolve_db_path(db, &config)?;
    let database = aven_core::db::Database::open(&db_path).await?;
    let cwd = std::env::current_dir()?;
    let resolved_workspace =
        resolve_active_workspace_with_database(&database, workspace.as_deref(), &config, &cwd)
            .await?;
    let launch = tui::resolve_launch(&database, &resolved_workspace, args).await?;

    tui::run(database, resolved_workspace, launch, db_path, config).await
}

async fn resolve_command_workspace(
    database: &db::Database,
    workspace: Option<&str>,
    config: &config::AppConfig,
) -> Result<workspaces::Workspace> {
    let cwd = std::env::current_dir()?;
    resolve_active_workspace_with_database(database, workspace, config, &cwd).await
}

async fn dispatch_database(
    db: Option<std::path::PathBuf>,
    workspace: Option<String>,
    metadata: command_metadata::CommandMetadata,
    command: DatabaseCommand,
) -> Result<()> {
    let db_flag_set = db.is_some();
    let config = config::AppConfig::load()?;
    let db_path = config::resolve_db_path(db, &config)?;
    let database = db::Database::open(&db_path).await?;
    let should_wake = metadata.wakes_daemon;
    let result = match command {
        DatabaseCommand::Add(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_add(&database, &workspace, &config, args).await
        }
        DatabaseCommand::Attachment(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_attachment(&database, &workspace, &config, &db_path, args).await
        }
        DatabaseCommand::Context(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_context(&database, &workspace, args).await
        }
        DatabaseCommand::Show(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_show(&database, &workspace, args).await
        }
        DatabaseCommand::List(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_list(&database, &workspace, args).await
        }
        DatabaseCommand::Search(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_search(&database, &workspace, args).await
        }
        DatabaseCommand::Backup { output } => {
            cmd_backup(
                &database,
                &config,
                &db_path,
                cli::BackupCommand {
                    command: None,
                    output,
                },
            )
            .await
        }
        DatabaseCommand::Dep(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_dep(&database, &workspace, args).await
        }
        DatabaseCommand::Epic(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_epic(&database, &workspace, args).await
        }
        DatabaseCommand::BulkUpdate(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_bulk_update(&database, &workspace, args).await
        }
        DatabaseCommand::Prime(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_prime(&database, &workspace, args).await
        }
        DatabaseCommand::Edit(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_edit(&database, &workspace, args).await
        }
        DatabaseCommand::Note(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_note(&database, &workspace, args).await
        }
        DatabaseCommand::NoteDelete(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_note_delete(&database, &workspace, args).await
        }
        DatabaseCommand::Export(args) => cmd_export(&database, args).await,
        DatabaseCommand::Import(args) => cmd_import(&database, &db_path, args).await,
        DatabaseCommand::Label(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_label(&database, &workspace, args).await
        }
        DatabaseCommand::Metadata(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_metadata(&database, &workspace, args).await
        }
        DatabaseCommand::Project(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_project(&database, &workspace, args).await
        }
        DatabaseCommand::Recur(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_recur(&database, &workspace, args).await
        }
        DatabaseCommand::Delete(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_delete_restore(&database, &workspace, args, true).await
        }
        DatabaseCommand::Restore(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_delete_restore(&database, &workspace, args, false).await
        }
        DatabaseCommand::Conflict(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_conflict(&database, &workspace, args).await
        }
        DatabaseCommand::Sync(args) => sync_client(&database, args, &config).await,
        DatabaseCommand::Workspace(args) => cmd_workspace(&database, args).await,
        DatabaseCommand::Text(args) => {
            let workspace =
                resolve_command_workspace(&database, workspace.as_deref(), &config).await?;
            cmd_text(&database, &workspace, args).await
        }
        DatabaseCommand::Doctor(args) => {
            cmd_doctor(
                &database,
                &config,
                &db_path,
                db_flag_set,
                workspace.as_deref(),
                args.integrity,
                args.json,
            )
            .await
        }
    };
    if result.is_ok() && should_wake {
        daemon::wake_if_enabled(&config);
    }
    result
}

#[cfg(test)]
mod tests {
    use crate::ids::{BASE32, encode_crockford};
    use crate::projects::normalize_key;

    #[test]
    fn normalizes_project_keys() {
        assert_eq!(
            normalize_key("Agentic Task Manager"),
            "agentic-task-manager"
        );
    }

    #[test]
    fn encodes_80_bit_ids_as_16_chars() {
        let id = encode_crockford(&[0xff; 10]);
        assert_eq!(id.len(), 16);
        assert!(id.chars().all(|ch| BASE32.contains(&(ch as u8))));
    }
}
