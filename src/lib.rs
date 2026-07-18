use anyhow::Result;

mod change_log;
mod choices;
mod cli;
mod command_metadata;
mod commands;
mod config;
mod config_edit;
mod daemon;
mod db;
mod due;
mod fuzzy;
mod ids;
mod input;
mod labels;
mod logging;
mod mutation;
mod operations;
mod projects;
mod query;
mod queue;
mod refs;
mod render;
mod signals;
mod sync;
mod task_enrichment;
mod task_fields;
mod task_intake;
mod task_render;
mod time_input;
mod tui;
mod types;
mod undo;
mod update;
mod workspaces;

mod attachments;

#[cfg(test)]
mod test_support;

pub use cli::Cli;

use cli::{BackupSubcommand, Commands, DaemonSubcommand, InternalSubcommand, SkillSubcommand};
use commands::{
    cmd_add, cmd_attachment, cmd_backup, cmd_bulk_update, cmd_config, cmd_conflict, cmd_context,
    cmd_delete_restore, cmd_dep, cmd_doctor, cmd_edit, cmd_epic, cmd_export, cmd_import,
    cmd_internal_natural_add, cmd_label, cmd_list, cmd_note, cmd_note_delete, cmd_prime,
    cmd_project, cmd_search, cmd_self_update, cmd_show, cmd_skill, cmd_skill_install, cmd_text,
    cmd_workspace,
};
use db::open_db;
use sync::{run_server, sync_client};
use workspaces::resolve_active_workspace;

pub async fn run_cli() -> Result<()> {
    let cli = cli::parse();
    let metadata = cli.command.metadata();
    logging::init(metadata.log_mode)?;

    match CliDispatch::from(cli.command) {
        CliDispatch::Standalone(command) => dispatch_standalone(cli.db, command).await,
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
    List(cli::ListArgs),
    Note(cli::NoteArgs),
    NoteDelete(cli::NoteDeleteArgs),
    Prime(cli::PrimeArgs),
    Project(cli::ProjectCommand),
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
            Commands::Project(args) => Self::database(DatabaseCommand::Project(args)),
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
            Commands::Server(args) => Self::Standalone(StandaloneCommand::Server(args)),
            Commands::Sync(args) => Self::database(DatabaseCommand::Sync(args)),
            Commands::Tui(args) => Self::Tui(args),
            Commands::Internal(args) => Self::Standalone(StandaloneCommand::Internal(args)),
        }
    }
}

