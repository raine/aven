//! What GPUI does not do for itself, and stand-ins for the macOS services the
//! app manages there. On Linux the app runs in place, with no bundle or install
//! layout, and leaves the daemon to `aven daemon install`.
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use aven_host::notify::freedesktop::{Click, Freedesktop};
use aven_host::notify::{Deliver, Report, Request};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Linux runs no app bundle, so nothing watches for one being replaced.
pub fn enclosing_bundle(_executable: &Path) -> Option<PathBuf> {
    None
}

/// The background intent and the agent that follows it. This app manages no
/// agent on Linux, so only the intent is real.
pub mod background {
    use anyhow::Result;
    use aven_host::config::AppConfig;
    use std::path::{Path, PathBuf};

    pub use aven_host::background::{set_intent, wants_agent};

    #[allow(dead_code, reason = "nothing reconciles an agent on Linux")]
    pub enum Reconciled {
        Unchanged,
        Started,
    }

    pub fn reconcile(_config: &AppConfig, _db_path: PathBuf, _app: &Path) -> Result<Reconciled> {
        Ok(Reconciled::Unchanged)
    }
}

/// The agent as the app shows it. Nothing reads it on Linux.
pub mod agent_service {
    use anyhow::{Result, bail};
    use std::path::PathBuf;

    #[allow(dead_code, reason = "nothing reads an agent on Linux")]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum LoginItem {
        Allowed,
        TurnedOff,
        Unknown,
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct AgentStatus {
        pub installed: bool,
        pub program: Option<PathBuf>,
        pub loaded: bool,
        pub running: bool,
        pub login_item: LoginItem,
    }

    pub fn agent_status() -> Result<AgentStatus> {
        bail!("Aven Desktop does not manage the daemon on Linux")
    }
}

/// The app has no installation layout on Linux and leaves the daemon to
/// `aven daemon install`.
pub fn agent_app() -> Option<PathBuf> {
    None
}

/// Posts the app's notifications through the freedesktop notification
/// service, and hands clicks on them to [`notification_clicks`].
pub struct Notifier;

impl Deliver for Notifier {
    fn deliver(&self, request: &Request) -> Result<Report> {
        static SERVICE: OnceLock<Freedesktop> = OnceLock::new();
        SERVICE
            .get_or_init(|| {
                Freedesktop::new("Aven", |click| {
                    let _ = clicks().0.send(click);
                })
            })
            .deliver(request)
    }
}

type Clicks = (
    UnboundedSender<Click>,
    Mutex<Option<UnboundedReceiver<Click>>>,
);

fn clicks() -> &'static Clicks {
    static CLICKS: OnceLock<Clicks> = OnceLock::new();
    CLICKS.get_or_init(|| {
        let (sender, receiver) = unbounded_channel();
        (sender, Mutex::new(Some(receiver)))
    })
}

/// Clicks on the app's notifications, for the one caller that follows them.
pub fn notification_clicks() -> Option<UnboundedReceiver<Click>> {
    clicks().1.lock().ok()?.take()
}

/// Linux desktops keep the setting in their own places, which this does not
/// read.
pub fn reduce_motion() -> bool {
    false
}

/// Only an app bundle's relaunch asks, which Linux has none of.
pub fn is_active() -> bool {
    false
}

/// Linux gives no keyboard state for an app that is not in front.
pub fn option_alone() -> bool {
    false
}

/// Minimizes the window, since Linux has no Dock icon to bring a hidden one
/// back from.
pub fn order_out(window: &gpui::Window) {
    window.minimize_window();
}

/// The panel keeps the window manager's frame.
pub fn plain_panel(_window: &gpui::Window) {}

/// The family the app's text is set in: the desktop's sans-serif font, as
/// fontconfig resolves it. Without fontconfig, GPUI's stand-ins for the
/// system font.
pub fn ui_font() -> gpui::SharedString {
    static FAMILY: OnceLock<gpui::SharedString> = OnceLock::new();
    FAMILY
        .get_or_init(|| desktop_sans().map_or_else(|| ".SystemUIFont".into(), Into::into))
        .clone()
}

