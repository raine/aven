//! Notifications from the command line and the daemon: which way they reach
//! the platform, and the CLI's own "Added" message.
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use aven_core::db::Database;
use aven_core::notification_outbox::QueuedNotification;
use aven_host::notify::{self, Authorization, Deliver, Report, Request};

/// How long the "Added" message may wait for delivery before it is dropped.
const ADDED_EXPIRY_SECONDS: i64 = 10 * 60;

/// Queues "Added …" for a task the TUI's background add created, unless the
/// TUI that asked for it still runs and shows it itself, then delivers it.
pub(crate) async fn notify_intake_task_added_if_tui_exited(
    database: &Database,
    tui_pid: u32,
    workspace_key: &str,
    task_id: &str,
    task_ref: &str,
    title: &str,
) {
    let Some(notification) = intake_task_added(
        process_is_running(tui_pid),
        workspace_key,
        task_id,
        task_ref,
        title,
    ) else {
        return;
    };
    if let Err(error) = queue_and_deliver(database, notification).await {
        tracing::debug!(error = %format!("{error:#}"), "failed to send notification");
    }
}

fn intake_task_added(
    tui_running: bool,
    workspace_key: &str,
    task_id: &str,
    task_ref: &str,
    title: &str,
) -> Option<QueuedNotification> {
    if tui_running {
        return None;
    }
    let event = notify::new_event();
    Some(QueuedNotification {
        tag: notify::task_tag(workspace_key, task_id, &event),
        event,
        kind: "added".to_string(),
        title: format!("Added {task_ref}"),
        body: title.to_string(),
        expires_at: notify::unix_now() + ADDED_EXPIRY_SECONDS,
    })
}

