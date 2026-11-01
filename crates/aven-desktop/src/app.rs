//! Application start, after isolation has passed.
use anyhow::{Context as _, Result};
use aven_host::config::AppConfig;
use gpui::{
    App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px, size,
};
use gpui_platform::application;

use crate::adapter::{self, Launch, Prefs};
use crate::assets::{Assets, load_fonts};
use crate::isolation::Isolation;
use crate::shell::Root;
use crate::theme::Theme;
use crate::{actions, input};

/// Records what this launch resolved, for launchers and reviewers.
fn write_launch_record(isolation: &Isolation, queue_rows: usize) -> Result<()> {
    let paths: serde_json::Map<_, _> = isolation
        .resolved_paths()
        .iter()
        .map(|(name, path)| (name.to_string(), path.display().to_string().into()))
        .collect();
    let record = serde_json::json!({
        "pid": std::process::id(),
        "executable": std::env::current_exe()?.display().to_string(),
        "paths": paths,
        "queue_rows": queue_rows,
    });
    std::fs::write(
        &isolation.launch_record,
        serde_json::to_vec_pretty(&record)?,
    )
    .context("write launch record")
}

pub fn run(isolation: Isolation) -> Result<()> {
    isolation.create_directories()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let launch = Launch {
        database: isolation.database.clone(),
        config: AppConfig::load()?,
        storage: isolation.protected_storage(),
    };
    let (session, replies) = adapter::start(runtime.handle().clone(), launch)?;
    let queue_rows = session.snapshot.ordered().count();
    let prefs = Prefs::path(&isolation.database);
    application().with_assets(Assets).run(move |cx: &mut App| {
        load_fonts(cx);
        cx.set_global(Theme::dark());
        cx.bind_keys(input::key_bindings());
        cx.bind_keys(actions::key_bindings());
        cx.on_action(|_: &actions::Quit, cx| cx.quit());
        cx.set_menus(actions::menus());
        let bounds = Bounds::centered(None, size(px(1440.), px(900.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                titlebar: Some(TitlebarOptions {
                    title: Some("Aven Dev".into()),
                    appears_transparent: true,
                    traffic_light_position: Some(point(px(14.), px(21.))),
                }),
                window_min_size: Some(size(px(960.), px(600.))),
                ..Default::default()
            },
            |window, cx| {
                cx.new(|cx| {
                    let mut root = Root::new(session, replies, prefs, window, cx);
                    root.follow_system(window, cx);
                    root
                })
            },
        )
        .expect("the window opens");
        cx.activate(true);
        // Launchers read this as proof the window opened.
        if let Err(error) = write_launch_record(&isolation, queue_rows) {
            eprintln!("aven-desktop failed: {error:#}");
            cx.quit();
        }
        cx.on_window_closed(|cx, _| cx.quit()).detach();
    });
    Ok(())
}
