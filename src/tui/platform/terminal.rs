use std::io::{self, Write};
use std::sync::Mutex;

use anyhow::{Context, Result};
use crossterm::Command;
use crossterm::event::{
    KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::terminal::supports_keyboard_enhancement;
#[cfg(not(test))]
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

use crate::tui::theme::Background;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum KeyboardEnhancementMode {
    Kitty,
    ModifyOtherKeys,
}

#[derive(Default)]
pub(super) struct KeyboardEnhancementState {
    pub(super) mode: Option<KeyboardEnhancementMode>,
}

impl KeyboardEnhancementState {
    pub(super) fn enable(
        &mut self,
        mode: KeyboardEnhancementMode,
        writer: &mut impl Write,
    ) -> io::Result<()> {
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

    pub(super) fn disable(&mut self, writer: &mut impl Write) -> io::Result<()> {
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

/// Asks the terminal for its colors with OSC 10 and OSC 11. When the terminal
/// does not answer, falls back to `COLORFGBG`, then to a dark background.
///
/// Call this before the TUI starts to read input, so the reply cannot reach
/// the event stream as key presses.
pub(crate) fn detect_background() -> Background {
    use terminal_colorsaurus::{QueryOptions, ThemeMode, theme_mode};

    match theme_mode(QueryOptions::default()) {
        Ok(ThemeMode::Light) => Background::Light,
        Ok(ThemeMode::Dark) => Background::Dark,
        Err(_) => std::env::var("COLORFGBG")
            .ok()
            .and_then(|value| colorfgbg_background(&value))
            .unwrap_or_default(),
    }
}

/// `COLORFGBG` holds `fg;bg` (sometimes `fg;default;bg`) as ANSI color
/// indexes. Index 7 and the bright colors except 8 are light backgrounds.
fn colorfgbg_background(value: &str) -> Option<Background> {
    let background: u8 = value.rsplit(';').next()?.parse().ok()?;
    Some(match background {
        7 | 9..=15 => Background::Light,
        _ => Background::Dark,
    })
}

pub(crate) trait TerminalTransition {
    fn suspend(&mut self) -> Result<()>;
    fn restore(&mut self) -> Result<()>;
}

pub(crate) struct SuspendedTerminal<'a, T: TerminalTransition> {
    transition: &'a mut T,
    restore_attempted: bool,
}

impl<'a, T: TerminalTransition> SuspendedTerminal<'a, T> {
    pub(crate) fn suspend(transition: &'a mut T) -> Result<Self> {
        if let Err(error) = transition.suspend() {
            let restore_error = transition.restore().err();
            return match restore_error {
                Some(restore_error) => Err(error.context(format!(
                    "terminal restoration after suspension failure also failed: {restore_error:#}"
                ))),
                None => Err(error),
            };
        }
        Ok(Self {
            transition,
            restore_attempted: false,
        })
    }

    pub(crate) fn restore(&mut self) -> Result<()> {
        if self.restore_attempted {
            return Ok(());
        }
        self.restore_attempted = true;
        self.transition.restore()
    }
}

impl<T: TerminalTransition> Drop for SuspendedTerminal<'_, T> {
    fn drop(&mut self) {
        if !self.restore_attempted {
            self.restore_attempted = true;
            let _ = self.transition.restore();
        }
    }
}

pub(super) fn run_while_terminal_suspended<T, F, R>(transition: &mut T, operation: F) -> Result<R>
where
    T: TerminalTransition,
    F: FnOnce() -> Result<R>,
{
    let mut suspended = SuspendedTerminal::suspend(transition)?;
    let result = operation();
    suspended.restore()?;
    result
}

pub(crate) struct SystemTerminalTransition {
    keyboard_mode: Option<KeyboardEnhancementMode>,
    mouse_capture: bool,
    bracketed_paste: bool,
}

impl SystemTerminalTransition {
    pub(crate) fn new(mouse_capture: bool) -> Self {
        Self {
            keyboard_mode: None,
            mouse_capture,
            bracketed_paste: true,
        }
    }
}

#[cfg(not(test))]
impl TerminalTransition for SystemTerminalTransition {
    fn suspend(&mut self) -> Result<()> {
        use crossterm::cursor::Show;
        use crossterm::event::{DisableBracketedPaste, DisableMouseCapture};

        self.keyboard_mode = {
            let mut state = keyboard_enhancement()?;
            let mode = state.mode;
            state
                .disable(&mut io::stdout())
                .context("suspend terminal keyboard enhancements")?;
            mode
        };
        let mut stdout = io::stdout();
        if self.bracketed_paste {
            crossterm::execute!(stdout, DisableBracketedPaste)?;
        }
        if self.mouse_capture {
            crossterm::execute!(stdout, DisableMouseCapture)?;
        }
        crossterm::execute!(stdout, Show, LeaveAlternateScreen)?;
        stdout.flush()?;
        disable_raw_mode()?;
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        use crossterm::cursor::Hide;
        use crossterm::event::{EnableBracketedPaste, EnableMouseCapture};

        crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
        enable_raw_mode()?;
        if let Some(mode) = self.keyboard_mode.take() {
            keyboard_enhancement()?
                .enable(mode, &mut io::stdout())
                .context("resume terminal keyboard enhancements")?;
        }
        let mut stdout = io::stdout();
        if self.bracketed_paste {
            crossterm::execute!(stdout, EnableBracketedPaste)?;
        }
        if self.mouse_capture {
            crossterm::execute!(stdout, EnableMouseCapture)?;
        }
        crossterm::execute!(stdout, Hide)?;
        stdout.flush()?;
        Ok(())
    }
}

#[cfg(test)]
impl TerminalTransition for SystemTerminalTransition {
    fn suspend(&mut self) -> Result<()> {
        let _ = (self.keyboard_mode, self.mouse_capture, self.bracketed_paste);
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use anyhow::Result;

    use super::*;

    #[derive(Default)]
    struct FakeEditorTransition {
        suspended: usize,
        restored: usize,
    }

    impl TerminalTransition for FakeEditorTransition {
        fn suspend(&mut self) -> Result<()> {
            self.suspended += 1;
            Ok(())
        }

        fn restore(&mut self) -> Result<()> {
            self.restored += 1;
            Ok(())
        }
    }

    #[test]
    fn external_editor_operation_restores_terminal_after_success_and_failure() {
        for result in [Ok("edited"), Err(anyhow::anyhow!("editor failed"))] {
            let mut transition = FakeEditorTransition::default();
            let failed = result.is_err();
            let actual = run_while_terminal_suspended(&mut transition, || result);

            assert_eq!(actual.is_err(), failed);
            assert_eq!(transition.suspended, 1);
            assert_eq!(transition.restored, 1);
        }
    }

    #[test]
    fn colorfgbg_uses_the_last_field_as_the_background() {
        assert_eq!(colorfgbg_background("0;15"), Some(Background::Light));
        assert_eq!(colorfgbg_background("0;default;7"), Some(Background::Light));
        assert_eq!(colorfgbg_background("15;0"), Some(Background::Dark));
        assert_eq!(colorfgbg_background("7;8"), Some(Background::Dark));
        assert_eq!(colorfgbg_background("15;default"), None);
        assert_eq!(colorfgbg_background(""), None);
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
