#[cfg(target_os = "macos")]
use std::fs;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
use std::io;
#[cfg(all(not(test), any(target_os = "macos", target_os = "linux")))]
use std::io::Write;
#[cfg(all(target_os = "linux", not(test)))]
use std::io::{Read, Seek};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::process::Command as ProcessCommand;
#[cfg(any(target_os = "linux", test))]
use std::process::Output;

use anyhow::Result;

#[derive(Debug)]
pub(crate) struct ClipboardImage {
    pub(crate) filename: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum LinuxClipboardBackend {
    Wayland,
    X11,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct ClipboardCommand {
    pub(super) program: &'static str,
    pub(super) args: Vec<std::ffi::OsString>,
}

#[cfg(any(target_os = "linux", test))]
pub(super) struct ClipboardCommandOutput {
    pub(super) success: bool,
    pub(super) status: String,
    pub(super) stdout: Vec<u8>,
    pub(super) stderr: Vec<u8>,
}

#[cfg(any(target_os = "linux", test))]
impl From<Output> for ClipboardCommandOutput {
    fn from(output: Output) -> Self {
        Self {
            success: output.status.success(),
            status: output.status.to_string(),
            stdout: output.stdout,
            stderr: output.stderr,
        }
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy)]
pub(super) struct ClipboardImageFormat {
    pub(super) mime: &'static str,
    pub(super) extension: &'static str,
}

#[cfg(any(target_os = "linux", test))]
const CLIPBOARD_IMAGE_FORMATS: [ClipboardImageFormat; 5] = [
    ClipboardImageFormat {
        mime: "image/png",
        extension: "png",
    },
    ClipboardImageFormat {
        mime: "image/jpeg",
        extension: "jpg",
    },
    ClipboardImageFormat {
        mime: "image/jpg",
        extension: "jpg",
    },
    ClipboardImageFormat {
        mime: "image/gif",
        extension: "gif",
    },
    ClipboardImageFormat {
        mime: "image/webp",
        extension: "webp",
    },
];

#[cfg(any(target_os = "linux", test))]
pub(super) fn linux_clipboard_backend_order(
    wayland_display: Option<&std::ffi::OsStr>,
    xdg_session_type: Option<&std::ffi::OsStr>,
) -> [LinuxClipboardBackend; 2] {
    let wayland_session = wayland_display.is_some_and(|value| !value.is_empty())
        || xdg_session_type.is_some_and(|value| {
            value
                .to_str()
                .is_some_and(|value| value.eq_ignore_ascii_case("wayland"))
        });
    if wayland_session {
        [LinuxClipboardBackend::Wayland, LinuxClipboardBackend::X11]
    } else {
        [LinuxClipboardBackend::X11, LinuxClipboardBackend::Wayland]
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn clipboard_list_command(backend: LinuxClipboardBackend) -> ClipboardCommand {
    match backend {
        LinuxClipboardBackend::Wayland => ClipboardCommand {
            program: "wl-paste",
            args: vec!["-l".into()],
        },
        LinuxClipboardBackend::X11 => ClipboardCommand {
            program: "xclip",
            args: vec![
                "-selection".into(),
                "clipboard".into(),
                "-t".into(),
                "TARGETS".into(),
                "-o".into(),
            ],
        },
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn clipboard_read_command(
    backend: LinuxClipboardBackend,
    format: ClipboardImageFormat,
) -> ClipboardCommand {
    match backend {
        LinuxClipboardBackend::Wayland => ClipboardCommand {
            program: "wl-paste",
            args: vec!["--type".into(), format.mime.into()],
        },
        LinuxClipboardBackend::X11 => ClipboardCommand {
            program: "xclip",
            args: vec![
                "-selection".into(),
                "clipboard".into(),
                "-t".into(),
                format.mime.into(),
                "-o".into(),
            ],
        },
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn clipboard_write_command(backend: LinuxClipboardBackend) -> ClipboardCommand {
    match backend {
        LinuxClipboardBackend::Wayland => ClipboardCommand {
            program: "wl-copy",
            args: Vec::new(),
        },
        LinuxClipboardBackend::X11 => ClipboardCommand {
            program: "xclip",
            args: vec!["-selection".into(), "clipboard".into(), "-in".into()],
        },
    }
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn advertised_clipboard_image_format(output: &[u8]) -> Option<ClipboardImageFormat> {
    let advertised = String::from_utf8_lossy(output);
    CLIPBOARD_IMAGE_FORMATS.iter().copied().find(|format| {
        advertised
            .lines()
            .any(|line| line.trim().eq_ignore_ascii_case(format.mime))
    })
}

#[cfg(any(target_os = "linux", test))]
enum ClipboardBackendResult {
    Image(ClipboardImage),
    NoImage,
    Unavailable,
    Failed(String),
}

#[cfg(any(target_os = "linux", test))]
fn clipboard_command_reports_no_content(output: &ClipboardCommandOutput) -> bool {
    let stderr = String::from_utf8_lossy(&output.stderr);
    stderr.contains("Nothing is copied")
        || stderr.contains("There is no owner for the CLIPBOARD selection")
}

#[cfg(any(target_os = "linux", test))]
fn clipboard_command_failure(
    command: &ClipboardCommand,
    output: &ClipboardCommandOutput,
) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!("{} exited with {}", command.program, output.status)
    } else {
        format!(
            "{} exited with {}: {stderr}",
            command.program, output.status
        )
    }
}

#[cfg(any(target_os = "linux", test))]
fn read_linux_clipboard_backend(
    backend: LinuxClipboardBackend,
    run: &mut impl FnMut(&ClipboardCommand) -> io::Result<ClipboardCommandOutput>,
) -> ClipboardBackendResult {
    let list_command = clipboard_list_command(backend);
    let list_output = match run(&list_command) {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return ClipboardBackendResult::Unavailable;
        }
        Err(error) => {
            return ClipboardBackendResult::Failed(format!(
                "could not run {}: {error}",
                list_command.program
            ));
        }
    };
    if !list_output.success {
        if clipboard_command_reports_no_content(&list_output) {
            return ClipboardBackendResult::NoImage;
        }
        return ClipboardBackendResult::Failed(clipboard_command_failure(
            &list_command,
            &list_output,
        ));
    }
    let Some(format) = advertised_clipboard_image_format(&list_output.stdout) else {
        return ClipboardBackendResult::NoImage;
    };

    let read_command = clipboard_read_command(backend, format);
    let image_output = match run(&read_command) {
        Ok(output) => output,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return ClipboardBackendResult::Unavailable;
        }
        Err(error) => {
            return ClipboardBackendResult::Failed(format!(
                "could not run {}: {error}",
                read_command.program
            ));
        }
    };
    if !image_output.success {
        if clipboard_command_reports_no_content(&image_output) {
            return ClipboardBackendResult::NoImage;
        }
        return ClipboardBackendResult::Failed(clipboard_command_failure(
            &read_command,
            &image_output,
        ));
    }
    if image_output.stdout.is_empty() {
        return ClipboardBackendResult::NoImage;
    }
    ClipboardBackendResult::Image(ClipboardImage {
        filename: format!("pasted-image.{}", format.extension),
        bytes: image_output.stdout,
    })
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn read_linux_clipboard_image_with(
    backends: [LinuxClipboardBackend; 2],
    mut run: impl FnMut(&ClipboardCommand) -> io::Result<ClipboardCommandOutput>,
) -> Result<Option<ClipboardImage>> {
    let mut saw_no_image = false;
    let mut failures = Vec::new();
    for backend in backends {
        match read_linux_clipboard_backend(backend, &mut run) {
            ClipboardBackendResult::Image(image) => return Ok(Some(image)),
            ClipboardBackendResult::NoImage => saw_no_image = true,
            ClipboardBackendResult::Unavailable => {}
            ClipboardBackendResult::Failed(error) => failures.push(error),
        }
    }
    if saw_no_image {
        return Ok(None);
    }
    if !failures.is_empty() {
        anyhow::bail!(failures.join("; "));
    }
    anyhow::bail!("Linux clipboard image paste requires wl-paste or xclip")
}

#[cfg(any(target_os = "linux", test))]
pub(super) fn copy_linux_clipboard_with(
    backends: [LinuxClipboardBackend; 2],
    value: &str,
    mut run: impl FnMut(&ClipboardCommand, &[u8]) -> io::Result<ClipboardCommandOutput>,
) -> Result<()> {
    let mut failures = Vec::new();
    for backend in backends {
        let command = clipboard_write_command(backend);
        match run(&command, value.as_bytes()) {
            Ok(output) if output.success => return Ok(()),
            Ok(output) => failures.push(clipboard_command_failure(&command, &output)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => failures.push(format!("could not run {}: {error}", command.program)),
        }
    }
    if failures.is_empty() {
        anyhow::bail!("Linux clipboard copy requires wl-copy or xclip");
    }
    anyhow::bail!(failures.join("; "))
}

#[cfg(all(target_os = "linux", not(test)))]
fn run_clipboard_write_command(
    command: &ClipboardCommand,
    value: &[u8],
) -> io::Result<ClipboardCommandOutput> {
    let mut stderr = tempfile::tempfile()?;
    let mut child = ProcessCommand::new(command.program)
        .args(&command.args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(stderr.try_clone()?)
        .spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| io::Error::other("clipboard command standard input is unavailable"))?;
    let write_result = stdin.write_all(value);
    drop(stdin);
    let status = child.wait()?;
    write_result?;
    stderr.rewind()?;
    let mut stderr_output = Vec::new();
    stderr.read_to_end(&mut stderr_output)?;
    Ok(ClipboardCommandOutput {
        success: status.success(),
        status: status.to_string(),
        stdout: Vec::new(),
        stderr: stderr_output,
    })
}

#[cfg(target_os = "macos")]
pub(crate) fn read_clipboard_image() -> Result<Option<ClipboardImage>> {
    let temp = tempfile::Builder::new()
        .prefix("aven-clipboard-image-")
        .suffix(".png")
        .tempfile()?;
    let path = temp.path().to_path_buf();
    let script = r#"
set outPath to POSIX file (system attribute "AVEN_CLIPBOARD_IMAGE_PATH")
try
    set imageData to the clipboard as «class PNGf»
on error
    return "no-image"
end try
set fileRef to open for access outPath with write permission
try
    set eof of fileRef to 0
    write imageData to fileRef
    close access fileRef
on error errText
    try
        close access fileRef
    end try
    error errText
end try
return "ok"
"#;
    let output = ProcessCommand::new("osascript")
        .arg("-e")
        .arg(script)
        .env("AVEN_CLIPBOARD_IMAGE_PATH", &path)
        .output()?;
    if !output.status.success() {
        anyhow::bail!("osascript exited with {}", output.status);
    }
    if String::from_utf8_lossy(&output.stdout).trim() == "no-image" {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Ok(None);
    }
    Ok(Some(ClipboardImage {
        filename: "pasted-image.png".to_string(),
        bytes,
    }))
}

#[cfg(target_os = "linux")]
pub(crate) fn read_clipboard_image() -> Result<Option<ClipboardImage>> {
    let backends = linux_clipboard_backend_order(
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        std::env::var_os("XDG_SESSION_TYPE").as_deref(),
    );
    read_linux_clipboard_image_with(backends, |command| {
        ProcessCommand::new(command.program)
            .args(&command.args)
            .output()
            .map(Into::into)
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub(crate) fn read_clipboard_image() -> Result<Option<ClipboardImage>> {
    Ok(None)
}

/// Environment variables that mean the native clipboard tool may write to a
/// different machine than the one the user is looking at: an SSH session, or a
/// multiplexer whose client can attach from elsewhere. Multiplexer panes do not
/// inherit `SSH_*` from the attached client, so those need their own markers.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
const OSC52_SESSION_VARS: [&str; 5] = ["SSH_TTY", "SSH_CONNECTION", "TMUX", "ZELLIJ", "HERDR_ENV"];

#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(super) fn prefers_osc52(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> bool {
    OSC52_SESSION_VARS
        .iter()
        .any(|name| env(name).is_some_and(|value| !value.is_empty()))
}

#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(super) fn osc52_sequence(value: &str) -> String {
    use base64::Engine;
    let payload = base64::engine::general_purpose::STANDARD.encode(value);
    // BEL rather than ST: some terminals and multiplexers only honor BEL here.
    format!("\x1b]52;c;{payload}\x07")
}

/// Copies with the native tool, and also asks the terminal to copy via OSC 52
/// when the session may be remote. Elsewhere OSC 52 is only a fallback for a
/// failed native copy, such as a Linux host without wl-copy or xclip.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(super) fn copy_with_osc52(
    prefer_osc52: bool,
    value: &str,
    native: impl FnOnce(&str) -> Result<()>,
    osc52: impl FnOnce(&str) -> io::Result<()>,
) -> Result<()> {
    if prefer_osc52 {
        let osc52_result = osc52(value);
        let native_result = native(value);
        return match (osc52_result, native_result) {
            (Ok(()), _) | (_, Ok(())) => Ok(()),
            (Err(osc52_error), Err(native_error)) => {
                anyhow::bail!("{native_error}; OSC 52 write failed: {osc52_error}")
            }
        };
    }
    match native(value) {
        Ok(()) => Ok(()),
        Err(native_error) => osc52(value).map_err(|osc52_error| {
            anyhow::anyhow!("{native_error}; OSC 52 write failed: {osc52_error}")
        }),
    }
}

#[cfg(all(not(test), any(target_os = "macos", target_os = "linux")))]
fn write_osc52(value: &str) -> io::Result<()> {
    let sequence = osc52_sequence(value);
    match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
        Ok(mut tty) => {
            tty.write_all(sequence.as_bytes())?;
            tty.flush()
        }
        Err(_) => {
            let mut stdout = io::stdout();
            stdout.write_all(sequence.as_bytes())?;
            stdout.flush()
        }
    }
}

#[cfg(all(not(test), any(target_os = "macos", target_os = "linux")))]
fn session_prefers_osc52() -> bool {
    prefers_osc52(|name| std::env::var_os(name))
}

#[cfg(all(not(test), target_os = "macos"))]
pub(crate) fn copy_to_clipboard(value: &str) -> Result<()> {
    copy_with_osc52(
        session_prefers_osc52(),
        value,
        copy_with_pbcopy,
        write_osc52,
    )
}

#[cfg(all(not(test), target_os = "macos"))]
fn copy_with_pbcopy(value: &str) -> Result<()> {
    let mut child = ProcessCommand::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(value.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("pbcopy exited with {status}");
    }
    Ok(())
}

#[cfg(all(not(test), target_os = "linux"))]
pub(crate) fn copy_to_clipboard(value: &str) -> Result<()> {
    let backends = linux_clipboard_backend_order(
        std::env::var_os("WAYLAND_DISPLAY").as_deref(),
        std::env::var_os("XDG_SESSION_TYPE").as_deref(),
    );
    copy_with_osc52(
        session_prefers_osc52(),
        value,
        |value| copy_linux_clipboard_with(backends, value, run_clipboard_write_command),
        write_osc52,
    )
}

#[cfg(all(not(test), not(any(target_os = "macos", target_os = "linux"))))]
pub(crate) fn copy_to_clipboard(_value: &str) -> Result<()> {
    anyhow::bail!("clipboard copy is unsupported on this platform")
}

#[cfg(test)]
thread_local! {
    static TEST_CLIPBOARD: std::cell::RefCell<Option<String>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(crate) fn copy_to_clipboard(value: &str) -> Result<()> {
    TEST_CLIPBOARD.with(|clipboard| clipboard.replace(Some(value.to_string())));
    Ok(())
}

#[cfg(test)]
pub(crate) fn clipboard_text_for_test() -> Option<String> {
    TEST_CLIPBOARD.with(|clipboard| clipboard.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_osc52_in_ssh_and_multiplexer_sessions() {
        fn env<'a>(
            vars: &'a [(&'a str, &'a str)],
        ) -> impl Fn(&str) -> Option<std::ffi::OsString> + 'a {
            move |name| {
                vars.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.into())
            }
        }

        assert!(!prefers_osc52(env(&[("TERM", "xterm-ghostty")])));
        assert!(!prefers_osc52(env(&[("TMUX", "")])));
        for name in OSC52_SESSION_VARS {
            assert!(
                prefers_osc52(env(&[(name, "1")])),
                "{name} should prefer OSC 52"
            );
        }
    }

    #[test]
    fn osc52_sequence_base64_encodes_utf8_with_bel_terminator() {
        assert_eq!(osc52_sequence("할일 ✓"), "\x1b]52;c;7ZWg7J28IOKckw==\x07");
    }

    #[test]
    fn remote_sessions_write_osc52_and_succeed_without_a_native_clipboard() {
        let mut written = None;
        copy_with_osc52(
            true,
            "task",
            |_| anyhow::bail!("Linux clipboard copy requires wl-copy or xclip"),
            |value| {
                written = Some(value.to_string());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(written.as_deref(), Some("task"));
    }

    #[test]
    fn remote_sessions_also_copy_natively_when_the_terminal_write_fails() {
        let mut native = None;
        copy_with_osc52(
            true,
            "task",
            |value| {
                native = Some(value.to_string());
                Ok(())
            },
            |_| Err(io::Error::other("no tty")),
        )
        .unwrap();
        assert_eq!(native.as_deref(), Some("task"));

        let error = copy_with_osc52(
            true,
            "task",
            |_| anyhow::bail!("pbcopy exited with 1"),
            |_| Err(io::Error::other("no tty")),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "pbcopy exited with 1; OSC 52 write failed: no tty"
        );
    }

    #[test]
    fn local_sessions_use_osc52_only_after_the_native_copy_fails() {
        let mut osc52_calls = 0;
        copy_with_osc52(
            false,
            "task",
            |_| Ok(()),
            |_| {
                osc52_calls += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(osc52_calls, 0);

        copy_with_osc52(
            false,
            "task",
            |_| anyhow::bail!("Linux clipboard copy requires wl-copy or xclip"),
            |_| {
                osc52_calls += 1;
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(osc52_calls, 1);
    }

    #[test]
    fn selects_linux_clipboard_backend_from_session_environment() {
        use std::ffi::OsStr;

        assert_eq!(
            linux_clipboard_backend_order(Some(OsStr::new("wayland-0")), None),
            [LinuxClipboardBackend::Wayland, LinuxClipboardBackend::X11]
        );
        assert_eq!(
            linux_clipboard_backend_order(None, Some(OsStr::new("WAYLAND"))),
            [LinuxClipboardBackend::Wayland, LinuxClipboardBackend::X11]
        );
        assert_eq!(
            linux_clipboard_backend_order(None, Some(OsStr::new("x11"))),
            [LinuxClipboardBackend::X11, LinuxClipboardBackend::Wayland]
        );
    }

    #[test]
    fn selects_linux_clipboard_read_and_write_commands() {
        let format =
            advertised_clipboard_image_format(b"image/webp\nimage/gif\nimage/jpeg\nimage/png\n")
                .unwrap();

        assert_eq!(format.mime, "image/png");
        assert_eq!(format.extension, "png");
        assert_eq!(
            clipboard_list_command(LinuxClipboardBackend::Wayland),
            ClipboardCommand {
                program: "wl-paste",
                args: vec!["-l".into()],
            }
        );
        assert_eq!(
            clipboard_read_command(LinuxClipboardBackend::X11, format),
            ClipboardCommand {
                program: "xclip",
                args: vec![
                    "-selection".into(),
                    "clipboard".into(),
                    "-t".into(),
                    "image/png".into(),
                    "-o".into(),
                ],
            }
        );
        assert_eq!(
            clipboard_write_command(LinuxClipboardBackend::Wayland),
            ClipboardCommand {
                program: "wl-copy",
                args: Vec::new(),
            }
        );
        assert_eq!(
            clipboard_write_command(LinuxClipboardBackend::X11),
            ClipboardCommand {
                program: "xclip",
                args: vec!["-selection".into(), "clipboard".into(), "-in".into()],
            }
        );
    }

    #[test]
    fn falls_back_from_missing_wayland_tool_to_x11() {
        use std::collections::VecDeque;

        let mut responses = VecDeque::from([
            Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
            Ok(clipboard_test_output(true, b"image/webp\n", b"")),
            Ok(clipboard_test_output(true, b"webp bytes", b"")),
        ]);
        let mut commands = Vec::new();

        let image = read_linux_clipboard_image_with(
            [LinuxClipboardBackend::Wayland, LinuxClipboardBackend::X11],
            |command| {
                commands.push(command.clone());
                responses.pop_front().unwrap()
            },
        )
        .unwrap()
        .unwrap();

        assert_eq!(image.filename, "pasted-image.webp");
        assert_eq!(image.bytes, b"webp bytes");
        assert_eq!(commands[0].program, "wl-paste");
        assert_eq!(commands[1].program, "xclip");
        assert_eq!(commands[2].args[3], "image/webp");
    }

    #[test]
    fn checks_x11_when_wayland_clipboard_has_no_image() {
        use std::collections::VecDeque;

        let mut responses = VecDeque::from([
            Ok(clipboard_test_output(true, b"text/plain\n", b"")),
            Ok(clipboard_test_output(true, b"image/gif\n", b"")),
            Ok(clipboard_test_output(true, b"gif bytes", b"")),
        ]);

        let image = read_linux_clipboard_image_with(
            [LinuxClipboardBackend::Wayland, LinuxClipboardBackend::X11],
            |_| responses.pop_front().unwrap(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(image.filename, "pasted-image.gif");
        assert_eq!(image.bytes, b"gif bytes");
    }

    #[test]
    fn reports_non_image_content_when_an_available_backend_has_no_image() {
        use std::collections::VecDeque;

        let mut responses = VecDeque::from([
            Ok(clipboard_test_output(true, b"text/plain\n", b"")),
            Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
        ]);

        let image = read_linux_clipboard_image_with(
            [LinuxClipboardBackend::Wayland, LinuxClipboardBackend::X11],
            |_| responses.pop_front().unwrap(),
        )
        .unwrap();

        assert!(image.is_none());

        let mut responses = VecDeque::from([
            Ok(clipboard_test_output(false, b"", b"Nothing is copied")),
            Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
        ]);
        let empty = read_linux_clipboard_image_with(
            [LinuxClipboardBackend::Wayland, LinuxClipboardBackend::X11],
            |_| responses.pop_front().unwrap(),
        )
        .unwrap();

        assert!(empty.is_none());
    }

    #[test]
    fn distinguishes_missing_tools_from_clipboard_command_failures() {
        use std::collections::VecDeque;

        let missing = read_linux_clipboard_image_with(
            [LinuxClipboardBackend::X11, LinuxClipboardBackend::Wayland],
            |_| Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
        )
        .unwrap_err();
        assert_eq!(
            missing.to_string(),
            "Linux clipboard image paste requires wl-paste or xclip"
        );

        let mut responses = VecDeque::from([
            Ok(clipboard_test_output(false, b"", b"cannot open display")),
            Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
        ]);
        let failed = read_linux_clipboard_image_with(
            [LinuxClipboardBackend::X11, LinuxClipboardBackend::Wayland],
            |_| responses.pop_front().unwrap(),
        )
        .unwrap_err();
        assert_eq!(
            failed.to_string(),
            "xclip exited with exit status: 1: cannot open display"
        );
    }

    #[test]
    fn writes_linux_clipboard_payload_with_backend_fallback() {
        use std::collections::VecDeque;

        let mut responses = VecDeque::from([
            Ok(clipboard_test_output(
                false,
                b"",
                b"cannot connect to wayland display",
            )),
            Ok(clipboard_test_output(true, b"", b"")),
        ]);
        let mut commands = Vec::new();
        let mut payloads = Vec::new();

        let value = "task title\nsecond line";
        copy_linux_clipboard_with(
            [LinuxClipboardBackend::Wayland, LinuxClipboardBackend::X11],
            value,
            |command, payload| {
                commands.push(command.clone());
                payloads.push(payload.to_vec());
                responses.pop_front().unwrap()
            },
        )
        .unwrap();

        assert_eq!(commands[0].program, "wl-copy");
        assert_eq!(commands[1].program, "xclip");
        assert_eq!(payloads, vec![value.as_bytes().to_vec(); 2]);
    }

    #[test]
    fn reports_missing_linux_clipboard_tools() {
        let error = copy_linux_clipboard_with(
            [LinuxClipboardBackend::X11, LinuxClipboardBackend::Wayland],
            "task title",
            |_, _| Err(io::Error::new(io::ErrorKind::NotFound, "missing")),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Linux clipboard copy requires wl-copy or xclip"
        );
    }

    #[test]
    fn reports_linux_clipboard_command_failures() {
        use std::collections::VecDeque;

        let mut responses = VecDeque::from([
            Ok(clipboard_test_output(
                false,
                b"",
                b"cannot connect to wayland display",
            )),
            Ok(clipboard_test_output(false, b"", b"cannot open display")),
        ]);

        let error = copy_linux_clipboard_with(
            [LinuxClipboardBackend::Wayland, LinuxClipboardBackend::X11],
            "task title",
            |_, _| responses.pop_front().unwrap(),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "wl-copy exited with exit status: 1: cannot connect to wayland display; \
                 xclip exited with exit status: 1: cannot open display"
        );
    }

    fn clipboard_test_output(
        success: bool,
        stdout: &[u8],
        stderr: &[u8],
    ) -> ClipboardCommandOutput {
        ClipboardCommandOutput {
            success,
            status: if success {
                "exit status: 0".to_string()
            } else {
                "exit status: 1".to_string()
            },
            stdout: stdout.to_vec(),
            stderr: stderr.to_vec(),
        }
    }
}
