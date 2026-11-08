//! What GPUI does not do for itself, and stand-ins for the macOS services the
//! app manages there. On Linux the app runs in place, with no bundle or install
//! layout, and leaves the daemon to `aven daemon install`.
use std::path::{Path, PathBuf};

use anyhow::Result;
use aven_host::notify::{Authorization, Deliver, Report, Request};

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
/// service. It has no permission to read, and clicks open nothing.
pub struct Notifier;

impl Deliver for Notifier {
    fn deliver(&self, request: &Request) -> Result<Report> {
        let errors = request
            .posts
            .iter()
            .map(|post| {
                notify_rust::Notification::new()
                    .appname("Aven")
                    .summary(&post.title)
                    .body(&post.body)
                    .show()
                    .err()
                    .map(|error| error.to_string())
            })
            .collect();
        Ok(Report {
            authorization: Authorization::Allowed,
            errors,
            delivered: None,
        })
    }
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
        pub raised: Option<UnboundedReceiver<Activation>>,
    }

    /// Claims the database at `db` for this app. Returns `None` once the app
    /// that has it is asked to come forward with `activation`. Anything that
    /// stops the claim from being checked lets this app run, since refusing
    /// to open is worse than a second window.
    pub fn claim(db: &Path, activation: &Activation) -> Option<Claim> {
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
                raise(&socket, activation);
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

    /// Asks the app that has the database to come forward, handing it this
    /// launch's activation, which only this launch was given. Its socket may
    /// not be bound yet when both start together, so this retries a moment.
    fn raise(socket: &Path, activation: &Activation) {
        let message = serde_json::to_vec(activation).unwrap_or_default();
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
    fn listen(socket: &Path) -> io::Result<UnboundedReceiver<Activation>> {
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
                    if sender.send(read_activation(stream)).is_err() {
                        break;
                    }
                }
            })?;
        Ok(receiver)
    }

    /// What a later launch handed over. One that says nothing readable
    /// still asks for the window, without a token.
    fn read_activation(mut stream: UnixStream) -> Activation {
        let _ = stream.set_read_timeout(Some(READ_FOR));
        let mut message = Vec::new();
        let _ = (&mut stream).take(MESSAGE_LIMIT).read_to_end(&mut message);
        serde_json::from_slice(&message).unwrap_or_default()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_second_claim_raises_the_first_with_its_activation() {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("aven.sqlite");
            let none = Activation::default();
            let mut first = claim(&db, &none).expect("the first app claims the database");
            let second = Activation {
                token: Some("token-1".into()),
                startup_id: Some("launcher-2_TIME42".into()),
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
            assert_eq!(read_activation(receiver), Activation::default());
        }
    }
}
