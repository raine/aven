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

pub fn reopen_after_exit(_bundle: &Path) -> std::io::Result<()> {
    Err(std::io::Error::from(std::io::ErrorKind::Unsupported))
}

/// One app per database. The app that holds `<db>.desktop.lock` is the one;
/// it listens on `<db>.desktop.sock`, beside the database and so as private
/// as it, for later launches asking it to come forward.
pub mod instance {
    use std::fs::{File, TryLockError};
    use std::io::{self, Write};
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

    /// How long a later launch keeps trying to reach an app that holds the
    /// lock but may not listen yet.
    const REACH_FOR: Duration = Duration::from_secs(2);
    const REACH_EVERY: Duration = Duration::from_millis(100);

    /// This app's hold on its database, kept for as long as it runs.
    pub struct Claim {
        _lock: Option<File>,
        /// One message per later launch, which wants the window in front.
        pub raised: Option<UnboundedReceiver<()>>,
    }

    /// Claims the database at `db` for this app. Returns `None` once the app
    /// that has it is asked to come forward. Anything that stops the claim
    /// from being checked lets this app run, since refusing to open is
    /// worse than a second window.
    pub fn claim(db: &Path) -> Option<Claim> {
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
                raise(&socket);
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
    fn raise(socket: &Path) {
        let mut waited = Duration::ZERO;
        loop {
            match UnixStream::connect(socket).and_then(|mut stream| stream.write_all(b"raise")) {
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
    fn listen(socket: &Path) -> io::Result<UnboundedReceiver<()>> {
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
                    if stream.is_ok() && sender.send(()).is_err() {
                        break;
                    }
                }
            })?;
        Ok(receiver)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_second_claim_raises_the_first() {
            let dir = tempfile::tempdir().unwrap();
            let db = dir.path().join("aven.sqlite");
            let mut first = claim(&db).expect("the first app claims the database");
            assert!(claim(&db).is_none(), "a second app on it must not run");
            let raised = first.raised.as_mut().expect("the first app listens");
            assert_eq!(raised.blocking_recv(), Some(()));

            let other = dir.path().join("other.sqlite");
            assert!(claim(&other).is_some(), "another database is its own");

            drop(first);
            assert!(claim(&db).is_some(), "the claim ends with the app");
        }
    }
}
