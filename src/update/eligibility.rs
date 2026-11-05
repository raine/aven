use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use super::release::{ReleaseChannel, platform_archive_name};
use super::{InstallMethod, InstallPlan, Release};

pub(crate) fn install_plan(release: Release) -> InstallPlan {
    let launch = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("aven"));
    let canonical = std::fs::canonicalize(&launch).unwrap_or_else(|_| launch.clone());
    let supported = platform_archive_name().is_ok();
    let writable = can_stage_beside(&replaced_path(&canonical));
    let channel = release_channel();
    InstallPlan {
        release,
        method: classify_install(&launch, &canonical, supported, writable, channel),
        channel,
    }
}

/// The channel this process updates within, decided once from where its
/// executable lives.
pub(crate) fn release_channel() -> ReleaseChannel {
    static CHANNEL: OnceLock<ReleaseChannel> = OnceLock::new();
    *CHANNEL.get_or_init(|| {
        let canonical = std::env::current_exe().and_then(std::fs::canonicalize);
        match canonical {
            Ok(canonical) => channel_for(
                &canonical,
                dirs::home_dir().as_deref(),
                cfg!(all(target_os = "macos", feature = "desktop")),
            ),
            Err(_) => ReleaseChannel::Terminal,
        }
    })
}

/// A Desktop install is a desktop build running from an `Aven.app` that is not
/// one of the terminal channel's bundles: the install script's bundle under
/// `~/.local/share/aven`, or a Homebrew formula's in its Cellar. Older terminal
/// releases were desktop builds too, so the build alone does not decide it.
#[cfg(not(target_os = "macos"))]
fn channel_for(_: &Path, _: Option<&Path>, _: bool) -> ReleaseChannel {
    ReleaseChannel::Terminal
}

#[cfg(target_os = "macos")]
fn channel_for(canonical: &Path, home: Option<&Path>, desktop_build: bool) -> ReleaseChannel {
    if !desktop_build {
        return ReleaseChannel::Terminal;
    }
    let Some(bundle) = crate::app_bundle::enclosing_bundle(canonical) else {
        return ReleaseChannel::Terminal;
    };
    let script_install =
        home.is_some_and(|home| bundle == crate::app_bundle::direct_install_bundle(home));
    if script_install || is_homebrew_path(&bundle) {
        ReleaseChannel::Terminal
    } else {
        ReleaseChannel::Desktop
    }
}

fn is_homebrew_path(path: &Path) -> bool {
    let path = path.to_string_lossy().to_ascii_lowercase();
    path.contains("/cellar/") || path.contains("/homebrew/")
}

