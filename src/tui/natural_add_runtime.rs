use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::Result;

#[cfg(not(test))]
pub(crate) fn spawn_add_task_only_natural(
    input: &str,
    workspace_id: &str,
    db_path: Option<&Path>,
    project: Option<&str>,
) -> Result<()> {
    let exe = std::env::current_exe()?;
    let cwd = std::env::current_dir()?;
    let log_path = task_intake_log_path();
    let stderr = open_spawn_log(&log_path)?;
    let stdout = stderr.try_clone()?;
    let mut command = std::process::Command::new(exe);
    let Some(db_path) = db_path else {
        anyhow::bail!("internal natural add requires a database path");
    };
    command
        .arg("--db")
        .arg(db_path)
        .arg("internal")
        .arg("natural-add")
        .arg("--workspace-id")
        .arg(workspace_id)
        .arg("--input")
        .arg(input);
    if let Some(project) = project {
        command.arg("--project").arg(project);
    }
    command
        .current_dir(cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(stdout))
        .stderr(std::process::Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    if let Some(db) = std::env::var_os("AVEN_DB") {
        command.env("AVEN_DB", db);
    }
    if let Some(log_file) = std::env::var_os("AVEN_LOG_FILE") {
        command.env("AVEN_LOG_FILE", log_file);
    }
    if let Some(log_filter) = std::env::var_os("AVEN_LOG") {
        command.env("AVEN_LOG", log_filter);
    }
    let child = command.spawn()?;
    tracing::info!(
        pid = child.id(),
        workspace_id = %workspace_id,
        "spawned background natural add worker"
    );
    Ok(())
}

#[cfg(test)]
pub(crate) fn spawn_add_task_only_natural(
    _input: &str,
    _workspace_id: &str,
    _db_path: Option<&Path>,
    _project: Option<&str>,
) -> Result<()> {
    Ok(())
}

#[cfg(not(test))]
fn open_spawn_log(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?)
}

pub(crate) fn task_intake_log_path() -> PathBuf {
    task_intake_log_path_from_env(
        std::env::var_os("AVEN_LOG_FILE"),
        std::env::var_os("XDG_STATE_HOME"),
        dirs::home_dir(),
    )
}

fn task_intake_log_path_from_env(
    log_file: Option<OsString>,
    xdg_state_home: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    log_file
        .map(PathBuf::from)
        .unwrap_or_else(|| default_log_path_display_from_env(xdg_state_home, home_dir))
}

fn default_log_path_display_from_env(
    xdg_state_home: Option<OsString>,
    home_dir: Option<PathBuf>,
) -> PathBuf {
    let mut dir = xdg_state_home
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home_dir.map(|home| home.join(".local/state")))
        .unwrap_or_else(|| PathBuf::from("~/.local/state"));
    dir.push("aven");
    dir.join("aven.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_intake_log_path_prefers_aven_log_file() {
        let log = PathBuf::from("natural.log");

        assert_eq!(
            task_intake_log_path_from_env(Some(log.clone().into_os_string()), None, None),
            log
        );
    }

    #[test]
    fn task_intake_log_path_uses_absolute_xdg_state_home() {
        let state = std::env::temp_dir().join("aven-state");

        assert_eq!(
            task_intake_log_path_from_env(
                None,
                Some(state.clone().into_os_string()),
                Some(PathBuf::from("ignored-home")),
            ),
            state.join("aven").join("aven.log")
        );
    }

    #[test]
    fn task_intake_log_path_ignores_relative_xdg_state_home_and_uses_home() {
        let home = std::env::temp_dir().join("aven-home");

        assert_eq!(
            task_intake_log_path_from_env(
                None,
                Some(OsString::from("relative-state")),
                Some(home.clone()),
            ),
            home.join(".local/state").join("aven").join("aven.log")
        );
    }
}
