//! Application start.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use aven_core::ids::TaskId;
use aven_host::config::{AppConfig, config_file_path, resolve_db_path};
#[cfg(target_os = "macos")]
use aven_host::desktop_install::{self, Layout, MacSystem};
use aven_host::notify::{self, Target};
use gpui::{
    App, Bounds, DisplayId, Global, Pixels, Size, TitlebarOptions, WindowBounds, WindowHandle,
    WindowOptions, point, prelude::*, px, size,
};
use gpui_platform::application;

#[cfg(target_os = "macos")]
use crate::adapter::ThemePref;
use crate::adapter::{self, Launch, Prefs, SavedWindow, WindowMode};
use crate::assets::{Assets, load_fonts};
use crate::platform::enclosing_bundle;
use crate::session::Session;
#[cfg(target_os = "macos")]
use crate::setup::Setup;
use crate::shell::Root;
use crate::theme::Theme;
#[cfg(target_os = "macos")]
use crate::theme::theme;
use crate::toast::Toast;
use crate::updates::Updates;
use crate::{actions, input, launcher, platform};

/// How long an outside quit leaves the store to finish queued writes.
const STORE_GRACE: Duration = Duration::from_millis(150);

/// How often the app checks whether its bundle was replaced.
const UPDATE_POLL: Duration = Duration::from_secs(5);

/// The window title.
const TITLE: &str = "Aven";
/// The windows' Wayland app ID and X11 `WM_CLASS`, which group them with the
/// desktop entry and icon of the same name in `linux/`. macOS ignores it.
pub(crate) const APP_ID: &str = "fi.zendit.Aven";

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
                // The app's own update opens the new version itself.
                if crate::updates::restarting(cx) {
                    return true;
                }
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

pub(crate) fn with_root(
    cx: &mut App,
    f: impl FnOnce(&mut Root, &mut gpui::Window, &mut gpui::Context<Root>),
) {
    // Setup shows before the main window exists.
    let Some(window) = cx.try_global::<MainWindow>().map(|main| main.0) else {
        return;
    };
    if let Err(error) = window.update(cx, f) {
        tracing::error!(%error, "the window could not be reached");
    }
}

pub(crate) fn show_window(cx: &mut App) {
    if !cx.has_global::<MainWindow>() {
        cx.activate(true);
        return;
    }
    with_root(cx, |_, window, _| window.activate_window());
    cx.activate(true);
}

/// Brings the window to the front for a later launch or a click, with the
/// token the window system gave that launch or click.
#[cfg(target_os = "linux")]
fn come_forward(activation: &platform::Activation, cx: &mut App) {
    with_root(cx, |_, window, _| platform::activate(window, activation));
}

/// Actions that need no window, so they work with it closed. An action from an
/// open window runs while GPUI holds that window, which nothing else can update
/// until the action returns, so those that use it wait with `defer`.
pub fn register_actions(cx: &mut App) {
    cx.on_action(|_: &actions::Quit, cx| {
        cx.defer(|cx| {
            if cx.has_global::<MainWindow>() {
                with_root(cx, Root::ask_quit);
            } else {
                cx.quit();
            }
        })
    });
    cx.on_action(|_: &actions::Hide, cx| cx.hide());
    cx.on_action(|_: &actions::HideOthers, cx| cx.hide_other_apps());
    cx.on_action(|_: &actions::ShowAll, cx| cx.unhide_other_apps());
    cx.on_action(|_: &actions::ShowWindow, cx| cx.defer(show_window));
}

