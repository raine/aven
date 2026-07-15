#[cfg(not(test))]
use std::fs;
use std::io::{self, Write};
#[cfg(not(test))]
use std::process::Command as ProcessCommand;
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

#[cfg(not(test))]
pub(crate) fn copy_to_clipboard(value: &str) -> Result<()> {
    let mut child = ProcessCommand::new("pbcopy")
        .stdin(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        stdin.write_all(value.as_bytes())?;
    }
    let status = child.wait()?;
    if !status.success() {
        anyhow::bail!("pbcopy exited with {status}");
    }
    Ok(())
}

#[cfg(test)]
pub(crate) fn copy_to_clipboard(_value: &str) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
