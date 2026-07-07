use std::fs::{self, OpenOptions};
use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::Once;

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

const APP_DIR: &str = "aven";
const LOG_FILE: &str = "aven.log";
static PANIC_HOOK: Once = Once::new();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogMode {
    Cli,
    Tui,
    Daemon,
    Server,
}

pub(crate) fn init(_mode: LogMode) -> Result<()> {
    let filter = std::env::var("AVEN_LOG").unwrap_or_else(|_| "aven=info".to_string());
    let filter = EnvFilter::try_new(filter).context("invalid AVEN_LOG filter")?;
    let path = std::env::var_os("AVEN_LOG_FILE")
        .map(PathBuf::from)
        .unwrap_or(default_log_path()?);
    init_file(&path, filter)?;
    install_panic_hook();
    Ok(())
}

fn default_log_path() -> Result<PathBuf> {
    let mut dir = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
        .context("could not find state directory")?;
    dir.push(APP_DIR);
    Ok(dir.join(LOG_FILE))
}

fn init_file(path: &Path, filter: EnvFilter) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create log directory {}", parent.display()))?;
    }
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open log file {}", path.display()))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .compact()
        .with_writer(std::sync::Mutex::new(file))
        .try_init()
        .map_err(|err| anyhow::anyhow!("initialize tracing subscriber: {err}"))
}

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let default_hook = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let location = info
                .location()
                .map(format_panic_location)
                .unwrap_or_else(|| "unknown".to_string());
            let message = panic_message(info);
            let backtrace = std::backtrace::Backtrace::force_capture();
            tracing::error!(
                panic.location = %location,
                panic.message = %message,
                panic.backtrace = %backtrace,
                "process panicked"
            );
            default_hook(info);
        }));
    });
}

fn format_panic_location(location: &panic::Location<'_>) -> String {
    format!(
        "{}:{}:{}",
        location.file(),
        location.line(),
        location.column()
    )
}

fn panic_message(info: &PanicHookInfo<'_>) -> String {
    info.payload()
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| info.payload().downcast_ref::<String>().map(String::as_str))
        .unwrap_or("unknown panic payload")
        .to_string()
}

#[cfg(test)]
mod tests {
    use std::panic::Location;

    use super::format_panic_location;

    #[test]
    fn formats_panic_location_with_line_and_column() {
        let location = Location::caller();

        assert_eq!(
            format_panic_location(location),
            format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            )
        );
    }
}