fn desktop_sans() -> Option<String> {
    let output = std::process::Command::new("fc-match")
        .args(["--format=%{family[0]}", "sans-serif"])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    let family = String::from_utf8(output.stdout).ok()?;
    let family = family.trim();
    (output.status.success() && !family.is_empty()).then(|| family.to_string())
}

pub use frame::{frame, title_bar_double_click, top_bar_insets};

/// The decorations the app draws when the compositor leaves them to it, as
/// GNOME's and Weston's do: window buttons in the top bar, resize edges and
/// a border.
mod frame {
    use accesskit::Role;
    use gpui::{
        AnyElement, App, CursorStyle, Decorations, Div, MouseButton, Pixels, ResizeEdge, Stateful,
        Tiling, Window, WindowButton, WindowButtonLayout, div, prelude::*, px, svg,
    };

    use crate::actions::CloseWindow;
    use crate::theme::{Theme, theme};

    /// The space between the top bar's start and its first control without
    /// window buttons there.
    const MARGIN: Pixels = px(12.);
    /// The top bar's height, which the buttons centre in.
    const BAR: Pixels = px(44.);
    const BUTTON: Pixels = px(24.);
    const GAP: Pixels = px(8.);
    /// How far in from the window's edge a press resizes it.
    const EDGE: Pixels = px(5.);
    /// How far along each edge from a corner a press resizes both ways.
    const CORNER: Pixels = px(14.);

    /// The room the window's own controls take at each end of the top bar.
    pub struct TopBarInsets {
        pub start: Pixels,
        pub end: Pixels,
    }

    pub fn top_bar_insets(window: &Window, cx: &App) -> TopBarInsets {
        let Some(layout) = buttons(window, cx) else {
            return TopBarInsets {
                start: MARGIN,
                end: px(0.),
            };
        };
        TopBarInsets {
            start: MARGIN + width(&layout.left),
            end: width(&layout.right),
        }
    }

    /// The window menu's default: a double click on the top bar maximizes
    /// the window, or restores it, when it is the window's title bar.
    pub fn title_bar_double_click(window: &Window) {
        if matches!(window.window_decorations(), Decorations::Client { .. }) {
            window.zoom_window();
        }
    }

