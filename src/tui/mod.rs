use anyhow::Result;
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
mod authoring;
mod config_overlay;
mod conflict_flow;
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

pub(crate) async fn run(
    pool: SqlitePool,
    project: Option<&str>,
    add_task: bool,
    natural: bool,
    config: crate::config::AppConfig,
) -> Result<()> {
    let mut app = app::App::new(pool, project).await?;
    app.set_config(config);
    if add_task {
        app.open_add_task_on_start(natural).await?;
    }
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal).await;
    ratatui::restore();
    result
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
    let mut terminal = ratatui::init();
    let result = app.run_add_task_only(&mut terminal, natural, config).await;
    ratatui::restore();
    if let Ok(Some(message)) = &result {
        println!("{message}");
    }
    result.map(|_| ())
}