/// The setup window's frame.
#[cfg(target_os = "macos")]
const SETUP_SIZE: Size<f32> = Size {
    width: 720.,
    height: 720.,
};

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
pub fn run(version: String, updates: Option<Arc<dyn Updates>>) -> Result<()> {
    crate::startup::mark("desktop-entry");
    let config = AppConfig::load()?;
    let automatic_checks = config.update.automatic_checks;
    let database = resolve_db_path(None, &config)?;
    // macOS opens one app per bundle by itself.
    #[cfg(target_os = "linux")]
    let Some(mut claim) =
        platform::instance::claim(&database, &platform::Activation::from_environment())
    else {
        return Ok(());
    };
    let prefs = Prefs::path(&database);
    let runtime = tokio::runtime::Runtime::new()?;
    let launch = Launch {
        database,
        config,
        config_file: config_file_path().ok(),
        storage: None,
        version: version.clone(),
    };
    crate::startup::mark("runtime-ready");
    let saved = Prefs::load(&prefs);
    // The database waits until this app is the installation that runs the
    // command and the daemon.
    #[cfg(target_os = "macos")]
    if let Some(layout) = Layout::current().ok().filter(desktop_install::needs_setup) {
        let app = application().with_assets(Assets);
        let handle = runtime.handle().clone();
        app.on_reopen(show_window);
        app.run(move |cx: &mut App| {
            prepare_app(cx);
            crate::updates::install(version, updates, automatic_checks, cx);
            open_setup(layout, handle, launch, saved, prefs, cx);
            cx.activate(true);
        });
        return Ok(());
    }
    let finish_store = adapter::prepare(runtime.handle().clone(), launch, saved.view.clone())?;
    let app = application().with_assets(Assets);
    crate::startup::mark("platform-ready");
    let (session, replies) = finish_store()?;
    crate::startup::mark("store-ready");
    app.on_reopen(show_window);
    // The claim itself stays here, held until the app quits.
    #[cfg(target_os = "linux")]
    let raised = claim.raised.take();
    app.run(move |cx: &mut App| {
        crate::startup::mark("app-callback");
        prepare_app(cx);
        crate::updates::install(version, updates, automatic_checks, cx);
        open_main(session, replies, saved, prefs, cx);
        #[cfg(target_os = "linux")]
        if let Some(mut raised) = raised {
            cx.spawn(async move |cx| {
                while let Some(activation) = raised.recv().await {
                    cx.update(|cx| come_forward(&activation, cx));
                }
            })
            .detach();
        }
    });
    Ok(())
}

/// What both windows need: fonts, theme, keys and menus.
fn prepare_app(cx: &mut App) {
    load_fonts(cx);
    crate::startup::mark("fonts-ready");
    cx.set_global(Theme::dark());
    cx.bind_keys(input::key_bindings());
    cx.bind_keys(actions::key_bindings());
    register_actions(cx);
    cx.set_menus(actions::menus());
    follow_notifications(cx);
}

/// What a clicked notification opened before the main window did, which it
/// shows once it opens.
#[derive(Default)]
struct NotifiedTask(Option<Notified>);

impl Global for NotifiedTask {}

/// What a notification opens in the main window.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Notified {
    Task(String, TaskId),
    /// The due digest's tasks, in its workspace.
    Due(String),
}

fn notified(tag: &str) -> Option<Notified> {
    match notify::target(tag)? {
        Target::Task { workspace, task } => Some(Notified::Task(workspace, task.parse().ok()?)),
        Target::Due { workspace } => Some(Notified::Due(workspace)),
    }
}

/// Opens notifications in the app. macOS holds a click that launched the app
/// until a handler exists, so it is registered before setup can show.
fn follow_notifications(cx: &mut App) {
    cx.set_global(NotifiedTask::default());
    cx.on_system_notification_response(|response, cx| {
        show_window(cx);
        let Some(opens) = notified(&response.tag) else {
            return;
        };
        if cx.has_global::<MainWindow>() {
            open_notified(opens, cx);
        } else {
            cx.global_mut::<NotifiedTask>().0 = Some(opens);
        }
    });
}

fn open_notified(opens: Notified, cx: &mut App) {
    with_root(cx, |root, window, cx| match opens {
        Notified::Task(workspace, id) => root.open_launched(workspace, id, window, cx),
        Notified::Due(workspace) => root.open_due(workspace, window, cx),
    });
}

