use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use tracing_subscriber::EnvFilter;

const APP_DIR: &str = "atm";
const LOG_FILE: &str = "atm.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LogMode {
    Cli,
    Tui,
    Daemon,
    Server,
}

pub(crate) fn init(_mode: LogMode) -> Result<()> {
    let filter = std::env::var("ATM_LOG").unwrap_or_else(|_| "atm=info".to_string());
    let filter = EnvFilter::try_new(filter).context("invalid ATM_LOG filter")?;
    let path = std::env::var_os("ATM_LOG_FILE")
        .map(PathBuf::from)
        .unwrap_or(default_log_path()?);
    init_file(&path, filter)
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
