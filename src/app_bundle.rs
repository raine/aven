//! Locations of the macOS `Aven.app` bundle that wraps the `aven` executable.

use std::path::{Path, PathBuf};

pub(crate) const AVEN_BUNDLE_ID: &str = "fi.zendit.Aven";
pub(crate) const BUNDLE_NAME: &str = "Aven.app";
/// The main executable, relative to the bundle directory.
pub(crate) const BUNDLE_EXECUTABLE: &str = "Contents/MacOS/aven";

/// The `.app` directory whose main executable is the canonical `aven` path.
pub(crate) fn enclosing_bundle(executable: &Path) -> Option<PathBuf> {
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
pub(crate) fn direct_install_bundle(home: &Path) -> PathBuf {
    home.join(".local/share/aven").join(BUNDLE_NAME)
}
