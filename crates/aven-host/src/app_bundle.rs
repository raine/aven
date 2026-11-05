//! Locations of the macOS `Aven.app` bundle that wraps the `aven` executable.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub const AVEN_BUNDLE_ID: &str = "fi.zendit.Aven";
pub const BUNDLE_NAME: &str = "Aven.app";
/// The main executable, relative to the bundle directory.
pub const BUNDLE_EXECUTABLE: &str = "Contents/MacOS/aven";

/// The `.app` directory whose main executable is the canonical `aven` path.
pub fn enclosing_bundle(executable: &Path) -> Option<PathBuf> {
    if executable.file_name()? != "aven" {
        return None;
    }
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Where the install script and `aven update` keep the bundle of a direct
/// install; the bin directory holds a symlink to its executable.
pub fn direct_install_bundle(home: &Path) -> PathBuf {
    home.join(".local/share/aven").join(BUNDLE_NAME)
}

/// `open --env` arguments carrying this process's `AVEN_*`, `HOME` and XDG
/// settings. Launch Services starts an app with the session's environment, so
/// an app reopened through `open` needs them passed along to keep using the
/// same data.
pub fn open_environment_args() -> Vec<OsString> {
    let mut arguments = Vec::new();
    for (key, value) in std::env::vars_os() {
        if forwarded(&key.to_string_lossy()) {
            let mut setting = key;
            setting.push("=");
            setting.push(value);
            arguments.push(OsString::from("--env"));
            arguments.push(setting);
        }
    }
    arguments
}

fn forwarded(key: &str) -> bool {
    key.starts_with("AVEN_")
        || matches!(
            key,
            "HOME" | "XDG_CONFIG_HOME" | "XDG_STATE_HOME" | "XDG_DATA_HOME" | "XDG_CACHE_HOME"
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_settings_are_forwarded_but_not_the_rest() {
        assert!(forwarded("AVEN_DB"));
        assert!(forwarded("HOME"));
        assert!(forwarded("XDG_DATA_HOME"));
        assert!(!forwarded("PATH"));
        assert!(!forwarded("XPC_SERVICE_NAME"));
    }
}
