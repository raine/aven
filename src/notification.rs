pub(crate) fn notify_intake_task_added_if_tui_exited(tui_pid: u32, task_ref: &str, title: &str) {
    notify_intake_task_added_with(
        process_is_running(tui_pid),
        task_ref,
        title,
        show_notification,
    );
}

fn notify_intake_task_added_with(
    tui_running: bool,
    task_ref: &str,
    title: &str,
    notify: impl FnOnce(&str),
) {
    if !tui_running {
        notify(&format!("Added {task_ref}: {title}"));
    }
}

#[cfg(unix)]
fn process_is_running(pid: u32) -> bool {
    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }

    let Ok(pid) = i32::try_from(pid) else {
        return false;
    };
    // Signal zero checks process existence without delivering a signal.
    let result = unsafe { kill(pid, 0) };
    result == 0 || std::io::Error::last_os_error().kind() == std::io::ErrorKind::PermissionDenied
}

#[cfg(windows)]
fn process_is_running(pid: u32) -> bool {
    use std::ffi::c_void;

    const SYNCHRONIZE: u32 = 0x0010_0000;
    const WAIT_TIMEOUT: u32 = 0x0000_0102;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit_handle: i32, process_id: u32) -> *mut c_void;
        fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    // The handle is closed before returning and is used only for a zero-time wait.
    unsafe {
        let handle = OpenProcess(SYNCHRONIZE, 0, pid);
        if handle.is_null() {
            return false;
        }
        let wait = WaitForSingleObject(handle, 0);
        CloseHandle(handle);
        wait == WAIT_TIMEOUT
    }
}

fn show_notification(message: &str) {
    if let Err(error) = send_notification("aven", message) {
        tracing::debug!(error = %format!("{error:#}"), "failed to send notification");
    }
}

