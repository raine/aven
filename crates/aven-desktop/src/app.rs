//! Application start.
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use aven_host::app_bundle::enclosing_bundle;
use aven_host::config::{AppConfig, resolve_db_path};
use gpui::{
    App, Bounds, DisplayId, Global, Pixels, Size, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, point, prelude::*, px, size,
};
use gpui_platform::application;

use crate::adapter::{self, Launch, Prefs, SavedWindow, WindowMode};
use crate::assets::{Assets, load_fonts};
use crate::shell::Root;
use crate::theme::Theme;
use crate::{actions, input, platform};

/// How long an outside quit leaves the store to finish queued writes.
const STORE_GRACE: Duration = Duration::from_millis(150);

/// How often the app checks whether its bundle was replaced.
const UPDATE_POLL: Duration = Duration::from_secs(5);

/// The window title.
const TITLE: &str = "Aven";

/// Identifies the file at `path`. A bundle swap puts a new file there, while
/// this process keeps running the old one.
fn file_identity(path: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;

    let metadata = std::fs::metadata(path).ok()?;
    Some((metadata.dev(), metadata.ino()))
}

/// Whether `now` is a different file from the one this process started from. A
/// missing file is a replacement still in progress, or an uninstall.
fn was_replaced(started: (u64, u64), now: Option<(u64, u64)>) -> bool {
    now.is_some_and(|now| now != started)
}

/// Follows an update of the bundle, whoever installed it. A running app keeps
/// executing the old file, so it quits normally and reopens as soon as the owner
/// is not using it.
fn relaunch_after_update(cx: &mut App) {
    let Ok(executable) = std::env::current_exe() else {
        return;
    };
    let Some(bundle) = enclosing_bundle(&executable) else {
        return;
    };
    let Some(started) = file_identity(&executable) else {
        return;
    };
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(UPDATE_POLL).await;
            if !was_replaced(started, file_identity(&executable)) {
                continue;
            }
            let finished = cx.update(|cx| {
                if platform::is_active() {
                    return false;
                }
                match platform::reopen_after_exit(&bundle) {
                    Ok(()) => with_root(cx, Root::quit),
                    Err(error) => tracing::warn!(%error, "could not schedule a relaunch"),
                }
                true
            });
            if finished {
                return;
            }
        }
    })
    .detach();
}

/// The app's one window. It stays open for the life of the app, and closing it
/// only takes it off screen.
struct MainWindow(WindowHandle<Root>);

impl Global for MainWindow {}

fn with_root(cx: &mut App, f: impl FnOnce(&mut Root, &mut gpui::Window, &mut gpui::Context<Root>)) {
    let window = cx.global::<MainWindow>().0;
    if let Err(error) = window.update(cx, f) {
        tracing::error!(%error, "the window could not be reached");
    }
}

fn show_window(cx: &mut App) {
    with_root(cx, |_, window, _| window.activate_window());
    cx.activate(true);
}

/// Actions that need no window, so they work with it closed. An action from an
/// open window runs while GPUI holds that window, which nothing else can update
/// until the action returns, so those that use it wait with `defer`.
pub fn register_actions(cx: &mut App) {
    cx.on_action(|_: &actions::Quit, cx| cx.defer(|cx| with_root(cx, Root::quit)));
    cx.on_action(|_: &actions::Hide, cx| cx.hide());
    cx.on_action(|_: &actions::HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &actions::ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &actions::ShowWindow, cx| cx.defer(show_window));
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

/// Runs on the owner's own configuration and database, as the CLI resolves them.
pub fn run() -> Result<()> {
    let config = AppConfig::load()?;
    let database = resolve_db_path(None, &config)?;
    let prefs = Prefs::path(&database);
    let runtime = tokio::runtime::Runtime::new()?;
    let launch = Launch {
        database,
        config,
        storage: None,
    };
    let (session, replies) = adapter::start(runtime.handle().clone(), launch)?;
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
                        title: Some(TITLE.into()),
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
        relaunch_after_update(cx);
        cx.activate(true);
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_different_file_counts_as_replaced() {
        assert!(!was_replaced((1, 2), Some((1, 2))));
        assert!(was_replaced((1, 2), Some((1, 3))));
        assert!(was_replaced((1, 2), Some((4, 2))));
        assert!(!was_replaced((1, 2), None));
    }

    #[test]
    fn a_swapped_bundle_executable_is_a_different_file() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("aven");
        std::fs::write(&executable, "old").unwrap();
        let started = file_identity(&executable).unwrap();
        // The old file stays open in the running app, so its inode stays taken.
        let kept = directory.path().join("previous");
        std::fs::rename(&executable, &kept).unwrap();
        std::fs::write(&executable, "new").unwrap();
        assert!(was_replaced(started, file_identity(&executable)));
    }

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