    pub fn frame(content: AnyElement, window: &mut Window, cx: &mut App) -> AnyElement {
        let Some(layout) = buttons(window, cx) else {
            return content;
        };
        let Decorations::Client { mut tiling } = window.window_decorations() else {
            return content;
        };
        if window.is_maximized() || window.is_fullscreen() {
            tiling = Tiling::tiled();
        }
        let t = theme(cx);
        let maximized = window.is_maximized();
        div()
            .relative()
            .size_full()
            .child(content)
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .border_color(t.line2)
                    .when(!tiling.top, |border| border.border_t_1())
                    .when(!tiling.bottom, |border| border.border_b_1())
                    .when(!tiling.left, |border| border.border_l_1())
                    .when(!tiling.right, |border| border.border_r_1()),
            )
            .children(edges(tiling))
            .child(side(&layout.left, maximized, &t).left_0().pl(px(10.)))
            .child(side(&layout.right, maximized, &t).right_0().pr(px(10.)))
            .into_any_element()
    }

    /// The buttons at each end of the top bar, when the app draws them.
    fn buttons(window: &Window, cx: &App) -> Option<WindowButtonLayout> {
        if !matches!(window.window_decorations(), Decorations::Client { .. }) {
            return None;
        }
        let controls = window.window_controls();
        let mut layout = cx
            .button_layout()
            .unwrap_or_else(WindowButtonLayout::linux_default);
        for slot in layout.left.iter_mut().chain(layout.right.iter_mut()) {
            let offered = match slot {
                Some(WindowButton::Minimize) => controls.minimize,
                Some(WindowButton::Maximize) => controls.maximize,
                Some(WindowButton::Close) | None => true,
            };
            if !offered {
                *slot = None;
            }
        }
        Some(layout)
    }

    fn width(side: &[Option<WindowButton>]) -> Pixels {
        let count = side.iter().flatten().count();
        if count == 0 {
            return px(0.);
        }
        px(10.) + BUTTON * count as f32 + GAP * (count - 1) as f32
    }

    fn side(buttons: &[Option<WindowButton>], maximized: bool, t: &Theme) -> Div {
        div()
            .absolute()
            .top_0()
            .h(BAR)
            .flex()
            .items_center()
            .gap(GAP)
            .children(
                buttons
                    .iter()
                    .flatten()
                    .map(|button| window_button(*button, maximized, t)),
            )
    }

    fn window_button(button: WindowButton, maximized: bool, t: &Theme) -> Stateful<Div> {
        let (icon, label) = match button {
            WindowButton::Minimize => ("icons/window-minimize.svg", "Minimize"),
            WindowButton::Maximize if maximized => ("icons/window-restore.svg", "Restore"),
            WindowButton::Maximize => ("icons/window-maximize.svg", "Maximize"),
            WindowButton::Close => ("icons/window-close.svg", "Close"),
        };
        div()
            .id(button.id())
            .role(Role::Button)
            .aria_label(label)
            .size(BUTTON)
            .rounded_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(t.raised)
            .hover(|style| style.bg(t.line))
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| match button {
                WindowButton::Minimize => window.minimize_window(),
                WindowButton::Maximize => window.zoom_window(),
                WindowButton::Close => window.dispatch_action(Box::new(CloseWindow), cx),
            })
            .child(svg().path(icon).size(px(14.)).text_color(t.body))
    }

    /// The bands along the window's free edges that resize it.
    fn edges(tiling: Tiling) -> Vec<Div> {
        let top = !tiling.top;
        let bottom = !tiling.bottom;
        let left = !tiling.left;
        let right = !tiling.right;
        let mut edges = Vec::new();
        let mut edge = |free: bool, resize: ResizeEdge, place: fn(Div) -> Div| {
            if free {
                edges.push(place(handle(resize)));
            }
        };
        edge(top, ResizeEdge::Top, |band| {
            band.top_0().left_0().w_full().h(EDGE)
        });
        edge(bottom, ResizeEdge::Bottom, |band| {
            band.bottom_0().left_0().w_full().h(EDGE)
        });
        edge(left, ResizeEdge::Left, |band| {
            band.top_0().left_0().h_full().w(EDGE)
        });
        edge(right, ResizeEdge::Right, |band| {
            band.top_0().right_0().h_full().w(EDGE)
        });
        edge(top && left, ResizeEdge::TopLeft, |corner| {
            corner.top_0().left_0().size(CORNER)
        });
        edge(top && right, ResizeEdge::TopRight, |corner| {
            corner.top_0().right_0().size(CORNER)
        });
        edge(bottom && left, ResizeEdge::BottomLeft, |corner| {
            corner.bottom_0().left_0().size(CORNER)
        });
        edge(bottom && right, ResizeEdge::BottomRight, |corner| {
            corner.bottom_0().right_0().size(CORNER)
        });
        edges
    }

    fn handle(edge: ResizeEdge) -> Div {
        let cursor = match edge {
            ResizeEdge::Top | ResizeEdge::Bottom => CursorStyle::ResizeUpDown,
            ResizeEdge::Left | ResizeEdge::Right => CursorStyle::ResizeLeftRight,
            ResizeEdge::TopLeft | ResizeEdge::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            ResizeEdge::TopRight | ResizeEdge::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
        };
        div().absolute().cursor(cursor).occlude().on_mouse_down(
            MouseButton::Left,
            move |_, window, cx| {
                cx.stop_propagation();
                window.start_window_resize(edge);
            },
        )
    }
}

/// What lets a window take focus from the window system: the token of the
/// launch or click that asked for it. A compositor that prevents focus
/// stealing only marks a window as wanting attention without one.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Activation {
    /// An xdg-activation token, as `XDG_ACTIVATION_TOKEN` gives one on
    /// Wayland.
    pub token: Option<String>,
    /// A startup notification id, as `DESKTOP_STARTUP_ID` gives one on X11.
    pub startup_id: Option<String>,
}

impl Activation {
    /// This process's own, from what its launcher set.
    pub fn from_environment() -> Self {
        let read = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
        Self {
            token: read("XDG_ACTIVATION_TOKEN"),
            startup_id: read("DESKTOP_STARTUP_ID"),
        }
    }
}

