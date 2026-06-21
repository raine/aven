use anyhow::Result;
use sqlx::SqlitePool;

mod app;
mod config_overlay;
mod event;
mod markdown;
mod navigation;
mod overlay;
mod store;
mod theme;
mod ui;
mod widgets;

pub(crate) async fn run(pool: SqlitePool) -> Result<()> {
    let app = app::App::new(pool).await?;
    let mut terminal = ratatui::init();
    let result = app.run(&mut terminal).await;
    ratatui::restore();
    result
}
