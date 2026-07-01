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
mod tui;
mod types;
mod undo;
mod workspaces;

#[cfg(test)]
mod test_support;

pub use cli::Cli;

use cli::{BackupSubcommand, Commands, DaemonSubcommand, InternalSubcommand, TmuxSubcommand};
use commands::{
    cmd_add, cmd_backup, cmd_bulk_update, cmd_config, cmd_conflict, cmd_context,
    cmd_delete_restore, cmd_dep, cmd_doctor, cmd_epic, cmd_export, cmd_import,
    cmd_internal_natural_add, cmd_label, cmd_list, cmd_note, cmd_note_delete, cmd_prime,
    cmd_project, cmd_search, cmd_show, cmd_skill, cmd_text, cmd_tmux_add_task_popup, cmd_update,
    cmd_workspace,
};
use db::open_db;
use sync::{run_server, sync_client};
use workspaces::{resolve_active_workspace, set_active_workspace};

pub async fn run_cli() -> Result<()> {
    let cli = cli::parse();
    logging::init(cli.command.metadata().log_mode)?;

    match cli.command {
        Commands::Backup(backup)
            if matches!(backup.command, Some(BackupSubcommand::Restore(_))) =>
        {
            let config = config::AppConfig::load()?;
            let db_path = config::resolve_db_path(cli.db, &config)?;
            let Some(BackupSubcommand::Restore(args)) = backup.command else {
                unreachable!()
            };
            commands::cmd_backup_restore(&db_path, args).await
        }
        Commands::Server(args) => {
            let config = config::AppConfig::load()?;
            run_server(args, config).await
        }
        Commands::Skill => cmd_skill().await,
        Commands::Config(args) => cmd_config(args).await,
        Commands::Internal(args) => {
            let config = config::AppConfig::load()?;
            let db_path = config::resolve_db_path(cli.db, &config)?;
            let pool = open_db(&db_path).await?;
            let mut conn = pool.acquire().await?;
            match args.command {
                InternalSubcommand::NaturalAdd(args) => {
                    cmd_internal_natural_add(&mut conn, &config, args).await
                }
            }
        }
        Commands::Daemon(args) => {
            let config = config::AppConfig::load()?;
            let db_path = config::resolve_db_path(cli.db, &config)?;
            match args.command {
                None => daemon::run(daemon::DaemonRunArgs { db_path, config }).await,
                Some(DaemonSubcommand::Install) => {
                    daemon::install(daemon::ServiceInstallArgs { db_path, config })
                }
                Some(DaemonSubcommand::Uninstall) => daemon::uninstall(),
            }
        }
        Commands::Tmux(args) => match args.command {
            TmuxSubcommand::AddTaskPopup(args) => cmd_tmux_add_task_popup(args),
        },
        command => {
            let db_flag_set = cli.db.is_some();
            let workspace = cli.workspace;
            let config = load_config_for_command(db_flag_set, &command)?;
            let db_path = config::resolve_db_path(cli.db, &config)?;
            let pool = open_db(&db_path).await?;
            let mut conn = pool.acquire().await?;
            let metadata = command.metadata();
            if metadata.needs_workspace {
                let cwd = std::env::current_dir()?;
                let workspace =
                    resolve_active_workspace(&mut conn, workspace.as_deref(), &config, &cwd)
                        .await?;
                set_active_workspace(workspace);
            }
            drop(conn);
            if let Commands::Tui(args) = &command {
                if args.add_task_only {
                    return tui::run_add_task(
                        pool,
                        args.project.as_deref(),
                        args.natural,
                        db_path,
                        config,
                    )
                    .await;
                }
                return tui::run(
                    pool,
                    args.project.as_deref(),
                    args.add_task,
                    args.natural,
                    config,
                )
                .await;
            }
            let mut conn = pool.acquire().await?;
            let should_wake = metadata.wakes_daemon;
            let result = match command {
                Commands::Add(args) => cmd_add(&mut conn, &config, args).await,
                Commands::Context(args) => cmd_context(&mut conn, args).await,
                Commands::Show(args) => cmd_show(&mut conn, args).await,
                Commands::List(args) => cmd_list(&mut conn, args).await,
                Commands::Search(args) => cmd_search(&mut conn, args).await,
                Commands::Backup(args) => cmd_backup(&mut conn, &db_path, args).await,
                Commands::Dep(args) => cmd_dep(&mut conn, args).await,
                Commands::Epic(args) => cmd_epic(&mut conn, args).await,
                Commands::BulkUpdate(args) => cmd_bulk_update(&mut conn, args).await,
                Commands::Prime(args) => cmd_prime(&mut conn, args).await,
                Commands::Update(args) => cmd_update(&mut conn, args).await,
                Commands::Note(args) => cmd_note(&mut conn, args).await,
                Commands::NoteDelete(args) => cmd_note_delete(&mut conn, args).await,
                Commands::Export(args) => cmd_export(&mut conn, args).await,
                Commands::Import(args) => cmd_import(&mut conn, &db_path, args).await,
                Commands::Label(args) => cmd_label(&mut conn, args).await,
                Commands::Project(args) => cmd_project(&mut conn, args).await,
                Commands::Delete(args) => cmd_delete_restore(&mut conn, args, true).await,
                Commands::Restore(args) => cmd_delete_restore(&mut conn, args, false).await,
                Commands::Conflict(args) => cmd_conflict(&mut conn, args).await,
                Commands::Sync(args) => sync_client(&mut conn, args, &config).await,
                Commands::Workspace(args) => cmd_workspace(&mut conn, args).await,
                Commands::Text(args) => cmd_text(&mut conn, args).await,
                Commands::Doctor(args) => {
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
                Commands::Tui(_) => unreachable!(),
                Commands::Config(_)
                | Commands::Daemon(_)
                | Commands::Server(_)
                | Commands::Internal(_)
                | Commands::Tmux(_)
                | Commands::Skill => unreachable!(),
            };
            if result.is_ok()
                && should_wake
                && config.sync.enabled
                && let Ok(addr) = config.wake_addr()
            {
                tracing::debug!(wake_addr = %addr, "waking daemon after local mutation");
                daemon::wake(addr);
            }
            result
        }
    }
}

fn load_config_for_command(_db_flag_set: bool, _command: &Commands) -> Result<config::AppConfig> {
    config::AppConfig::load()
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
        let project = create_project(&mut conn, "app").await.unwrap();
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
        assert!(
            conflict_exists(
                &mut conn,
                crate::workspaces::active_workspace_id().as_str(),
                "7KQ9A1X4MV2P8D6R",
                "title"
            )
            .await
            .unwrap()
        );
    }
}