/// Hands the notification to the platform service. Success means it was
/// accepted, not that it was displayed.
pub(crate) fn send_notification(title: &str, message: &str) -> anyhow::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use mac_notification_sys::{Notification, set_application};

        // mac-notification-sys opens a "Choose Application" dialog when sending
        // without an application, and accepts only the first one per process.
        if let Err(error) = set_application(macos::notification_application()) {
            tracing::debug!(%error, "failed to set notification application");
        }
        Notification::default()
            .title(title)
            .message(message)
            .send()?;
    }

    #[cfg(not(target_os = "macos"))]
    {
        notify_rust::Notification::new()
            .summary(title)
            .body(message)
            .show()?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
mod macos {
    use std::path::Path;
    use std::sync::OnceLock;

    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{Boolean, OSStatus, TCFType};
    use core_foundation::bundle::CFBundle;
    use core_foundation::string::{CFString, CFStringRef};
    use core_foundation::url::{CFURL, CFURLRef};

    pub(super) use crate::app_bundle::{AVEN_BUNDLE_ID, enclosing_bundle};
    pub(super) const FALLBACK_BUNDLE_ID: &str = "com.apple.Terminal";

    #[link(name = "CoreServices", kind = "framework")]
    unsafe extern "C" {
        fn LSRegisterURL(url: CFURLRef, update: Boolean) -> OSStatus;
        fn LSCopyApplicationURLsForBundleIdentifier(
            bundle_id: CFStringRef,
            error: *mut *mut std::ffi::c_void,
        ) -> CFArrayRef;
    }

    /// Notifications appear as Aven when this process runs from a registered
    /// Aven.app, and as Terminal otherwise.
    pub(super) fn notification_application() -> &'static str {
        static APPLICATION: OnceLock<&'static str> = OnceLock::new();
        APPLICATION.get_or_init(|| {
            let executable = std::env::current_exe().and_then(std::fs::canonicalize);
            choose_application(
                executable.ok().as_deref(),
                bundle_identifier,
                register_bundle,
            )
        })
    }

    pub(super) fn choose_application(
        executable: Option<&Path>,
        identifier: impl FnOnce(&Path) -> Option<String>,
        register: impl FnOnce(&Path) -> Result<(), String>,
    ) -> &'static str {
        let Some(bundle) = executable.and_then(enclosing_bundle) else {
            return FALLBACK_BUNDLE_ID;
        };
        let found = identifier(&bundle);
        if found.as_deref() != Some(AVEN_BUNDLE_ID) {
            tracing::debug!(
                bundle = %bundle.display(),
                identifier = ?found,
                "executable bundle is not Aven; notifying as Terminal"
            );
            return FALLBACK_BUNDLE_ID;
        }
        match register(&bundle) {
            Ok(()) => AVEN_BUNDLE_ID,
            Err(error) => {
                tracing::debug!(
                    bundle = %bundle.display(),
                    %error,
                    "failed to register Aven bundle; notifying as Terminal"
                );
                FALLBACK_BUNDLE_ID
            }
        }
    }

    pub(super) fn bundle_identifier(bundle: &Path) -> Option<String> {
        let url = CFURL::from_path(bundle, true)?;
        let info = CFBundle::new(url)?.info_dictionary();
        info.find(CFString::from_static_string("CFBundleIdentifier"))?
            .downcast::<CFString>()
            .map(|identifier| identifier.to_string())
    }

    /// Registers the bundle at its current path, which changes on Homebrew
    /// upgrades, and confirms Launch Services resolves the Aven identifier.
    fn register_bundle(bundle: &Path) -> Result<(), String> {
        let url = CFURL::from_path(bundle, true).ok_or("bundle path is not a URL")?;
        let status = unsafe { LSRegisterURL(url.as_concrete_TypeRef(), 1) };
        if status != 0 {
            return Err(format!("LSRegisterURL returned {status}"));
        }
        let identifier = CFString::from_static_string(AVEN_BUNDLE_ID);
        let urls = unsafe {
            LSCopyApplicationURLsForBundleIdentifier(
                identifier.as_concrete_TypeRef(),
                std::ptr::null_mut(),
            )
        };
        if urls.is_null() {
            return Err("Launch Services does not resolve the bundle identifier".into());
        }
        // The copy follows the create rule; wrapping releases it.
        drop(unsafe { CFArray::<CFURL>::wrap_under_create_rule(urls) });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exited_tui_sends_task_notification() {
        let mut message = None;

        notify_intake_task_added_with(false, "APP-7KQ9", "Fix slack dispatch", |value| {
            message = Some(value.to_string());
        });

        assert_eq!(
            message.as_deref(),
            Some("Added APP-7KQ9: Fix slack dispatch")
        );
    }

    #[test]
    fn running_tui_suppresses_task_notification() {
        notify_intake_task_added_with(true, "APP-7KQ9", "Fix slack dispatch", |_| {
            panic!("running TUI should suppress the notification");
        });
    }

    #[test]
    fn current_process_is_running() {
        assert!(process_is_running(std::process::id()));
    }

    #[cfg(target_os = "macos")]
    mod macos_application {
        use std::path::Path;

        use crate::notification::macos::*;

        #[test]
        fn bundle_executable_resolves_enclosing_bundle() {
            assert_eq!(
                enclosing_bundle(Path::new(
                    "/opt/homebrew/Cellar/aven/0.2.0/Aven.app/Contents/MacOS/aven"
                ))
                .as_deref(),
                Some(Path::new("/opt/homebrew/Cellar/aven/0.2.0/Aven.app"))
            );
        }

        #[test]
        fn bare_and_dev_executables_have_no_bundle() {
            for path in [
                "/usr/local/bin/aven",
                "/Users/me/code/aven/target/debug/aven",
                "/Applications/Aven.app/Contents/MacOS/other",
                "/tmp/Aven.app/Contents/Resources/aven",
                "/tmp/Aven/Contents/MacOS/aven",
                "aven",
            ] {
                assert_eq!(enclosing_bundle(Path::new(path)), None, "{path}");
            }
        }

        #[test]
        fn registered_aven_bundle_notifies_as_aven() {
            let executable = Path::new("/x/Aven.app/Contents/MacOS/aven");
            let application = choose_application(
                Some(executable),
                |bundle| {
                    assert_eq!(bundle, Path::new("/x/Aven.app"));
                    Some(AVEN_BUNDLE_ID.into())
                },
                |_| Ok(()),
            );
            assert_eq!(application, AVEN_BUNDLE_ID);
        }

        #[test]
        fn non_aven_or_unregistered_bundle_falls_back_to_terminal() {
            let executable = Path::new("/x/Aven.app/Contents/MacOS/aven");
            assert_eq!(
                choose_application(
                    Some(executable),
                    |_| Some("com.example.Other".into()),
                    |_| panic!("only Aven bundles are registered"),
                ),
                FALLBACK_BUNDLE_ID
            );
            assert_eq!(
                choose_application(Some(executable), |_| None, |_| unreachable!()),
                FALLBACK_BUNDLE_ID
            );
            assert_eq!(
                choose_application(
                    Some(executable),
                    |_| Some(AVEN_BUNDLE_ID.into()),
                    |_| Err("not registered".into()),
                ),
                FALLBACK_BUNDLE_ID
            );
        }

        #[test]
        fn bare_executable_falls_back_to_terminal_without_bundle_lookup() {
            assert_eq!(
                choose_application(
                    Some(Path::new("/usr/local/bin/aven")),
                    |_| unreachable!(),
                    |_| unreachable!(),
                ),
                FALLBACK_BUNDLE_ID
            );
            assert_eq!(
                choose_application(None, |_| unreachable!(), |_| unreachable!()),
                FALLBACK_BUNDLE_ID
            );
        }

        #[test]
        fn reads_identifier_from_bundle_info_plist() {
            let temp = tempfile::tempdir().unwrap();
            let bundle = temp.path().join("Aven.app");
            std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
            std::fs::write(
                bundle.join("Contents/Info.plist"),
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>fi.zendit.Aven</string>
    <key>CFBundleExecutable</key>
    <string>aven</string>
</dict>
</plist>
"#,
            )
            .unwrap();

            assert_eq!(bundle_identifier(&bundle).as_deref(), Some(AVEN_BUNDLE_ID));
            assert_eq!(bundle_identifier(temp.path()), None);
        }
    }
}
