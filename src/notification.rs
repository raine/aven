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
    #[cfg(target_os = "macos")]
    {
        use mac_notification_sys::{Notification, set_application};

        if let Err(error) = set_application("com.apple.Terminal") {
            tracing::debug!(%error, "failed to set notification application");
        }
        if let Err(error) = Notification::default()
            .title("aven")
            .message(message)
            .send()
        {
            tracing::debug!(%error, "failed to send notification");
        }
    }

    #[cfg(not(target_os = "macos"))]
    {
        if let Err(error) = notify_rust::Notification::new()
            .summary("aven")
            .body(message)
            .show()
        {
            tracing::debug!(%error, "failed to send notification");
        }
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
}