async fn queue_and_deliver(database: &Database, notification: QueuedNotification) -> Result<()> {
    database
        .queue_notifications(&[], &[notification], notify::unix_now())
        .await?;
    notify::deliver_pending(database, &delivery(database.path())).await?;
    Ok(())
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

/// How this process reaches the platform's notification service, for the
/// database at `database`.
///
/// On macOS, an executable in Aven's bundle delivers through a short-lived
/// child of itself, so no long-lived process takes clicks from the app.
/// Anything else posts with the older notification API under Terminal's
/// identity, which a bundle can't use once Aven has used the newer one.
///
/// On Linux, a build with the desktop app opens it at what a clicked
/// notification is about, for as long as this process runs to hear the
/// click.
pub(crate) fn delivery(database: &Path) -> Arc<dyn Deliver> {
    #[cfg(all(target_os = "linux", feature = "desktop"))]
    if let Some(opening) = desktop::opening_app(database) {
        return opening;
    }
    #[cfg(not(all(target_os = "linux", feature = "desktop")))]
    let _ = database;
    permission_child().unwrap_or_else(|| Arc::new(Unbundled))
}

/// The short-lived child that reads and asks for notification permission,
/// when this executable is in Aven's bundle. Elsewhere Aven posts as
/// Terminal, whose permission this can't read.
fn permission_child() -> Option<Arc<dyn Deliver>> {
    #[cfg(target_os = "macos")]
    if let Some(executable) = macos::bundle_executable() {
        return Some(Arc::new(notify::child::ChildProcess { executable }));
    }
    None
}

/// What the user allowed for Aven's notifications, or `None` where this
/// can't read it.
pub(crate) fn authorization() -> Option<Result<Authorization>> {
    permission_child().map(|child| notify::authorization(child.as_ref()))
}

/// What `aven daemon status` and `aven doctor` say about notifications, and
/// what to do about it.
pub(crate) fn describe(authorization: Authorization) -> (&'static str, Option<&'static str>) {
    match authorization {
        Authorization::Allowed => ("allowed", None),
        Authorization::Denied => (
            "denied",
            Some("turn on Allow notifications in System Settings > Notifications > Aven"),
        ),
        Authorization::NotAsked => (
            "not asked",
            Some("run `aven config set reminders.enabled true` in a terminal and choose Allow"),
        ),
        Authorization::Unavailable => ("unavailable", None),
    }
}

/// Whether macOS hasn't asked the user about Aven's notifications yet.
pub(crate) fn not_asked() -> bool {
    matches!(authorization(), Some(Ok(Authorization::NotAsked)))
}

/// Asks the user to allow notifications and waits for the answer. The
/// question shows as a notification and stays open only while the asking
/// process runs, so only a process the user is waiting on calls this.
pub(crate) fn ask() -> Option<Authorization> {
    let child = permission_child()?;
    // Without an answer in time, macOS records the question as declined.
    notify::ask(child.as_ref())
        .or_else(|error| {
            tracing::debug!(error = %format!("{error:#}"), "notification question");
            notify::authorization(child.as_ref())
        })
        .ok()
}

/// Asks the user to allow notifications from a command they run in a
/// terminal, when macOS hasn't asked them yet, and says what came of it.
/// Does nothing without a terminal on stdin.
pub(crate) fn ask_in_terminal() {
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() || !not_asked() {
        return;
    }
    println!(
        "macOS will ask whether Aven may send notifications. Choose Allow to get reminders; waiting for your answer..."
    );
    match ask() {
        Some(Authorization::Allowed) => println!("Notifications are allowed."),
        Some(Authorization::Denied) => println!(
            "Notifications are off. Turn them on in System Settings > Notifications > Aven to get reminders."
        ),
        _ => println!("No answer came. Run this again to be asked."),
    }
}

/// Posts each notification in this process. Nothing here can tell whether
/// notifications are allowed or what is shown.
struct Unbundled;

impl Deliver for Unbundled {
    fn deliver(&self, request: &Request) -> Result<Report> {
        Ok(Report {
            authorization: Authorization::Allowed,
            errors: request
                .posts
                .iter()
                .map(|post| {
                    send_notification(&post.title, &post.body)
                        .err()
                        .map(|error| format!("{error:#}"))
                })
                .collect(),
            delivered: None,
        })
    }
}

/// Hands the notification to the platform service. Success means it was
/// accepted, not that it was displayed.
fn send_notification(title: &str, message: &str) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        use mac_notification_sys::{Notification, set_application};

        // mac-notification-sys opens a "Choose Application" dialog when sending
        // without an application, and accepts only the first one per process.
        if let Err(error) = set_application(macos::FALLBACK_BUNDLE_ID) {
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

/// `aven internal notify`: the child side of [`notify::child`], in its own
/// short-lived process.
pub(crate) fn serve_child() -> Result<()> {
    use std::io::{Read, Write};

    let mut input = Vec::new();
    std::io::stdin().read_to_end(&mut input)?;
    #[cfg(target_os = "macos")]
    let output = notify::child::serve(&input, &notify::un::InProcess)?;
    #[cfg(not(target_os = "macos"))]
    let output = notify::child::serve(&input, &Unbundled)?;
    std::io::stdout().write_all(&output)?;
    Ok(())
}

/// Clicks that open Aven Desktop on Linux.
#[cfg(all(target_os = "linux", feature = "desktop"))]
mod desktop {
    use std::os::unix::process::CommandExt as _;
    use std::path::Path;
    use std::process::{Command, Stdio};
    use std::sync::Arc;

    use aven_host::notify::Deliver;
    use aven_host::notify::freedesktop::{Click, Freedesktop};

    use crate::desktop_launch::{OPEN_NOTIFICATION, has_display};

    /// Posts through the session bus, and opens the app for a click.
    pub(super) fn opening_app(database: &Path) -> Option<Arc<dyn Deliver>> {
        let executable = std::env::current_exe().ok()?;
        let database = database.to_path_buf();
        Some(Arc::new(Freedesktop::new("Aven", move |click| {
            open(&executable, &database, click);
        })))
    }

    /// Starts the app at what `click` is about, with the click's token so
    /// its window can take focus. An app already open on the database takes
    /// the click over from it.
    fn open(executable: &Path, database: &Path, click: Click) {
        if !has_display() {
            tracing::warn!("a notification was clicked, but there is no display to open Aven on");
            return;
        }
        let mut command = Command::new(executable);
        command
            .args(OPEN_NOTIFICATION)
            .arg(database)
            .arg(&click.tag)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        if let Some(token) = &click.activation_token {
            command
                .env("XDG_ACTIVATION_TOKEN", token)
                .env("DESKTOP_STARTUP_ID", token);
        }
        match command.spawn() {
            Ok(mut child) => {
                std::thread::spawn(move || child.wait());
            }
            Err(error) => {
                tracing::warn!(%error, "could not open Aven for a clicked notification");
            }
        }
    }
}

#[cfg(target_os = "macos")]
pub(crate) use macos::bundle_identifier;

#[cfg(target_os = "macos")]
mod macos {
    use std::path::{Path, PathBuf};
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

    /// This process's executable when it is the main executable of a
    /// registered Aven bundle.
    pub(super) fn bundle_executable() -> Option<PathBuf> {
        static EXECUTABLE: OnceLock<Option<PathBuf>> = OnceLock::new();
        EXECUTABLE
            .get_or_init(|| {
                let executable = std::env::current_exe()
                    .and_then(std::fs::canonicalize)
                    .ok()?;
                in_aven_bundle(&executable, bundle_identifier, register_bundle)
                    .then_some(executable)
            })
            .clone()
    }

    pub(super) fn in_aven_bundle(
        executable: &Path,
        identifier: impl FnOnce(&Path) -> Option<String>,
        register: impl FnOnce(&Path) -> Result<(), String>,
    ) -> bool {
        let Some(bundle) = enclosing_bundle(executable) else {
            return false;
        };
        let found = identifier(&bundle);
        if found.as_deref() != Some(AVEN_BUNDLE_ID) {
            tracing::debug!(
                bundle = %bundle.display(),
                identifier = ?found,
                "executable bundle is not Aven; notifying as Terminal"
            );
            return false;
        }
        match register(&bundle) {
            Ok(()) => true,
            Err(error) => {
                tracing::debug!(
                    bundle = %bundle.display(),
                    %error,
                    "failed to register Aven bundle; notifying as Terminal"
                );
                false
            }
        }
    }

    pub(crate) fn bundle_identifier(bundle: &Path) -> Option<String> {
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
    fn exited_tui_queues_task_notification() {
        let notification =
            intake_task_added(false, "home", "T1", "APP-7KQ9", "Fix slack dispatch").unwrap();

        assert_eq!(notification.title, "Added APP-7KQ9");
        assert_eq!(notification.body, "Fix slack dispatch");
        assert_eq!(
            notify::target(&notification.tag),
            Some(notify::Target::Task {
                workspace: "home".into(),
                task: "T1".into(),
            })
        );
        assert!(notification.expires_at > notify::unix_now());
    }

    #[test]
    fn running_tui_suppresses_task_notification() {
        assert_eq!(
            intake_task_added(true, "home", "T1", "APP-7KQ9", "Fix slack dispatch"),
            None
        );
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
        fn registered_aven_bundle_delivers_as_aven() {
            let executable = Path::new("/x/Aven.app/Contents/MacOS/aven");
            assert!(in_aven_bundle(
                executable,
                |bundle| {
                    assert_eq!(bundle, Path::new("/x/Aven.app"));
                    Some(AVEN_BUNDLE_ID.into())
                },
                |_| Ok(()),
            ));
        }

        #[test]
        fn non_aven_or_unregistered_bundle_falls_back_to_terminal() {
            let executable = Path::new("/x/Aven.app/Contents/MacOS/aven");
            assert!(!in_aven_bundle(
                executable,
                |_| Some("com.example.Other".into()),
                |_| panic!("only Aven bundles are registered"),
            ));
            assert!(!in_aven_bundle(executable, |_| None, |_| unreachable!()));
            assert!(!in_aven_bundle(
                executable,
                |_| Some(AVEN_BUNDLE_ID.into()),
                |_| Err("not registered".into()),
            ));
        }

        #[test]
        fn bare_executable_falls_back_to_terminal_without_bundle_lookup() {
            assert!(!in_aven_bundle(
                Path::new("/usr/local/bin/aven"),
                |_| unreachable!(),
                |_| unreachable!(),
            ));
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
