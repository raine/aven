use std::path::Path;
use std::process::Command as ProcessCommand;

use anyhow::{Context, Result};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OperatingSystem {
    Macos,
    Linux,
    Windows,
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct ViewerCommand {
    pub(super) program: &'static str,
    pub(super) args: Vec<std::ffi::OsString>,
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn open_image_in_default_viewer(path: &Path) -> Result<()> {
    let os = current_operating_system();
    spawn_viewer(
        default_image_viewer_command(os, path),
        "could not start the default image viewer",
    )
}

#[cfg(not(test))]
pub(crate) fn open_url_in_default_browser(url: &str) -> Result<()> {
    validate_browser_url(url)?;
    spawn_viewer(
        default_browser_command(current_operating_system(), url),
        "could not start the default browser",
    )
}

fn validate_browser_url(url: &str) -> Result<()> {
    anyhow::ensure!(
        url.starts_with("https://") || url.starts_with("http://"),
        "browser URL must use http or https"
    );
    Ok(())
}

fn current_operating_system() -> OperatingSystem {
    if cfg!(target_os = "macos") {
        OperatingSystem::Macos
    } else if cfg!(target_os = "windows") {
        OperatingSystem::Windows
    } else {
        OperatingSystem::Linux
    }
}

fn spawn_viewer(spec: ViewerCommand, context: &'static str) -> Result<()> {
    let mut child = ProcessCommand::new(spec.program)
        .args(spec.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context(context)?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

pub(super) fn default_image_viewer_command(os: OperatingSystem, path: &Path) -> ViewerCommand {
    let path = path.as_os_str().to_owned();
    match os {
        OperatingSystem::Macos => ViewerCommand {
            program: "open",
            args: vec![path],
        },
        OperatingSystem::Linux => ViewerCommand {
            program: "xdg-open",
            args: vec![path],
        },
        OperatingSystem::Windows => ViewerCommand {
            program: "rundll32.exe",
            args: vec!["url.dll,FileProtocolHandler".into(), path],
        },
    }
}

pub(super) fn default_browser_command(os: OperatingSystem, url: &str) -> ViewerCommand {
    let url = std::ffi::OsString::from(url);
    match os {
        OperatingSystem::Macos => ViewerCommand {
            program: "open",
            args: vec![url],
        },
        OperatingSystem::Linux => ViewerCommand {
            program: "xdg-open",
            args: vec![url],
        },
        OperatingSystem::Windows => ViewerCommand {
            program: "rundll32.exe",
            args: vec!["url.dll,FileProtocolHandler".into(), url],
        },
    }
}

#[cfg(test)]
thread_local! {
    static TEST_BROWSER_URL: std::cell::RefCell<Option<String>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(crate) fn open_url_in_default_browser(url: &str) -> Result<()> {
    validate_browser_url(url)?;
    TEST_BROWSER_URL.with(|opened| opened.replace(Some(url.to_string())));
    Ok(())
}

#[cfg(test)]
pub(crate) fn browser_url_for_test() -> Option<String> {
    TEST_BROWSER_URL.with(|opened| opened.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selects_platform_default_image_viewer_commands() {
        let path = Path::new("/tmp/attachment image.png");
        assert_eq!(
            default_image_viewer_command(OperatingSystem::Macos, path),
            ViewerCommand {
                program: "open",
                args: vec![path.as_os_str().to_owned()],
            }
        );
        assert_eq!(
            default_image_viewer_command(OperatingSystem::Linux, path),
            ViewerCommand {
                program: "xdg-open",
                args: vec![path.as_os_str().to_owned()],
            }
        );
        assert_eq!(
            default_image_viewer_command(OperatingSystem::Windows, path),
            ViewerCommand {
                program: "rundll32.exe",
                args: vec![
                    "url.dll,FileProtocolHandler".into(),
                    path.as_os_str().to_owned(),
                ],
            }
        );
    }

    #[test]
    fn selects_platform_default_browser_commands() {
        let url = "https://aventasks.dev/recurring-tasks/";
        assert_eq!(
            default_browser_command(OperatingSystem::Macos, url),
            ViewerCommand {
                program: "open",
                args: vec![url.into()],
            }
        );
        assert_eq!(
            default_browser_command(OperatingSystem::Linux, url),
            ViewerCommand {
                program: "xdg-open",
                args: vec![url.into()],
            }
        );
        assert_eq!(
            default_browser_command(OperatingSystem::Windows, url),
            ViewerCommand {
                program: "rundll32.exe",
                args: vec!["url.dll,FileProtocolHandler".into(), url.into()],
            }
        );
    }
}
