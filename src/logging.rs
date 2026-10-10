use std::panic::{self, PanicHookInfo};
use std::path::{Path, PathBuf};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

const APP_DIR: &str = "aven";
const LOG_FILE: &str = "aven.log";
static PANIC_HOOK: Once = Once::new();
static LOGS_TO_STDERR: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogMode {
    Cli,
    Tui,
    Daemon,
    Server,
}

pub(crate) fn init(mode: LogMode) -> Result<()> {
    LOGS_TO_STDERR.store(mode.uses_stderr(), Ordering::Relaxed);
    let filter = std::env::var("AVEN_LOG").unwrap_or_else(|_| "aven=info".to_string());
    let filter = EnvFilter::try_new(filter).context("invalid AVEN_LOG filter")?;
    if mode.uses_stderr() {
        init_stderr(filter)?;
    } else {
        let _ = init_file_logging(filter);
    }
    install_panic_hook();
    Ok(())
}

impl LogMode {
    fn uses_stderr(self) -> bool {
        matches!(self, Self::Daemon | Self::Server)
    }
}

pub(crate) fn record_command_error(error: &anyhow::Error) {
    // Long-running modes already write tracing events to stderr. Their final
    // error is rendered once by main rather than duplicated as a log record.
    if !LOGS_TO_STDERR.load(Ordering::Relaxed) {
        tracing::error!(error = %format_args!("{error:#}"), "command failed");
    }
}

fn init_file_logging(filter: EnvFilter) -> Result<()> {
    let path = log_path_from(
        std::env::var_os("AVEN_LOG_FILE").map(PathBuf::from),
        std::env::var_os("XDG_STATE_HOME").map(PathBuf::from),
        dirs::home_dir,
    )?;
    init_file(&path, filter)
}

pub(crate) fn log_path_from(
    log_file: Option<PathBuf>,
    state_home: Option<PathBuf>,
    home_dir: impl FnOnce() -> Option<PathBuf>,
) -> Result<PathBuf> {
    if let Some(path) = log_file {
        return Ok(path);
    }
    let dir = state_home
        .filter(|path| path.is_absolute())
        .or_else(|| home_dir().map(|home| home.join(".local/state")))
        .context("could not find state directory")?;
    Ok(dir.join(APP_DIR).join(LOG_FILE))
}

fn init_stderr(filter: EnvFilter) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_ansi(false)
        .with_target(true)
        .compact()
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|err| anyhow::anyhow!("initialize tracing subscriber: {err}"))
}

fn init_file(path: &Path, filter: EnvFilter) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        aven_core::private_fs::create_dir_all(parent)
            .with_context(|| format!("create log directory {}", parent.display()))?;
    }
    let file = aven_core::private_fs::open_append_file(path)
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

    use super::{LogMode, format_panic_location};

    #[test]
    fn missing_home_and_state_only_disable_default_logging() {
        assert!(super::log_path_from(None, None, || None).is_err());
        let explicit = std::path::PathBuf::from("custom.log");
        assert_eq!(
            super::log_path_from(Some(explicit.clone()), None, || {
                panic!("explicit log path must not resolve the home directory")
            })
            .unwrap(),
            explicit
        );
        assert_eq!(
            super::log_path_from(None, Some("/state".into()), || None).unwrap(),
            std::path::PathBuf::from("/state/aven/aven.log")
        );
    }

    #[test]
    fn long_running_modes_use_stderr() {
        assert!(LogMode::Server.uses_stderr());
        assert!(LogMode::Daemon.uses_stderr());
        assert!(!LogMode::Cli.uses_stderr());
        assert!(!LogMode::Tui.uses_stderr());
    }

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
