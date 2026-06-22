use anyhow::Result;
use clap::Parser;

mod choices;
mod cli;
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

use cli::{Commands, ConflictCommand, ConflictSubcommand, DaemonSubcommand, TmuxSubcommand};
use commands::{
    cmd_add, cmd_bulk_update, cmd_config, cmd_conflict, cmd_delete_restore, cmd_doctor, cmd_label,
    cmd_labels, cmd_list, cmd_note, cmd_prime, cmd_project, cmd_projects, cmd_show, cmd_skill,
    cmd_tmux_add_task_popup, cmd_update, cmd_workspace,
};
use db::open_db;
use sync::{run_server, sync_client};
use workspaces::{resolve_active_workspace, set_active_workspace};

pub async fn run_cli() -> Result<()> {
    let cli = Cli::parse();
    let log_mode = match &cli.command {
        Commands::Server(_) => logging::LogMode::Server,
        Commands::Daemon(_) => logging::LogMode::Daemon,
        Commands::Tui(_) => logging::LogMode::Tui,
        _ => logging::LogMode::Cli,
    };
    logging::init(log_mode)?;

    match cli.command {
        Commands::Server(args) => {
            let config = config::AppConfig::load()?;
            run_server(args, config).await
        }
        Commands::Skill => cmd_skill().await,
        Commands::Config(args) => cmd_config(args).await,
        Commands::Daemon(args) => {
            let config = config::AppConfig::load()?;
            let db_path = config::resolve_db_path(cli.db, &config)?;
            match args.command {
                DaemonSubcommand::Run => {
                    daemon::run(daemon::DaemonRunArgs { db_path, config }).await
                }
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
            if command_needs_workspace(&command) {
                let cwd = std::env::current_dir()?;
                let workspace =
                    resolve_active_workspace(&mut conn, workspace.as_deref(), &config, &cwd)
                        .await?;
                set_active_workspace(workspace);
            }
            drop(conn);
            if let Commands::Tui(args) = &command {
                if args.add_task {
                    return tui::run_add_task(pool, args.project.as_deref(), args.natural, config)
                        .await;
                }
                return tui::run(pool, args.project.as_deref(), config).await;
            }
            let mut conn = pool.acquire().await?;
            let should_wake = command_should_wake(&command);
            let result = match command {
                Commands::Add(args) => cmd_add(&mut conn, &config, args).await,
                Commands::Show(args) => cmd_show(&mut conn, args).await,
                Commands::List(args) => cmd_list(&mut conn, args).await,
                Commands::BulkUpdate(args) => cmd_bulk_update(&mut conn, args).await,
                Commands::Prime(args) => cmd_prime(&mut conn, args).await,
                Commands::Update(args) => cmd_update(&mut conn, args).await,
                Commands::Note(args) => cmd_note(&mut conn, args).await,
                Commands::Projects(args) => cmd_projects(&mut conn, args).await,
                Commands::Labels(args) => cmd_labels(&mut conn, args).await,
                Commands::Label(args) => cmd_label(&mut conn, args).await,
                Commands::Project(args) => cmd_project(&mut conn, args).await,
                Commands::Delete(args) => cmd_delete_restore(&mut conn, args, true).await,
                Commands::Restore(args) => cmd_delete_restore(&mut conn, args, false).await,
                Commands::Conflict(args) => cmd_conflict(&mut conn, args).await,
                Commands::Sync(args) => sync_client(&mut conn, args, &config).await,
                Commands::Workspace(args) => cmd_workspace(&mut conn, args).await,
                Commands::Doctor => {
                    cmd_doctor(
                        &mut conn,
                        &config,
                        &db_path,
                        db_flag_set,
                        workspace.as_deref(),
                    )
                    .await
                }
                Commands::Tui(_) => unreachable!(),
                Commands::Config(_)
                | Commands::Daemon(_)
                | Commands::Server(_)
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

fn command_needs_workspace(command: &Commands) -> bool {
    matches!(
        command,
        Commands::Add(_)
            | Commands::Show(_)
            | Commands::List(_)
            | Commands::BulkUpdate(_)
            | Commands::Prime(_)
            | Commands::Update(_)
            | Commands::Note(_)
            | Commands::Projects(_)
            | Commands::Labels(_)
            | Commands::Label(_)
            | Commands::Project(_)
            | Commands::Delete(_)
            | Commands::Restore(_)
            | Commands::Conflict(_)
            | Commands::Tui(_)
    )
}

fn command_should_wake(command: &Commands) -> bool {
    matches!(command, Commands::BulkUpdate(args) if !args.dry_run)
        || matches!(
            command,
            Commands::Add(_)
                | Commands::Update(_)
                | Commands::Note(_)
                | Commands::Label(_)
                | Commands::Project(_)
                | Commands::Workspace(_)
                | Commands::Delete(_)
                | Commands::Restore(_)
                | Commands::Conflict(ConflictCommand {
                    command: ConflictSubcommand::Resolve { .. }
                })
        )
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::{conflict_exists, set_field_version};
    use crate::ids::{BASE32, encode_crockford};
    use crate::projects::{create_project, normalize_key};
    use crate::refs::resolve_task_ref;
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
    async fn resolves_short_refs_when_unambiguous() {
        let (_temp, mut conn) = test_conn().await;
        let project = create_project(&mut conn, "app").await.unwrap();
        sqlx::query!(
            "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', 'test', '', ?, 'inbox', 'none', 't', 't')",
            project.key,
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        let task = resolve_task_ref(&mut conn, "7KQ").await.unwrap();
        assert_eq!(task.id, "7KQ9A1X4MV2P8D6R");
    }

    #[tokio::test]
    async fn rejects_ambiguous_refs() {
        let (_temp, mut conn) = test_conn().await;
        let project = create_project(&mut conn, "app").await.unwrap();
        for id in ["7KQ9A1X4MV2P8D6R", "7KQZZZZZZZZZZZZZ"] {
            sqlx::query!(
                "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
                 VALUES (?, 'test', '', ?, 'inbox', 'none', 't', 't')",
                id,
                project.key,
            )
            .execute(&mut *conn)
            .await
            .unwrap();
        }
        assert!(resolve_task_ref(&mut conn, "7KQ").await.is_err());
    }

    #[tokio::test]
    async fn creates_conflict_on_same_field_version_mismatch() {
        let (_temp, mut conn) = test_conn().await;
        let project = create_project(&mut conn, "app").await.unwrap();
        sqlx::query!(
            "INSERT INTO tasks(id, title, description, project_key, status, priority, created_at, updated_at)
             VALUES ('7KQ9A1X4MV2P8D6R', 'local', '', ?, 'inbox', 'none', 't', 't')",
            project.key,
        )
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
