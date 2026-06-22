#[cfg(not(test))]
use std::fs;
#[cfg(not(test))]
use std::io;
use std::process::Command;
#[cfg(not(test))]
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
#[cfg(not(test))]
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};

pub(crate) fn is_editor_prefix_key(key: KeyEvent) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('x')
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
fn external_editor_command(path: &std::path::Path) -> Command {
    let mut command = Command::new("sh");
    command
        .arg("-c")
        .arg("exec ${VISUAL:-${EDITOR:-vi}} \"$1\"")
        .arg("sh")
        .arg(path);
    command
}

#[cfg(not(test))]
fn suspend_terminal() -> Result<impl FnOnce() -> Result<()>> {
    disable_raw_mode()?;
    crossterm::execute!(io::stdout(), LeaveAlternateScreen)?;
    Ok(|| {
        crossterm::execute!(io::stdout(), EnterAlternateScreen)?;
        enable_raw_mode()?;
        Ok(())
    })
}

pub(crate) fn copy_to_clipboard(value: &str) -> Result<()> {
    let mut child = Command::new("pbcopy")
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
