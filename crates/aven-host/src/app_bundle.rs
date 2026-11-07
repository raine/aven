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

/// Where the bundle at `bundle` really is when macOS runs it through App
/// Translocation, a read-only mirror under `/private/var/folders` for a
/// quarantined app that Finder didn't move. `None` when it runs in place or
/// macOS can't say.
pub fn translocated_original(bundle: &Path) -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    return translocation::original(bundle);
    #[cfg(not(target_os = "macos"))]
    {
        let _ = bundle;
        None
    }
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

/// Security's App Translocation functions. They are exported but not in the
/// public SDK, so they are looked up at run time.
#[cfg(target_os = "macos")]
mod translocation {
    use std::ffi::{CStr, c_void};
    use std::path::{Path, PathBuf};

    use core_foundation::base::{Boolean, CFRelease, TCFType as _};
    use core_foundation::error::CFErrorRef;
    use core_foundation::url::{CFURL, CFURLRef};

    type IsTranslocated = unsafe extern "C" fn(CFURLRef, *mut bool, *mut CFErrorRef) -> Boolean;
    type CreateOriginalPath = unsafe extern "C" fn(CFURLRef, *mut CFErrorRef) -> CFURLRef;

    const SECURITY: &CStr = c"/System/Library/Frameworks/Security.framework/Security";

    pub(super) fn functions() -> Option<(IsTranslocated, CreateOriginalPath)> {
        // SAFETY: Security is a system framework, which stays loaded once
        // opened. Each symbol is cast to the signature SecTranslocate.h
        // declares for it.
        unsafe {
            let security = libc::dlopen(SECURITY.as_ptr(), libc::RTLD_LAZY);
            if security.is_null() {
                return None;
            }
            let is = libc::dlsym(security, c"SecTranslocateIsTranslocatedURL".as_ptr());
            let original =
                libc::dlsym(security, c"SecTranslocateCreateOriginalPathForURL".as_ptr());
            if is.is_null() || original.is_null() {
                return None;
            }
            Some((
                std::mem::transmute::<*mut c_void, IsTranslocated>(is),
                std::mem::transmute::<*mut c_void, CreateOriginalPath>(original),
            ))
        }
    }

    pub(super) fn original(bundle: &Path) -> Option<PathBuf> {
        let (is_translocated, create_original) = functions()?;
        let url = CFURL::from_path(bundle, true)?;
        // SAFETY: `url` outlives both calls. The original URL follows the
        // create rule and an error, when set, is owned by the caller.
        unsafe {
            let mut error: CFErrorRef = std::ptr::null_mut();
            let mut translocated = false;
            let ok = is_translocated(url.as_concrete_TypeRef(), &mut translocated, &mut error);
            if !error.is_null() {
                CFRelease(error.cast());
            }
            if ok == 0 || !translocated {
                return None;
            }
            let mut error: CFErrorRef = std::ptr::null_mut();
            let original = create_original(url.as_concrete_TypeRef(), &mut error);
            if !error.is_null() {
                CFRelease(error.cast());
            }
            if original.is_null() {
                return None;
            }
            CFURL::wrap_under_create_rule(original).to_path()
        }
    }
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

    #[cfg(target_os = "macos")]
    #[test]
    fn a_bundle_that_runs_in_place_is_not_translocated() {
        assert!(translocation::functions().is_some());
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join(BUNDLE_NAME);
        std::fs::create_dir_all(&bundle).unwrap();
        assert_eq!(translocated_original(&bundle), None);
    }
}