async fn dispatch_standalone(
    db: Option<std::path::PathBuf>,
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
        StandaloneCommand::Update(args) => cmd_self_update(args).await,
        StandaloneCommand::Internal(args) => {
            let config = config::AppConfig::load()?;
            let db_path = config::resolve_db_path(db, &config)?;
            let pool = open_db(&db_path).await?;
            let mut conn = pool.acquire().await?;
            match args.command {
                InternalSubcommand::NaturalAdd(args) => {
                    cmd_internal_natural_add(&mut conn, &config, args).await
                }
            }
        }
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
    let pool = open_db(&db_path).await?;
    let mut conn = pool.acquire().await?;
    let cwd = std::env::current_dir()?;
    let resolved_workspace =
        resolve_active_workspace(&mut conn, workspace.as_deref(), &config, &cwd).await?;
    drop(conn);

    if args.add_task_only {
        return tui::run_add_task(
            pool,
            resolved_workspace,
            args.project.as_deref(),
            args.natural,
            db_path,
            config,
        )
        .await;
    }
    tui::run(
        pool,
        resolved_workspace,
        args.project.as_deref(),
        args.add_task,
        args.natural,
        db_path,
        config,
    )
    .await
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
    let pool = open_db(&db_path).await?;
    let mut conn = pool.acquire().await?;
    let resolved_workspace = if metadata.needs_workspace {
        let cwd = std::env::current_dir()?;
        Some(resolve_active_workspace(&mut conn, workspace.as_deref(), &config, &cwd).await?)
    } else {
        None
    };
    drop(conn);
    let mut conn = pool.acquire().await?;
    let command_workspace = || {
        resolved_workspace
            .as_ref()
            .expect("command requires workspace context")
    };
    let should_wake = metadata.wakes_daemon;
    let result = match command {
        DatabaseCommand::Add(args) => cmd_add(&mut conn, command_workspace(), &config, args).await,
        DatabaseCommand::Attachment(args) => {
            cmd_attachment(&mut conn, command_workspace(), &config, &db_path, args).await
        }
        DatabaseCommand::Context(args) => cmd_context(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Show(args) => cmd_show(&mut conn, command_workspace(), args).await,
        DatabaseCommand::List(args) => cmd_list(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Search(args) => cmd_search(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Backup { output } => {
            cmd_backup(
                &mut conn,
                &config,
                &db_path,
                cli::BackupCommand {
                    command: None,
                    output,
                },
            )
            .await
        }
        DatabaseCommand::Dep(args) => cmd_dep(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Epic(args) => cmd_epic(&mut conn, command_workspace(), args).await,
        DatabaseCommand::BulkUpdate(args) => {
            cmd_bulk_update(&mut conn, command_workspace(), args).await
        }
        DatabaseCommand::Prime(args) => cmd_prime(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Edit(args) => cmd_edit(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Note(args) => cmd_note(&mut conn, command_workspace(), args).await,
        DatabaseCommand::NoteDelete(args) => {
            cmd_note_delete(&mut conn, command_workspace(), args).await
        }
        DatabaseCommand::Export(args) => cmd_export(&mut conn, args).await,
        DatabaseCommand::Import(args) => cmd_import(&mut conn, &db_path, args).await,
        DatabaseCommand::Label(args) => cmd_label(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Project(args) => cmd_project(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Delete(args) => {
            cmd_delete_restore(&mut conn, command_workspace(), args, true).await
        }
        DatabaseCommand::Restore(args) => {
            cmd_delete_restore(&mut conn, command_workspace(), args, false).await
        }
        DatabaseCommand::Conflict(args) => cmd_conflict(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Sync(args) => sync_client(&mut conn, &db_path, args, &config).await,
        DatabaseCommand::Workspace(args) => cmd_workspace(&mut conn, args).await,
        DatabaseCommand::Text(args) => cmd_text(&mut conn, command_workspace(), args).await,
        DatabaseCommand::Doctor(args) => {
            cmd_doctor(
                &mut conn,
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
    use serde_json::json;

    use super::*;
    use crate::db::{conflict_exists, set_field_version};
    use crate::ids::{BASE32, encode_crockford};
    use crate::projects::{create_project, normalize_key};
    use crate::test_support::test_conn;

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

    #[tokio::test]
    async fn creates_conflict_on_same_field_version_mismatch() {
        let (_temp, mut conn) = test_conn().await;
        let project = create_project(&mut conn, &crate::workspaces::Workspace::default(), "app")
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tasks(id, title, description, project_id, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', 'local', '', ?, 'inbox', 'none', 't', 't')",
        )
        .bind(project.id)
        .execute(&mut *conn)
        .await
        .unwrap();
        set_field_version(&mut conn, "7KQ9A1X4MV2P8D6R", "title", "localchange")
            .await
            .unwrap();
        let change = sync::ChangeWire {
            change_id: "remotechange1234".to_string(),
            client_id: "remote".to_string(),
            local_seq: 1,
            entity_type: "task".to_string(),
            entity_id: "7KQ9A1X4MV2P8D6R".to_string(),
            field: Some("title".to_string()),
            op_type: "set_field".to_string(),
            payload: json!({ "value": "remote" }),
            base_version: Some("base".to_string()),
            created_at: "t".to_string(),
            server_seq: Some(1),
        };
        sync::apply_remote_set_field(&mut conn, &change, false)
            .await
            .unwrap();
        let task_id = "7KQ9A1X4MV2P8D6R".parse().unwrap();
        assert!(
            conflict_exists(
                &mut conn,
                &crate::workspaces::default_workspace_id(),
                &task_id,
                "title"
            )
            .await
            .unwrap()
        );
    }
}
