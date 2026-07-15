use std::path::{Path, PathBuf};

use super::release::platform_archive_name;
use super::{InstallMethod, InstallPlan, Release};

pub(crate) fn install_plan(release: Release) -> InstallPlan {
    let launch = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aven"));
    let canonical = std::fs::canonicalize(&launch).unwrap_or_else(|_| launch.clone());
    let supported = platform_archive_name().is_ok();
    let writable = can_stage_beside(&canonical);
    InstallPlan {
        release,
        method: classify_install(&launch, &canonical, supported, writable),
    }
}

fn classify_install(
    launch: &Path,
    canonical: &Path,
    supported: bool,
    writable: bool,
) -> InstallMethod {
    if !supported {
        return InstallMethod::Unsupported {
            reason: format!(
                "Aven release binaries do not support {}/{}.",
                std::env::consts::OS,
                std::env::consts::ARCH
            ),
        };
    }

    let paths = [launch, canonical]
        .into_iter()
        .map(|path| path.to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
    if paths
        .iter()
        .any(|path| path.contains("/cellar/") || path.contains("/homebrew/"))
    {
        return InstallMethod::Homebrew;
    }
    if paths.iter().any(|path| {
        path.starts_with("/nix/store/")
            || path.contains("/.nix-profile/")
            || path.contains("/nix/profile/")
    }) {
        return InstallMethod::Nix;
    }
    if paths.iter().any(|path| path.contains("/.cargo/bin/")) {
        return InstallMethod::Cargo;
    }
    if paths.iter().any(|path| {
        path.contains("/target/debug/")
            || path.contains("/target/release/")
            || path.ends_with("/target/debug/aven")
            || path.ends_with("/target/release/aven")
    }) {
        return InstallMethod::Development;
    }
    if writable {
        InstallMethod::Direct {
            target: canonical.to_path_buf(),
        }
    } else {
        InstallMethod::Unwritable {
            target: canonical.to_path_buf(),
        }
    }
}

pub(super) fn can_stage_beside(target: &Path) -> bool {
    let Some(parent) = target.parent() else {
        return false;
    };
    tempfile::Builder::new()
        .prefix(".aven-write-test-")
        .tempfile_in(parent)
        .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_managed_and_development_paths_before_writability() {
        assert_eq!(
            classify_install(
                Path::new("/opt/homebrew/bin/aven"),
                Path::new("/opt/homebrew/Cellar/aven/1.0.0/bin/aven"),
                true,
                true,
            ),
            InstallMethod::Homebrew
        );
        assert_eq!(
            classify_install(
                Path::new("/home/me/.cargo/bin/aven"),
                Path::new("/home/me/.cargo/bin/aven"),
                true,
                true,
            ),
            InstallMethod::Cargo
        );
        assert_eq!(
            classify_install(
                Path::new("/nix/store/abc-aven/bin/aven"),
                Path::new("/nix/store/abc-aven/bin/aven"),
                true,
                true,
            ),
            InstallMethod::Nix
        );
        assert_eq!(
            classify_install(
                Path::new("/code/aven/target/debug/aven"),
                Path::new("/code/aven/target/debug/aven"),
                true,
                true,
            ),
            InstallMethod::Development
        );
    }

    #[test]
    fn classifies_direct_unwritable_and_unsupported_paths() {
        assert_eq!(
            classify_install(
                Path::new("/home/me/.local/bin/aven"),
                Path::new("/home/me/.local/bin/aven"),
                true,
                true,
            ),
            InstallMethod::Direct {
                target: PathBuf::from("/home/me/.local/bin/aven")
            }
        );
        assert_eq!(
            classify_install(
                Path::new("/opt/bin/aven"),
                Path::new("/opt/bin/aven"),
                true,
                false,
            ),
            InstallMethod::Unwritable {
                target: PathBuf::from("/opt/bin/aven")
            }
        );
        assert!(matches!(
            classify_install(
                Path::new("/opt/bin/aven"),
                Path::new("/opt/bin/aven"),
                false,
                true,
            ),
            InstallMethod::Unsupported { .. }
        ));
    }
}
