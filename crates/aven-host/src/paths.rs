//! Comparing and naming filesystem paths.

use std::path::{Path, PathBuf};

/// Whether both paths exist and resolve to the same file.
pub(crate) fn same_file(left: &Path, right: &Path) -> bool {
    matches!((left.canonicalize(), right.canonicalize()), (Ok(left), Ok(right)) if left == right)
}

/// Whether the paths are equal or resolve to the same file, so a path that
/// no longer exists still matches itself.
#[cfg(target_os = "macos")]
pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    left == right || same_file(left, right)
}

/// The file beside the database at `db` whose name adds `suffix` to its own.
pub(crate) fn beside_db(db: &Path, suffix: &str) -> PathBuf {
    let mut name = db.file_name().unwrap_or(db.as_os_str()).to_os_string();
    name.push(suffix);
    db.with_file_name(name)
}
