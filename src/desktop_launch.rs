//! Whether this process is a command or the Linux desktop app.
//!
//! One executable is both. A desktop launcher starts it with no arguments and
//! no terminal inside a graphical session; everything else is the command line.

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

/// Whether this process opens the app. A command with arguments returns before
/// it looks at anything else.
pub fn opens() -> bool {
    let has_arguments = std::env::args_os().len() > 1;
    if has_arguments {
        return false;
    }
    let names_display = |name| std::env::var_os(name).is_some_and(|value| !value.is_empty());
    opens_app(Launch {
        has_arguments,
        has_terminal: has_terminal(),
        has_display: names_display("WAYLAND_DISPLAY") || names_display("DISPLAY"),
    })
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
}
