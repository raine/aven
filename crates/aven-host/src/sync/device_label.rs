//! This computer's name, offered as its device label.

#[cfg(target_os = "macos")]
pub(super) fn automatic_label() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).ok())
        .flatten()
}

#[cfg(target_os = "linux")]
pub(super) fn automatic_label() -> Option<String> {
    let mut bytes = [0_u8; 256];
    // SAFETY: `bytes` is writable for the supplied length. gethostname writes
    // at most that many bytes and does not retain the pointer.
    if unsafe { libc::gethostname(bytes.as_mut_ptr().cast(), bytes.len()) } != 0 {
        return None;
    }
    let length = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    std::str::from_utf8(&bytes[..length])
        .ok()
        .map(str::to_string)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(super) fn automatic_label() -> Option<String> {
    None
}
