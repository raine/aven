//! Application start, after isolation has passed.
use std::time::Duration;

use anyhow::{Context as _, Result};
use aven_host::config::AppConfig;
use gpui::{
    App, Bounds, DisplayId, Global, Pixels, Size, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, point, prelude::*, px, size,
};
use gpui_platform::application;

use crate::adapter::{self, Launch, Prefs, SavedWindow, WindowMode};
use crate::assets::{Assets, load_fonts};
use crate::isolation::Isolation;
use crate::shell::Root;
use crate::theme::Theme;
use crate::{actions, input};

/// How long an outside quit leaves the store to finish queued writes.
const STORE_GRACE: Duration = Duration::from_millis(150);

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

/// The app's one window. It stays open for the life of the app, and closing it
/// only takes it off screen.
struct MainWindow(WindowHandle<Root>);

impl Global for MainWindow {}

fn with_root(cx: &mut App, f: impl FnOnce(&mut Root, &mut gpui::Window, &mut gpui::Context<Root>)) {
    let window = cx.global::<MainWindow>().0;
    let _ = window.update(cx, f);
}

fn show_window(cx: &mut App) {
    with_root(cx, |_, window, _| window.activate_window());
    cx.activate(true);
}

/// Actions that need no window, so they work with it closed.
pub fn register_actions(cx: &mut App) {
    cx.on_action(|_: &actions::Quit, cx| with_root(cx, Root::quit));
    cx.on_action(|_: &actions::Hide, cx| cx.hide());
    cx.on_action(|_: &actions::HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &actions::ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &actions::ShowWindow, cx| show_window(cx));
}

/// The default frame, which also stands in for a saved one no display can show.
const DEFAULT_SIZE: Size<f32> = Size {
    width: 1440.,
    height: 900.,
};
/// The part of a window's top edge that must be on a display to reach it.
const REACHABLE: Size<f32> = Size {
    width: 120.,
    height: 24.,
};

/// The saved frame, shrunk to `display`, if that display shows its title bar.
/// `display` is visible bounds.
fn restored_bounds(saved: &SavedWindow, display: Bounds<Pixels>) -> Option<Bounds<Pixels>> {
    let bounds = Bounds::new(
        point(px(saved.x), px(saved.y)),
        size(px(saved.width), px(saved.height)),
    );
    let top = Bounds::new(bounds.origin, size(bounds.size.width, px(REACHABLE.height)));
    let shown = display.intersect(&top).size;
    (shown.width >= px(REACHABLE.width) && shown.height >= px(REACHABLE.height))
        .then(|| Bounds::new(bounds.origin, bounds.size.min(&display.size)))
}

/// Where the window opens: its saved display, or the primary one if that left.
fn window_placement(saved: Option<&SavedWindow>, cx: &App) -> (WindowBounds, Option<DisplayId>) {
    let restored = saved.and_then(|saved| {
        let home = cx
            .displays()
            .into_iter()
            .find(|display| display.uuid().ok().map(|uuid| uuid.to_string()) == saved.display)
            .or_else(|| cx.primary_display())?;
        let bounds = restored_bounds(saved, home.visible_bounds())?;
        let bounds = match saved.mode {
            WindowMode::Windowed => WindowBounds::Windowed(bounds),
            WindowMode::Zoomed => WindowBounds::Maximized(bounds),
            WindowMode::FullScreen => WindowBounds::Fullscreen(bounds),
        };
        Some((bounds, Some(home.id())))
    });
    restored.unwrap_or_else(|| {
        let default = size(px(DEFAULT_SIZE.width), px(DEFAULT_SIZE.height));
        (
            WindowBounds::Windowed(Bounds::centered(None, default, cx)),
            None,
        )
    })
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
    let saved = Prefs::load(&prefs);
    let app = application().with_assets(Assets);
    app.on_reopen(show_window);
    app.run(move |cx: &mut App| {
        load_fonts(cx);
        cx.set_global(Theme::dark());
        cx.bind_keys(input::key_bindings());
        cx.bind_keys(actions::key_bindings());
        register_actions(cx);
        cx.set_menus(actions::menus());
        let (bounds, display_id) = window_placement(saved.window.as_ref(), cx);
        let view = saved.view.clone();
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(bounds),
                    display_id,
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
                        let mut root = Root::new(session, replies, saved, prefs, window, cx);
                        root.follow_system(window, cx);
                        if let Some(view) = &view {
                            root.session.reopen_view(view);
                        }
                        let this = cx.weak_entity();
                        window.on_window_should_close(cx, move |window, cx| {
                            let _ = this.update(cx, |root, cx| root.close(window, cx));
                            false
                        });
                        root
                    })
                },
            )
            .expect("the window opens");
        cx.set_global(MainWindow(window));
        // Anything but Quit, such as logging out, ends here. Nothing runs on the
        // main thread after this, so the store gets a moment without a reply.
        cx.on_app_quit(|cx| {
            let mut started = false;
            with_root(cx, |root, window, cx| started = root.begin_quit(window, cx));
            let timer = cx.background_executor().timer(STORE_GRACE);
            async move {
                if started {
                    timer.await;
                }
            }
        })
        .detach();
        cx.activate(true);
        // Launchers read this as proof the window opened.
        if let Err(error) = write_launch_record(&isolation, queue_rows) {
            eprintln!("aven-desktop failed: {error:#}");
            cx.quit();
        }
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f32, y: f32) -> SavedWindow {
        SavedWindow {
            display: None,
            x,
            y,
            width: 1000.,
            height: 700.,
            mode: WindowMode::Windowed,
        }
    }

    fn display(x: f32, width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(x), px(0.)), size(px(width), px(height)))
    }

    #[test]
    fn a_frame_on_its_display_comes_back_as_it_was() {
        let restored = restored_bounds(&frame(1200., 40.), display(0., 2560., 1415.)).unwrap();
        assert_eq!(restored.origin, point(px(1200.), px(40.)));
        assert_eq!(restored.size, size(px(1000.), px(700.)));
    }

    #[test]
    fn a_frame_from_a_wider_display_is_dropped() {
        assert_eq!(
            restored_bounds(&frame(1700., 40.), display(0., 1512., 945.)),
            None
        );
    }

    #[test]
    fn a_title_bar_above_the_menu_bar_or_past_an_edge_is_dropped() {
        let display = Bounds::new(point(px(0.), px(33.)), size(px(1512.), px(900.)));
        assert_eq!(restored_bounds(&frame(100., -40.), display), None);
        assert_eq!(restored_bounds(&frame(1450., 100.), display), None);
    }

    #[test]
    fn a_frame_larger_than_its_display_shrinks() {
        let restored = restored_bounds(&frame(10., 10.), display(0., 800., 600.)).unwrap();
        assert_eq!(restored.size, size(px(800.), px(600.)));
    }
}
