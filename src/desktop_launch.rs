//! Whether this process is a command or the Linux desktop app.
//!
//! One executable is both. A desktop launcher starts it with no arguments and
//! no terminal inside a graphical session; everything else is the command line,
//! except a click on a notification, which opens the app at what it is about.
use std::ffi::OsString;
use std::path::PathBuf;

/// The hidden command a clicked notification runs:
/// `aven internal open-notification <DB> <TAG>`.
pub const OPEN_NOTIFICATION: [&str; 2] = ["internal", "open-notification"];

/// What this process opens.
#[derive(Debug, PartialEq, Eq)]
pub enum Opening {
    App,
    /// The app on `database`, at what the notification `tag` is about.
    Notification {
        database: PathBuf,
        tag: String,
    },
}

/// What a launch looks like from inside the process.
#[derive(Debug, Clone, Copy)]
struct Launch {
    has_arguments: bool,
    has_terminal: bool,
    /// `WAYLAND_DISPLAY` or `DISPLAY` names a display.
    has_display: bool,
}

fn opens_app(launch: Launch) -> bool {
    !launch.has_arguments && !launch.has_terminal && launch.has_display
}

/// What this process opens, if not the command line. A command with other
/// arguments returns before it looks at anything else.
pub fn opens() -> Option<Opening> {
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    if !arguments.is_empty() {
        return notification(arguments).filter(|_| has_display());
    }
    opens_app(Launch {
        has_arguments: false,
        has_terminal: has_terminal(),
        has_display: has_display(),
    })
    .then_some(Opening::App)
}

/// The notification a clicked notification's command opens.
fn notification(arguments: Vec<OsString>) -> Option<Opening> {
    let [internal, open, database, tag] = <[OsString; 4]>::try_from(arguments).ok()?;
    (internal == OPEN_NOTIFICATION[0] && open == OPEN_NOTIFICATION[1]).then_some(())?;
    Some(Opening::Notification {
        database: database.into(),
        tag: tag.into_string().ok()?,
    })
}

/// Whether `WAYLAND_DISPLAY` or `DISPLAY` names a display.
pub fn has_display() -> bool {
    let names_display = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    names_display("WAYLAND_DISPLAY") || names_display("DISPLAY")
}

fn has_terminal() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAUNCHER: Launch = Launch {
        has_arguments: false,
        has_terminal: false,
        has_display: true,
    };

    #[test]
    fn a_desktop_launcher_opens_the_app() {
        assert!(opens_app(LAUNCHER));
    }

    #[test]
    fn arguments_a_terminal_or_no_display_mean_the_command_line() {
        for launch in [
            Launch {
                has_arguments: true,
                ..LAUNCHER
            },
            Launch {
                has_terminal: true,
                ..LAUNCHER
            },
            Launch {
                has_display: false,
                ..LAUNCHER
            },
        ] {
            assert!(!opens_app(launch), "{launch:?}");
        }
    }

    #[test]
    fn a_clicked_notification_names_its_database_and_tag() {
        let arguments = |list: &[&str]| list.iter().map(OsString::from).collect::<Vec<_>>();
        assert_eq!(
            notification(arguments(&[
                "internal",
                "open-notification",
                "/tmp/aven.sqlite",
                "v1:task:home:T:e1",
            ])),
            Some(Opening::Notification {
                database: "/tmp/aven.sqlite".into(),
                tag: "v1:task:home:T:e1".into(),
            })
        );
        for other in [
            &["internal", "open-notification", "/tmp/aven.sqlite"][..],
            &["internal", "notify", "/tmp/aven.sqlite", "tag"],
            &["list", "open-notification", "/tmp/aven.sqlite", "tag"],
        ] {
            assert_eq!(notification(arguments(other)), None, "{other:?}");
        }
    }
}