/// Opens setup, and the main window on the database once setup is through.
#[cfg(target_os = "macos")]
fn open_setup(
    layout: Layout,
    handle: tokio::runtime::Handle,
    launch: Launch,
    saved: Prefs,
    prefs: PathBuf,
    cx: &mut App,
) {
    let dark = match saved.theme {
        ThemePref::Dark => Some(true),
        ThemePref::Light => Some(false),
        ThemePref::System => None,
    };
    let on_done = Box::new(move |window: &mut gpui::Window, cx: &mut App| {
        let started =
            adapter::prepare(handle, launch, saved.view.clone()).and_then(|finish| finish());
        match started {
            Ok((session, replies)) => {
                open_main(session, replies, saved, prefs, cx);
                window.remove_window();
            }
            Err(error) => {
                tracing::error!(error = %format_args!("{error:#}"), "the app could not start");
                cx.quit();
            }
        }
    });
    let bounds = Bounds::centered(None, size(px(SETUP_SIZE.width), px(SETUP_SIZE.height)), cx);
    let opened = cx.open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(TITLE.into()),
                appears_transparent: true,
                traffic_light_position: Some(point(px(14.), px(21.))),
            }),
            window_min_size: Some(size(px(560.), px(560.))),
            ..Default::default()
        },
        |window, cx| {
            follow_appearance(dark, window, cx);
            window.on_window_should_close(cx, |_, cx| {
                cx.quit();
                false
            });
            cx.new(|cx| Setup::new(layout, Arc::new(MacSystem), on_done, window, cx))
        },
    );
    if let Err(error) = opened {
        tracing::error!(%error, "the setup window could not open");
        cx.quit();
    }
}

/// Keeps the theme on the saved choice, or on the system's while it changes.
#[cfg(target_os = "macos")]
fn follow_appearance(dark: Option<bool>, window: &mut gpui::Window, cx: &mut App) {
    let apply = move |window: &gpui::Window, cx: &mut App| {
        let dark = dark.unwrap_or_else(|| {
            matches!(
                window.appearance(),
                gpui::WindowAppearance::Dark | gpui::WindowAppearance::VibrantDark
            )
        });
        if theme(cx).dark != dark {
            cx.set_global(if dark { Theme::dark() } else { Theme::light() });
            cx.refresh_windows();
        }
    };
    apply(window, cx);
    window
        .observe_window_appearance(move |window, cx| apply(window, cx))
        .detach();
}

/// Opens the app's one window on a started store.
fn open_main(
    session: Session,
    replies: adapter::Replies,
    saved: Prefs,
    prefs: PathBuf,
    cx: &mut App,
) {
    let (bounds, display_id) = window_placement(saved.window.as_ref(), cx);
    let launcher_prefs = saved.launcher.clone();
    crate::startup::mark("window-start");
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
                // GPUI asks Wayland for activation only for a window with
                // an app ID.
                app_id: Some(APP_ID.into()),
                ..Default::default()
            },
            |window, cx| {
                crate::startup::mark("native-window-ready");
                cx.new(|cx| {
                    let mut root = Root::new(session, replies, saved, prefs, window, cx);
                    root.follow_system(window, cx);
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
    crate::startup::mark("window-ready");
    cx.set_global(MainWindow(window));
    if let Some(error) = launcher::init(&launcher_prefs, cx) {
        with_root(cx, |root, _, cx| {
            let toast = Toast::error("The launcher shortcut isn\u{2019}t set").detail(error);
            root.session.show_toast(toast);
            cx.notify();
        });
    }
    if let Some(task) = cx.global_mut::<NotifiedTask>().0.take() {
        open_notified(task, cx);
    }
    // Anything but Quit, such as logging out, ends here. Nothing runs on the
    // main thread after this, so the store gets a moment without a reply.
    cx.on_app_quit(|cx| {
        crate::updates::discard(cx);
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
    crate::updates::start(cx);
    cx.activate(true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notification_points_at_a_task_only_with_a_valid_id() {
        let task = Some(Notified::Task(
            "home".into(),
            "0123456789ABCDEF".parse().unwrap(),
        ));
        assert_eq!(notified("v1:task:home:0123456789ABCDEF:e1"), task);
        assert_eq!(notified("v1:task:home:nope:e1"), None);
        assert_eq!(notified("launch-failed:3"), None);
        assert_eq!(notified("v1:available:e1"), None);
    }

    #[test]
    fn the_due_digest_opens_the_due_view_of_its_workspace() {
        assert_eq!(
            notified("v1:digest:work:2026-11-06"),
            Some(Notified::Due("work".into()))
        );
    }

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