/// Brings `window` to the front with `activation`, or asks GPUI to, which on
/// Wayland only marks the window as wanting attention.
pub fn activate(window: &gpui::Window, activation: &Activation) {
    use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};

    let handles = HasWindowHandle::window_handle(window)
        .and_then(|handle| Ok((handle.as_raw(), window.display_handle()?.as_raw())));
    let activated = match (handles, &activation.token, &activation.startup_id) {
        (
            Ok((RawWindowHandle::Wayland(surface), RawDisplayHandle::Wayland(display))),
            Some(token),
            _,
        ) => Some(wayland::activate(display.display, surface.surface, token)),
        (Ok((RawWindowHandle::Xcb(window), RawDisplayHandle::Xcb(display))), _, Some(id)) => {
            display
                .connection
                .map(|connection| x11::activate(connection, window.window.get(), id))
        }
        _ => None,
    };
    match activated {
        Some(Ok(())) => {}
        Some(Err(error)) => {
            tracing::warn!(error = %format_args!("{error:#}"), "could not bring the window forward with its token");
            window.activate_window();
        }
        None => window.activate_window(),
    }
}

/// xdg-activation on GPUI's own Wayland connection, which GPUI only uses
/// with tokens it asked for itself.
mod wayland {
    use std::cell::RefCell;
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use anyhow::Result;
    use wayland_backend::client::{Backend, ObjectId};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::WlRegistry;
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop};
    use wayland_protocols::xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1;

    struct State;

    impl Dispatch<WlRegistry, GlobalListContents> for State {
        fn event(
            _: &mut Self,
            _: &WlRegistry,
            _: <WlRegistry as Proxy>::Event,
            _: &GlobalListContents,
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    delegate_noop!(State: ignore XdgActivationV1);

    /// The compositor's activation global, bound on its own queue of the
    /// display it belongs to.
    struct Bound {
        display: *mut c_void,
        connection: Connection,
        _queue: EventQueue<State>,
        activation: XdgActivationV1,
    }

    thread_local! {
        static BOUND: RefCell<Option<Bound>> = const { RefCell::new(None) };
    }

    /// Activates `surface` with `token`. GPUI's window handles name its live
    /// `wl_display` and `wl_surface`, which outlive the window.
    pub fn activate(display: NonNull<c_void>, surface: NonNull<c_void>, token: &str) -> Result<()> {
        BOUND.with_borrow_mut(|bound| {
            if bound
                .as_ref()
                .is_none_or(|bound| bound.display != display.as_ptr())
            {
                *bound = Some(bind(display)?);
            }
            let bound = bound.as_ref().expect("bound above");
            // SAFETY: `surface` is GPUI's live `wl_surface` on this display.
            let id =
                unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.as_ptr().cast()) }?;
            let surface = WlSurface::from_id(&bound.connection, id)?;
            bound.activation.activate(token.to_string(), &surface);
            bound.connection.flush()?;
            Ok(())
        })
    }

    fn bind(display: NonNull<c_void>) -> Result<Bound> {
        // SAFETY: `display` is GPUI's live `wl_display`, which stays open for
        // as long as the app runs.
        let backend = unsafe { Backend::from_foreign_display(display.as_ptr().cast()) };
        let connection = Connection::from_backend(backend);
        let (globals, queue) = registry_queue_init::<State>(&connection)?;
        let activation = globals.bind(&queue.handle(), 1..=1, ())?;
        Ok(Bound {
            display: display.as_ptr(),
            connection,
            _queue: queue,
            activation,
        })
    }
}

/// Startup notification and `_NET_ACTIVE_WINDOW` on GPUI's own X11
/// connection, with the time the launch began, which GPUI leaves unset.
mod x11 {
    use std::ffi::c_void;
    use std::ptr::NonNull;

    use anyhow::Result;
    use x11rb::connection::Connection as _;
    use x11rb::protocol::xproto::{
        Atom, ClientMessageEvent, ConnectionExt as _, EventMask, Window,
    };
    use x11rb::xcb_ffi::XCBConnection;

    /// `_NET_ACTIVE_WINDOW`'s source for a request from an application.
    const FROM_APPLICATION: u32 = 1;
    /// How much of a startup notification message one client message holds.
    const CHUNK: usize = 20;

