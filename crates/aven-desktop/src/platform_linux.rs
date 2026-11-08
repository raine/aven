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
