use anyhow::{Context, Result};
use aven_core::db::Database;
use ratatui::DefaultTerminal;

mod app;
mod app_attachments;
mod app_authoring;
mod app_config;
mod app_conflicts;
mod app_dispatch;
mod app_edit;
mod app_filters;
mod app_intake;
mod app_lifecycle;
mod app_navigation;
mod app_onboarding;
mod app_overlay_submit;
mod app_projects;
mod app_search;
mod app_update;
mod attachment_controller;
mod authoring;
mod bounded_history;
mod columns;
mod config_overlay;
mod conflict_flow;
mod detail_selection;
mod event;
mod inline_images;
mod markdown;
mod natural_add_runtime;
mod navigation;
mod overlay;
mod platform;
mod preview_controller;
mod shortcut_buffer;
mod store;
mod text;
mod theme;
mod time;
mod toast;
mod ui;
mod widgets;

struct TerminalSession {
    terminal: DefaultTerminal,
    keyboard_enhancement: platform::KeyboardEnhancementGuard,
    restored: bool,
}

impl TerminalSession {
    fn init() -> Result<Self> {
        let terminal = init_terminal()?;
        let keyboard_enhancement = match platform::KeyboardEnhancementGuard::enable() {
            Ok(guard) => guard,
            Err(error) => {
                ratatui::restore();
                return Err(error);
            }
        };
        Ok(Self {
            terminal,
            keyboard_enhancement,
            restored: false,
        })
    }

    fn terminal_mut(&mut self) -> &mut DefaultTerminal {
        &mut self.terminal
    }

    fn restore(&mut self) -> Result<()> {
        let result = self.keyboard_enhancement.disable();
        ratatui::restore();
        if result.is_ok() {
            self.restored = true;
        }
        result
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if !self.restored {
            let _ = self.keyboard_enhancement.disable();
            ratatui::restore();
        }
    }
}

pub(crate) async fn resolve_launch(
    database: &Database,
    workspace: &crate::workspaces::Workspace,
    args: crate::cli::TuiArgs,
) -> Result<store::TuiLaunch> {
    store::TuiLaunch::resolve(database, workspace, args).await
}

pub(crate) async fn run(
    database: Database,
    workspace: crate::workspaces::Workspace,
    launch: store::TuiLaunch,
    db_path: std::path::PathBuf,
    config: crate::config::AppConfig,
) -> Result<()> {
    run_with_welcome_intro(database, workspace, launch, db_path, config, false).await
}

pub(crate) async fn run_demo(
    database: Database,
    workspace: crate::workspaces::Workspace,
    launch: store::TuiLaunch,
    db_path: std::path::PathBuf,
    config: crate::config::AppConfig,
) -> Result<()> {
    run_with_welcome_intro(database, workspace, launch, db_path, config, true).await
}

async fn run_with_welcome_intro(
    database: Database,
    workspace: crate::workspaces::Workspace,
    launch: store::TuiLaunch,
    db_path: std::path::PathBuf,
    config: crate::config::AppConfig,
    show_welcome_intro: bool,
) -> Result<()> {
    let mut app = app::App::new_with_view_state(database, workspace, launch.view_state).await?;
    app.set_add_task_db_path(db_path);
    match launch.startup {
        store::TuiStartup::AddTaskOnly { natural } => {
            let mut terminal = TerminalSession::init()?;
            let result = app
                .run_add_task_only(terminal.terminal_mut(), natural, config)
                .await;
            let restore_result = terminal.restore();
            let result = match (result, restore_result) {
                (Ok(value), Ok(())) => Ok(value),
                (Err(error), _) | (Ok(_), Err(error)) => Err(error),
            };
            if let Ok(Some(message)) = &result {
                println!("{message}");
            }
            result.map(|_| ())
        }
        startup => {
            app.set_config(config);
            app.start_update_check();
            match startup {
                store::TuiStartup::Browse if show_welcome_intro => app.show_welcome_intro(),
                store::TuiStartup::Browse => app.maybe_open_onboarding().await,
                store::TuiStartup::AddTask { natural } => {
                    app.open_add_task_on_start(natural).await?;
                }
                store::TuiStartup::Detail { task_id } => {
                    app.open_task_on_start(&task_id)?;
                }
                store::TuiStartup::AddTaskOnly { .. } => unreachable!(),
            }
            let mut terminal = TerminalSession::init()?;
            let result = app.run(terminal.terminal_mut()).await;
            let restore_result = terminal.restore();
            result.and(restore_result)
        }
    }
}

fn init_terminal() -> Result<DefaultTerminal> {
    match ratatui::try_init() {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            ratatui::restore();
            Err(error).context("initialize terminal")
        }
    }
}