    /// Activates `window` for the launch `startup_id`, and tells the
    /// launcher that launch is done.
    pub fn activate(connection: NonNull<c_void>, window: Window, startup_id: &str) -> Result<()> {
        // SAFETY: `connection` is GPUI's live XCB connection, which this
        // borrows without closing.
        let conn = unsafe { XCBConnection::from_raw_xcb_connection(connection.as_ptr(), false) }?;
        let root = conn.get_geometry(window)?.reply()?.root;
        let atom = |name: &str| -> Result<Atom> {
            Ok(conn.intern_atom(false, name.as_bytes())?.reply()?.atom)
        };
        let send = |message: ClientMessageEvent| -> Result<()> {
            conn.send_event(
                false,
                root,
                EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
                message,
            )?;
            Ok(())
        };
        let (begin, more) = (atom("_NET_STARTUP_INFO_BEGIN")?, atom("_NET_STARTUP_INFO")?);
        for (index, chunk) in remove_message(startup_id).chunks(CHUNK).enumerate() {
            let mut data = [0_u8; CHUNK];
            data[..chunk.len()].copy_from_slice(chunk);
            let kind = if index == 0 { begin } else { more };
            send(ClientMessageEvent::new(8, window, kind, data))?;
        }
        let time = launch_time(startup_id).unwrap_or(x11rb::CURRENT_TIME);
        let active = atom("_NET_ACTIVE_WINDOW")?;
        send(ClientMessageEvent::new(
            32,
            window,
            active,
            [FROM_APPLICATION, time, 0, 0, 0],
        ))?;
        conn.flush()?;
        Ok(())
    }

    /// The X server time a launch began, which launchers put at the end of
    /// the id, as in `kwin-1234-5678_TIME87654321`.
    pub(super) fn launch_time(startup_id: &str) -> Option<u32> {
        startup_id.rsplit_once("_TIME")?.1.parse().ok()
    }

    /// The startup notification message that ends the launch `startup_id`,
    /// with its terminating NUL.
    pub(super) fn remove_message(startup_id: &str) -> Vec<u8> {
        let mut quoted = String::with_capacity(startup_id.len() + 2);
        for character in startup_id.chars() {
            if matches!(character, '"' | '\\') {
                quoted.push('\\');
            }
            quoted.push(character);
        }
        format!("remove: ID=\"{quoted}\"\0").into_bytes()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_launch_time_comes_from_the_end_of_its_id() {
            assert_eq!(launch_time("kwin-1234-5678_TIME87654321"), Some(87654321));
            assert_eq!(launch_time("no-time"), None);
            assert_eq!(launch_time("bad_TIMEsoon"), None);
        }

        #[test]
        fn a_remove_message_quotes_the_id_and_ends_in_nul() {
            assert_eq!(
                remove_message(r#"a "b"\c"#),
                b"remove: ID=\"a \\\"b\\\"\\\\c\"\0".to_vec()
            );
        }
    }
}

pub fn reopen_after_exit(_bundle: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// One app per database. The app that holds `<db>.desktop.lock` is the one;
/// it listens on `<db>.desktop.sock`, beside the database and so as private
/// as it, for later launches asking it to come forward.
pub mod instance {
    use std::fs::{File, TryLockError};
    use std::io::{self, Read, Write};
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    use super::Activation;

    /// What a later launch asks of the app that has the database.
    #[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
    pub struct Forward {
        /// What lets the window come to the front, which only the later
        /// launch was given.
        pub activation: Activation,
        /// The tag of a clicked notification to open.
        pub notification: Option<String>,
    }

    /// How long a later launch keeps trying to reach an app that holds the
    /// lock but may not listen yet.
    const REACH_FOR: Duration = Duration::from_secs(2);
    const REACH_EVERY: Duration = Duration::from_millis(100);
    /// How long the app waits for a later launch to say what it wants.
    const READ_FOR: Duration = Duration::from_secs(1);
    /// The most a later launch may say.
    const MESSAGE_LIMIT: u64 = 16 * 1024;

    /// This app's hold on its database, kept for as long as it runs.
    pub struct Claim {
        _lock: Option<File>,
        /// One message per later launch, which wants the window in front.
        pub raised: Option<UnboundedReceiver<Forward>>,
    }

    /// Claims the database at `db` for this app. Returns `None` once the app
    /// that has it is handed `forward`. Anything that stops the claim from
    /// being checked lets this app run, since refusing to open is worse than
    /// a second window.
    pub fn claim(db: &Path, forward: &Forward) -> Option<Claim> {
        let lock = match open_lock(&beside(db, ".desktop.lock")) {
            Ok(lock) => lock,
            Err(error) => {
                tracing::warn!(%error, "could not check for another Aven on this database");
                return Some(Claim {
                    _lock: None,
                    raised: None,
                });
            }
        };
        let socket = beside(db, ".desktop.sock");
        match lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                raise(&socket, forward);
                return None;
            }
            Err(TryLockError::Error(error)) => {
                tracing::warn!(%error, "could not check for another Aven on this database");
                return Some(Claim {
                    _lock: None,
                    raised: None,
                });
            }
        }
        let raised = listen(&socket)
            .inspect_err(|error| {
                tracing::warn!(%error, "later launches cannot bring this window forward");
            })
            .ok();
        Some(Claim {
            _lock: Some(lock),
            raised,
        })
    }

