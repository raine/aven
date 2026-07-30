#[cfg(any(not(test), target_os = "macos"))]
use std::fs;
use std::io::{self, Write};
#[cfg(all(target_os = "linux", not(test)))]
use std::io::{Read, Seek};
use std::path::Path;
use std::process::Command as ProcessCommand;
#[cfg(any(target_os = "linux", test))]
use std::process::Output;
use std::sync::Mutex;
#[cfg(not(test))]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use crossterm::Command;
use crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use crossterm::terminal::supports_keyboard_enhancement;

#[derive(Debug)]
pub(crate) struct ClipboardImage {
    pub(crate) filename: String,
    pub(crate) bytes: Vec<u8>,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinuxClipboardBackend {
    Wayland,
    X11,
}

#[cfg(any(target_os = "linux", test))]
#[derive(Clone, Debug, Eq, PartialEq)]
struct ClipboardCommand {
    program: &'static str,
    args: Vec<std::ffi::OsString>,
}

#[cfg(any(target_os = "linux", test))]
struct ClipboardCommandOutput {
    success: bool,
    status: String,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
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
struct ClipboardImageFormat {
    mime: &'static str,
    extension: &'static str,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperatingSystem {
    Macos,
    Linux,
    Windows,
}

#[derive(Debug, Eq, PartialEq)]
struct ViewerCommand {
    program: &'static str,
    args: Vec<std::ffi::OsString>,
}

#[cfg_attr(test, allow(dead_code))]
pub(crate) fn open_image_in_default_viewer(path: &Path) -> Result<()> {
    let os = current_operating_system();
    spawn_viewer(
        default_image_viewer_command(os, path),
        "could not start the default image viewer",
    )
}

#[cfg(not(test))]
pub(crate) fn open_url_in_default_browser(url: &str) -> Result<()> {
    validate_browser_url(url)?;
    spawn_viewer(
        default_browser_command(current_operating_system(), url),
        "could not start the default browser",
    )
}

fn validate_browser_url(url: &str) -> Result<()> {
    anyhow::ensure!(
        url.starts_with("https://") || url.starts_with("http://"),
        "browser URL must use http or https"
    );
    Ok(())
}

fn current_operating_system() -> OperatingSystem {
    if cfg!(target_os = "macos") {
        OperatingSystem::Macos
    } else if cfg!(target_os = "windows") {
        OperatingSystem::Windows
    } else {
        OperatingSystem::Linux
    }
}

fn spawn_viewer(spec: ViewerCommand, context: &'static str) -> Result<()> {
    let mut child = ProcessCommand::new(spec.program)
        .args(spec.args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .context(context)?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

fn default_image_viewer_command(os: OperatingSystem, path: &Path) -> ViewerCommand {
    let path = path.as_os_str().to_owned();
    match os {
        OperatingSystem::Macos => ViewerCommand {
            program: "open",
            args: vec![path],
        },
        OperatingSystem::Linux => ViewerCommand {
            program: "xdg-open",
            args: vec![path],
        },
        OperatingSystem::Windows => ViewerCommand {
            program: "rundll32.exe",
            args: vec!["url.dll,FileProtocolHandler".into(), path],
        },
    }
}

fn default_browser_command(os: OperatingSystem, url: &str) -> ViewerCommand {
    let url = std::ffi::OsString::from(url);
    match os {
        OperatingSystem::Macos => ViewerCommand {
            program: "open",
            args: vec![url],
        },
        OperatingSystem::Linux => ViewerCommand {
            program: "xdg-open",
            args: vec![url],
        },
        OperatingSystem::Windows => ViewerCommand {
            program: "rundll32.exe",
            args: vec!["url.dll,FileProtocolHandler".into(), url],
        },
    }
}

#[cfg(not(test))]
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub(crate) fn is_editor_prefix_key(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x')
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeyboardEnhancementMode {
    Kitty,
    ModifyOtherKeys,
}

#[derive(Default)]
struct KeyboardEnhancementState {
    mode: Option<KeyboardEnhancementMode>,
}

impl KeyboardEnhancementState {
    fn enable(&mut self, mode: KeyboardEnhancementMode, writer: &mut impl Write) -> io::Result<()> {
        match mode {
            KeyboardEnhancementMode::Kitty => crossterm::execute!(
                writer,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )?,
            KeyboardEnhancementMode::ModifyOtherKeys => {
                crossterm::execute!(writer, SetModifyOtherKeys(true))?
            }
        }
        self.mode = Some(mode);
        Ok(())
    }

    fn disable(&mut self, writer: &mut impl Write) -> io::Result<()> {
        let Some(mode) = self.mode else {
            return Ok(());
        };
        match mode {
            KeyboardEnhancementMode::Kitty => {
                crossterm::execute!(writer, PopKeyboardEnhancementFlags)?
            }
            KeyboardEnhancementMode::ModifyOtherKeys => {
                crossterm::execute!(writer, SetModifyOtherKeys(false))?
            }
        }
        self.mode = None;
        Ok(())
    }
}

struct SetModifyOtherKeys(bool);

impl Command for SetModifyOtherKeys {
    fn write_ansi(&self, writer: &mut impl std::fmt::Write) -> std::fmt::Result {
        writer.write_str(if self.0 { "\x1b[>4;2m" } else { "\x1b[>4m" })
    }
}

static KEYBOARD_ENHANCEMENT: Mutex<KeyboardEnhancementState> =
    Mutex::new(KeyboardEnhancementState { mode: None });

fn keyboard_enhancement() -> io::Result<std::sync::MutexGuard<'static, KeyboardEnhancementState>> {
    KEYBOARD_ENHANCEMENT
        .lock()
        .map_err(|_| io::Error::other("keyboard enhancement state lock is poisoned"))
}

fn detected_keyboard_enhancement() -> Option<KeyboardEnhancementMode> {
    if matches!(supports_keyboard_enhancement(), Ok(true)) {
        Some(KeyboardEnhancementMode::Kitty)
    } else if cfg!(unix) {
        Some(KeyboardEnhancementMode::ModifyOtherKeys)
    } else {
        None
    }
}

pub(crate) struct KeyboardEnhancementGuard {
    active: bool,
}

impl KeyboardEnhancementGuard {
    pub(crate) fn enable() -> Result<Self> {
        let Some(mode) = detected_keyboard_enhancement() else {
            return Ok(Self { active: false });
        };
        keyboard_enhancement()?
            .enable(mode, &mut io::stdout())
            .context("enable terminal keyboard enhancements")?;
        Ok(Self { active: true })
    }

    pub(crate) fn disable(&mut self) -> Result<()> {
        if self.active {
            keyboard_enhancement()?
                .disable(&mut io::stdout())
                .context("disable terminal keyboard enhancements")?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for KeyboardEnhancementGuard {
    fn drop(&mut self) {
        let _ = self.disable();
    }
}

#[cfg(not(test))]
pub(crate) fn edit_text_externally(value: String, filename: &str) -> Result<String> {
    let path = temp_editor_path(filename)?;
    fs::write(&path, value)?;
    let result =
        run_external_editor(&path).and_then(|()| fs::read_to_string(&path).map_err(Into::into));
    let _ = fs::remove_file(&path);
    if let Some(parent) = path.parent() {
        let _ = fs::remove_dir(parent);
    }
    result
}

#[cfg(test)]
pub(crate) fn edit_text_externally(value: String, _filename: &str) -> Result<String> {
    Ok(format!("{value} from editor"))
}

#[cfg(not(test))]
fn temp_editor_path(filename: &str) -> io::Result<std::path::PathBuf> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("aven-tui-editor-{pid}-{millis}"));
    fs::create_dir(&dir)?;
    Ok(dir.join(filename))
}

#[cfg(not(test))]
fn run_external_editor(path: &std::path::Path) -> Result<()> {
    let restore = suspend_terminal()?;
    let status = external_editor_command(path).status();
    restore()?;
    let status = status?;
    if !status.success() {
        anyhow::bail!("editor exited with {status}");
    }
    Ok(())
}

#[cfg(not(test))]
fn external_editor_command(path: &std::path::Path) -> ProcessCommand {
    let mut command = ProcessCommand::new("sh");
    command
        .arg("-c")
        .arg("exec ${VISUAL:-${EDITOR:-vi}} \"$1\"")
        .arg("sh")
        .arg(path);
    command
}

#[cfg(not(test))]
fn suspend_terminal() -> Result<impl FnOnce() -> Result<()>> {
    let keyboard_mode = {
        let mut state = keyboard_enhancement()?;
        let mode = state.mode;
        state
            .disable(&mut io::stdout())
            .context("suspend terminal keyboard enhancements")?;
        mode
    };
    disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(move || {
        crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
        enable_raw_mode()?;
        if let Some(mode) = keyboard_mode {
            keyboard_enhancement()?
                .enable(mode, &mut io::stdout())
                .context("resume terminal keyboard enhancements")?;
        }
        Ok(())
    })
}

#[cfg(any(target_os = "linux", test))]
fn linux_clipboard_backend_order(
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
fn clipboard_list_command(backend: LinuxClipboardBackend) -> ClipboardCommand {
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
fn clipboard_read_command(
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
fn clipboard_write_command(backend: LinuxClipboardBackend) -> ClipboardCommand {
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
fn advertised_clipboard_image_format(output: &[u8]) -> Option<ClipboardImageFormat> {
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
fn read_linux_clipboard_image_with(
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
fn copy_linux_clipboard_with(
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

#[cfg(target_os = "macos")]
pub(crate) fn read_clipboard_text() -> Result<Option<String>> {
    let output = ProcessCommand::new("pbpaste").output()?;
    if !output.status.success() {
        anyhow::bail!("pbpaste exited with {}", output.status);
    }
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    Ok((!text.trim().is_empty()).then_some(text))
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn read_clipboard_text() -> Result<Option<String>> {
    Ok(None)
}

#[cfg(all(not(test), target_os = "macos"))]
pub(crate) fn copy_to_clipboard(value: &str) -> Result<()> {
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
    copy_linux_clipboard_with(backends, value, run_clipboard_write_command)
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
    static TEST_BROWSER_URL: std::cell::RefCell<Option<String>> = const {
        std::cell::RefCell::new(None)
    };
}

#[cfg(test)]
pub(crate) fn open_url_in_default_browser(url: &str) -> Result<()> {
    validate_browser_url(url)?;
    TEST_BROWSER_URL.with(|opened| opened.replace(Some(url.to_string())));
    Ok(())
}

#[cfg(test)]
pub(crate) fn browser_url_for_test() -> Option<String> {
    TEST_BROWSER_URL.with(|opened| opened.borrow().clone())
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
    fn selects_platform_default_image_viewer_commands() {
        let path = Path::new("/tmp/attachment image.png");
        assert_eq!(
            default_image_viewer_command(OperatingSystem::Macos, path),
            ViewerCommand {
                program: "open",
                args: vec![path.as_os_str().to_owned()],
            }
        );
        assert_eq!(
            default_image_viewer_command(OperatingSystem::Linux, path),
            ViewerCommand {
                program: "xdg-open",
                args: vec![path.as_os_str().to_owned()],
            }
        );
        assert_eq!(
            default_image_viewer_command(OperatingSystem::Windows, path),
            ViewerCommand {
                program: "rundll32.exe",
                args: vec![
                    "url.dll,FileProtocolHandler".into(),
                    path.as_os_str().to_owned(),
                ],
            }
        );
    }

    #[test]
    fn selects_platform_default_browser_commands() {
        let url = "https://aven.raine.dev/recurring-tasks/";
        assert_eq!(
            default_browser_command(OperatingSystem::Macos, url),
            ViewerCommand {
                program: "open",
                args: vec![url.into()],
            }
        );
        assert_eq!(
            default_browser_command(OperatingSystem::Linux, url),
            ViewerCommand {
                program: "xdg-open",
                args: vec![url.into()],
            }
        );
        assert_eq!(
            default_browser_command(OperatingSystem::Windows, url),
            ViewerCommand {
                program: "rundll32.exe",
                args: vec!["url.dll,FileProtocolHandler".into(), url.into()],
            }
        );
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

    #[test]
    fn kitty_keyboard_enhancement_pushes_and_pops_state() {
        let mut state = KeyboardEnhancementState::default();
        let mut output = Vec::new();

        state
            .enable(KeyboardEnhancementMode::Kitty, &mut output)
            .unwrap();
        assert_eq!(state.mode, Some(KeyboardEnhancementMode::Kitty));
        state.disable(&mut output).unwrap();

        assert_eq!(output, b"\x1b[>1u\x1b[<1u");
        assert_eq!(state.mode, None);
    }

    #[test]
    fn failed_restore_keeps_keyboard_state_available_for_retry() {
        struct FailingWriter;

        impl Write for FailingWriter {
            fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
                Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed"))
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let mut state = KeyboardEnhancementState::default();
        let mut output = Vec::new();
        state
            .enable(KeyboardEnhancementMode::Kitty, &mut output)
            .unwrap();

        assert!(state.disable(&mut FailingWriter).is_err());
        assert_eq!(state.mode, Some(KeyboardEnhancementMode::Kitty));
        state.disable(&mut output).unwrap();

        assert_eq!(output, b"\x1b[>1u\x1b[<1u");
        assert_eq!(state.mode, None);
    }

    #[test]
    fn modify_other_keys_enhancement_restores_terminal_mode() {
        let mut state = KeyboardEnhancementState::default();
        let mut output = Vec::new();

        state
            .enable(KeyboardEnhancementMode::ModifyOtherKeys, &mut output)
            .unwrap();
        assert_eq!(state.mode, Some(KeyboardEnhancementMode::ModifyOtherKeys));
        state.disable(&mut output).unwrap();
        state.disable(&mut output).unwrap();

        assert_eq!(output, b"\x1b[>4;2m\x1b[>4m");
        assert_eq!(state.mode, None);
    }
}
