use anyhow::{Context, Result};
use ratatui::DefaultTerminal;
use sqlx::SqlitePool;

mod app;
mod app_authoring;
mod app_config;
mod app_conflicts;
mod app_dispatch;
mod app_edit;
mod app_filters;
mod app_lifecycle;
mod app_navigation;
mod app_overlay_submit;
mod app_projects;
mod app_search;
mod authoring;
mod columns;
mod config_overlay;
mod conflict_flow;
mod detail_selection;
mod event;
mod markdown;
mod natural_add_runtime;
mod navigation;
mod overlay;
mod platform;
mod shortcut_buffer;
mod store;
mod text;
mod theme;
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

pub(crate) async fn run(
    pool: SqlitePool,
    project: Option<&str>,
    add_task: bool,
    natural: bool,
    db_path: std::path::PathBuf,
    config: crate::config::AppConfig,
) -> Result<()> {
    let mut app = app::App::new(pool, project).await?;
    app.set_add_task_db_path(db_path);
    app.set_config(config);
    if add_task {
        app.open_add_task_on_start(natural).await?;
    }
    let mut terminal = TerminalSession::init()?;
    let result = app.run(terminal.terminal_mut()).await;
    let restore_result = terminal.restore();
    result.and(restore_result)
}

pub(crate) async fn run_add_task(
    pool: SqlitePool,
    project: Option<&str>,
    natural: bool,
    db_path: std::path::PathBuf,
    config: crate::config::AppConfig,
) -> Result<()> {
    let mut app = app::App::new(pool, project).await?;
    app.set_add_task_db_path(db_path);
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

fn init_terminal() -> Result<DefaultTerminal> {
    match ratatui::try_init() {
        Ok(terminal) => Ok(terminal),
        Err(error) => {
            ratatui::restore();
            Err(error).context("initialize terminal")
        }
    }
}