    fn beside(db: &Path, suffix: &str) -> PathBuf {
        let mut path = db.as_os_str().to_owned();
        path.push(suffix);
        path.into()
    }

    fn open_lock(path: &Path) -> io::Result<File> {
        File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .mode(0o600)
            .open(path)
    }

    /// Asks the app that has the database to come forward. Its socket may
    /// not be bound yet when both start together, so this retries a moment.
    fn raise(socket: &Path, forward: &Forward) {
        let message = serde_json::to_vec(forward).unwrap_or_default();
        let mut waited = Duration::ZERO;
        loop {
            match UnixStream::connect(socket).and_then(|mut stream| stream.write_all(&message)) {
                Ok(()) => {
                    tracing::info!("Aven is already open on this database; brought it forward");
                    return;
                }
                Err(error) if waited >= REACH_FOR => {
                    tracing::warn!(%error, "Aven is already open on this database but did not answer");
                    return;
                }
                Err(_) => {
                    std::thread::sleep(REACH_EVERY);
                    waited += REACH_EVERY;
                }
            }
        }
    }

    /// Listens at `socket`, which the lock makes this app's to replace.
    fn listen(socket: &Path) -> io::Result<UnboundedReceiver<Forward>> {
        match std::fs::symlink_metadata(socket) {
            Ok(meta) if meta.file_type().is_socket() => std::fs::remove_file(socket)?,
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "a file that is not a socket has the app's socket name",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(socket)?;
        std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
        let (sender, receiver) = unbounded_channel();
        std::thread::Builder::new()
            .name("aven-raise".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else {
                        continue;
                    };
                    if sender.send(read_forward(stream)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(receiver)
    }

    /// What a later launch handed over. One that says nothing readable
    /// still asks for the window, without a token.
    fn read_forward(mut stream: UnixStream) -> Forward {
        let _ = stream.set_read_timeout(Some(READ_FOR));
        let mut message = Vec::new();
        let _ = (&mut stream).take(MESSAGE_LIMIT).read_to_end(&mut message);
        serde_json::from_slice(&message).unwrap_or_default()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_second_claim_hands_the_first_its_activation_and_notification() {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("aven.sqlite");
            let none = Forward::default();
            let mut first = claim(&db, &none).expect("the first app claims the database");
            let second = Forward {
                activation: Activation {
                    token: Some("token-1".into()),
                    startup_id: Some("launcher-2_TIME42".into()),
                },
                notification: Some("v1:task:home:T:e1".into()),
            };
            assert!(
                claim(&db, &second).is_none(),
                "a second app on it must not run"
            );
            let raised = first.raised.as_mut().expect("the first app listens");
            assert_eq!(raised.blocking_recv(), Some(second));

            let other = dir.path().join("other.sqlite");
            assert!(
                claim(&other, &none).is_some(),
                "another database is its own"
            );

            drop(first);
            assert!(claim(&db, &none).is_some(), "the claim ends with the app");
        }

        #[test]
        fn an_unreadable_message_still_raises() {
            let (mut sender, receiver) = UnixStream::pair().unwrap();
            sender.write_all(b"raise").unwrap();
            drop(sender);
            assert_eq!(read_forward(receiver), Forward::default());
        }
    }
}