fn classify_install(
    launch: &Path,
    canonical: &Path,
    supported: bool,
    writable: bool,
    channel: ReleaseChannel,
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

    // A cask links its bin entry into Homebrew's prefix, but the app it links
    // to updates itself, so a Desktop install goes by where the app lives.
    let considered: &[&Path] = match channel {
        ReleaseChannel::Terminal => &[launch, canonical],
        ReleaseChannel::Desktop => &[canonical],
    };
    if considered.iter().any(|path| is_homebrew_path(path)) {
        return InstallMethod::Homebrew;
    }
    let paths = considered
        .iter()
        .map(|path| path.to_string_lossy().to_ascii_lowercase())
        .collect::<Vec<_>>();
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

/// The path a direct update replaces: the whole `Aven.app` for a bundled executable.
fn replaced_path(canonical: &Path) -> PathBuf {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = crate::app_bundle::enclosing_bundle(canonical) {
        return bundle;
    }
    canonical.to_path_buf()
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

    #[cfg(target_os = "macos")]
    #[test]
    fn bundled_executables_replace_the_whole_bundle() {
        assert_eq!(
            replaced_path(Path::new(
                "/Users/me/.local/share/aven/Aven.app/Contents/MacOS/aven"
            )),
            PathBuf::from("/Users/me/.local/share/aven/Aven.app")
        );
        assert_eq!(
            replaced_path(Path::new("/Users/me/.local/bin/aven")),
            PathBuf::from("/Users/me/.local/bin/aven")
        );
        assert_eq!(
            classify_install(
                Path::new("/opt/homebrew/bin/aven"),
                Path::new("/opt/homebrew/Cellar/aven/1.0.0/Aven.app/Contents/MacOS/aven"),
                true,
                true,
                ReleaseChannel::Terminal,
            ),
            InstallMethod::Homebrew
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn desktop_builds_in_terminal_bundles_stay_on_the_terminal_channel() {
        let home = Path::new("/Users/me");
        let channel = |path: &str, desktop| channel_for(Path::new(path), Some(home), desktop);
        let applications = "/Applications/Aven.app/Contents/MacOS/aven";
        assert_eq!(channel(applications, true), ReleaseChannel::Desktop);
        assert_eq!(
            channel("/Users/me/Applications/Aven.app/Contents/MacOS/aven", true),
            ReleaseChannel::Desktop
        );
        assert_eq!(
            channel("/Users/me/Apps/Aven.app/Contents/MacOS/aven", true),
            ReleaseChannel::Desktop,
            "a Desktop app moved elsewhere must not become terminal-only"
        );
        assert_eq!(channel(applications, false), ReleaseChannel::Terminal);
        assert_eq!(
            channel(
                "/Users/me/.local/share/aven/Aven.app/Contents/MacOS/aven",
                true
            ),
            ReleaseChannel::Terminal
        );
        assert_eq!(
            channel(
                "/opt/homebrew/Cellar/aven/1.0.0/Aven.app/Contents/MacOS/aven",
                true
            ),
            ReleaseChannel::Terminal
        );
        assert_eq!(
            channel("/Users/me/.local/bin/aven", true),
            ReleaseChannel::Terminal
        );
    }

    #[test]
    fn a_cask_link_does_not_make_desktop_package_managed() {
        let cask_link = Path::new("/opt/homebrew/bin/aven");
        let app = Path::new("/Applications/Aven.app/Contents/MacOS/aven");
        assert_eq!(
            classify_install(cask_link, app, true, true, ReleaseChannel::Desktop),
            InstallMethod::Direct {
                target: app.to_path_buf()
            }
        );
        assert_eq!(
            classify_install(cask_link, app, true, true, ReleaseChannel::Terminal),
            InstallMethod::Homebrew,
            "terminal installs keep the existing package-manager restriction"
        );
        assert_eq!(
            classify_install(
                cask_link,
                Path::new("/opt/homebrew/Cellar/aven/1.0.0/Aven.app/Contents/MacOS/aven"),
                true,
                true,
                ReleaseChannel::Desktop,
            ),
            InstallMethod::Homebrew
        );
    }

    #[test]
    fn classifies_managed_and_development_paths_before_writability() {
        assert_eq!(
            classify_install(
                Path::new("/opt/homebrew/bin/aven"),
                Path::new("/opt/homebrew/Cellar/aven/1.0.0/bin/aven"),
                true,
                true,
                ReleaseChannel::Terminal,
            ),
            InstallMethod::Homebrew
        );
        assert_eq!(
            classify_install(
                Path::new("/home/me/.cargo/bin/aven"),
                Path::new("/home/me/.cargo/bin/aven"),
                true,
                true,
                ReleaseChannel::Terminal,
            ),
            InstallMethod::Cargo
        );
        assert_eq!(
            classify_install(
                Path::new("/nix/store/abc-aven/bin/aven"),
                Path::new("/nix/store/abc-aven/bin/aven"),
                true,
                true,
                ReleaseChannel::Terminal,
            ),
            InstallMethod::Nix
        );
        assert_eq!(
            classify_install(
                Path::new("/code/aven/target/debug/aven"),
                Path::new("/code/aven/target/debug/aven"),
                true,
                true,
                ReleaseChannel::Terminal,
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
                ReleaseChannel::Terminal,
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
                ReleaseChannel::Terminal,
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
                ReleaseChannel::Terminal,
            ),
            InstallMethod::Unsupported { .. }
        ));
    }
}
