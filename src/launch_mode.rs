//! Whether this process is a command or an opened app.
//!
//! `Contents/MacOS/aven` is one executable for both. Only a launch by Launch
//! Services becomes the app, and everything else stays clear of AppKit: this is
//! decided before any other work in the process.
use std::path::Path;

use crate::app_bundle;
use crate::notification;

const LAUNCHD: libc::pid_t = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// Arguments or a terminal: the command line, as run anywhere else.
    Cli,
    /// The bundle's executable with nothing to do, such as from a launchd job,
    /// cron or `ssh -T` through the bin symlink. Exits quietly.
    Idle,
    /// Launch Services opened the bundle, from Finder, the Dock, `open` or a
    /// clicked notification.
    App,
}

/// What a launch looks like from inside the process.
#[derive(Debug, Clone, Copy)]
struct Launch<'a> {
    /// Launch Services may add a `-psn_` argument, which does not count.
    has_arguments: bool,
    has_terminal: bool,
    /// The identifier of the bundle around this executable, if there is one.
    bundle: Option<&'a str>,
    parent: libc::pid_t,
    /// launchd's name for this job.
    service: Option<&'a str>,
}

fn select(launch: Launch<'_>) -> LaunchMode {
    if launch.has_arguments || launch.has_terminal {
        return LaunchMode::Cli;
    }
    let Some(bundle) = launch.bundle else {
        return LaunchMode::Cli;
    };
    if launch.parent == LAUNCHD && launch.service.is_some_and(|name| names_app(name, bundle)) {
        LaunchMode::App
    } else {
        LaunchMode::Idle
    }
}

/// Launch Services runs an app as the launchd job
/// `application.<bundle identifier>.<numbers>`. Processes inherit that name from
/// any app that starts them, so it only counts for this bundle.
fn names_app(service: &str, bundle: &str) -> bool {
    service
        .strip_prefix("application.")
        .and_then(|name| name.strip_prefix(bundle))
        .is_some_and(|rest| rest.starts_with('.'))
}

/// Selects this process's mode. A command with arguments returns before it
/// looks at anything else.
pub fn current() -> LaunchMode {
    let has_arguments = std::env::args_os()
        .skip(1)
        .any(|argument| !argument.to_string_lossy().starts_with("-psn_"));
    if has_arguments {
        return LaunchMode::Cli;
    }
    let bundle = std::env::current_exe()
        .and_then(std::fs::canonicalize)
        .ok()
        .as_deref()
        .and_then(app_bundle::enclosing_bundle);
    let identifier = bundle.as_deref().and_then(identifier);
    let service = std::env::var("XPC_SERVICE_NAME").ok();
    select(Launch {
        has_arguments,
        has_terminal: has_terminal(),
        bundle: identifier.as_deref(),
        // SAFETY: `getppid` has no preconditions and cannot fail.
        parent: unsafe { libc::getppid() },
        service: service.as_deref(),
    })
}

fn has_terminal() -> bool {
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .is_ok()
}

/// A bundle with no readable identifier stays a bundle, one no job can name.
fn identifier(bundle: &Path) -> Option<String> {
    Some(notification::bundle_identifier(bundle).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const OPENED: Launch<'_> = Launch {
        has_arguments: false,
        has_terminal: false,
        bundle: Some("fi.zendit.Aven"),
        parent: LAUNCHD,
        service: Some("application.fi.zendit.Aven.1234567.1234999"),
    };

    #[test]
    fn launch_services_opens_the_app() {
        assert_eq!(select(OPENED), LaunchMode::App);
        let development = Launch {
            bundle: Some("fi.zendit.Aven.dev"),
            service: Some("application.fi.zendit.Aven.dev.1234567.1234999"),
            ..OPENED
        };
        assert_eq!(select(development), LaunchMode::App);
    }

    #[test]
    fn arguments_or_a_terminal_mean_the_command_line() {
        for launch in [
            Launch {
                has_arguments: true,
                ..OPENED
            },
            Launch {
                has_terminal: true,
                ..OPENED
            },
            Launch {
                bundle: None,
                ..OPENED
            },
        ] {
            assert_eq!(select(launch), LaunchMode::Cli, "{launch:?}");
        }
    }

    /// The callers that reach the bundle's executable through the bin symlink
    /// with no arguments and no terminal.
    #[test]
    fn bare_callers_never_open_the_app() {
        for (caller, parent, service) in [
            ("cron", 812, None),
            ("launchd job", LAUNCHD, Some("com.raine.aven.daemon")),
            ("launchd job without a name", LAUNCHD, None),
            ("ssh -T", 4410, None),
            ("a script run by the app", 4410, OPENED.service),
            (
                "orphan of another app",
                LAUNCHD,
                Some("application.com.googlecode.iterm2.77.88"),
            ),
            (
                "job named after a prefix of the identifier",
                LAUNCHD,
                Some("application.fi.zendit.Avenue.77.88"),
            ),
        ] {
            let launch = Launch {
                parent,
                service,
                ..OPENED
            };
            assert_eq!(select(launch), LaunchMode::Idle, "{caller}");
        }
    }

    #[test]
    fn an_unreadable_bundle_is_never_named_by_launch_services() {
        let launch = Launch {
            bundle: Some(""),
            ..OPENED
        };
        assert_eq!(select(launch), LaunchMode::Idle);
    }
}
