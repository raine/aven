use anyhow::Result;
use sqlx::SqlitePool;

mod app;
mod app_conflicts;
mod app_edit;
mod app_filters;
mod authoring;
mod config_overlay;
mod conflict_flow;
mod event;
mod markdown;
mod navigation;
mod overlay;
mod store;
mod theme;
mod ui;
mod widgets;

pub(crate) async fn run(pool: SqlitePool) -> Result<()> {
    let app = app::App::new_for_inferred_project(pool).await?;
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal).await;
    ratatui::restore();
    result
}
