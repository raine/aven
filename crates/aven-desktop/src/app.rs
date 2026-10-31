//! Application start, after isolation has passed.
use anyhow::{Context as _, Result};
use chrono::Local;
use gpui::{
    App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px, size,
};
use gpui_platform::application;

use crate::isolation::Isolation;
use crate::shell::{Palette, Shell, load_groups};

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
        isolation.root.join("launch.json"),
        serde_json::to_vec_pretty(&record)?,
    )
    .context("write launch record")
}

pub fn run(isolation: Isolation) -> Result<()> {
    isolation.create_directories()?;
    let runtime = tokio::runtime::Runtime::new()?;
    let groups = runtime.block_on(load_groups(&isolation.database, None, Local::now()))?;
    write_launch_record(
        &isolation,
        groups.iter().map(|group| group.rows.len()).sum(),
    )?;
    application().run(move |cx: &mut App| {
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
            |_, cx| {
                cx.new(|_| Shell {
                    palette: Palette::dark(),
                    groups,
                })
            },
        )
        .expect("the window opens");
        cx.activate(true);
        cx.on_window_closed(|cx, _| cx.quit()).detach();
    });
    Ok(())
}
