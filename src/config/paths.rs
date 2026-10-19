use std::env;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::AppConfig;

const APP_DIR: &str = "aven";

pub fn expand_tilde(path: &Path) -> Result<PathBuf> {
    expand_tilde_from(path, dirs::home_dir().as_deref())
}

pub(crate) fn expand_tilde_from(path: &Path, home: Option<&Path>) -> Result<PathBuf> {
    let mut components = path.components();
    if !matches!(components.next(), Some(Component::Normal(component)) if component == "~") {
        return Ok(path.to_path_buf());
    }
    let home = home.context("could not find home directory")?;
    Ok(home.join(components.as_path()))
}

pub fn config_dir_path() -> Result<PathBuf> {
    if let Ok(path) = env::var("AVEN_CONFIG_DIR") {
        return Ok(PathBuf::from(path));
    }
    let home = dirs::home_dir().context("could not find home directory")?;
    Ok(home.join(".config").join(APP_DIR))
}

pub fn config_file_path() -> Result<PathBuf> {
    let mut path = config_dir_path()?;
    path.push("config.yaml");
    Ok(path)
}

fn state_dir_path() -> Result<PathBuf> {
    let mut dir = env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| dirs::home_dir().map(|home| home.join(".local/state")))
        .context("could not find state directory")?;
    dir.push(APP_DIR);
    Ok(dir)
}

pub fn default_db_path() -> Result<PathBuf> {
    Ok(state_dir_path()?.join("db.sqlite"))
}

/// Sync server storage: the first `$STATE_DIRECTORY` entry when systemd
/// provides one, otherwise a `server` directory under the user state directory.
pub fn default_server_data_path() -> Result<PathBuf> {
    default_server_data_path_from(env::var_os("STATE_DIRECTORY"), state_dir_path)
}

fn default_server_data_path_from(
    systemd_state: Option<std::ffi::OsString>,
    user_state: impl FnOnce() -> Result<PathBuf>,
) -> Result<PathBuf> {
    let systemd_dir = systemd_state
        .as_deref()
        .and_then(|dirs| env::split_paths(dirs).next())
        .filter(|path| path.is_absolute());
    let dir = match systemd_dir {
        Some(dir) => dir,
        None => user_state()?.join("server"),
    };
    Ok(dir.join("sync-server.sqlite"))
}

pub fn resolve_db_path(flag: Option<PathBuf>, config: &AppConfig) -> Result<PathBuf> {
    resolve_db_path_from(
        flag,
        env::var_os("AVEN_DB").map(PathBuf::from),
        debug_db_path_from_env(),
        config,
        cfg!(debug_assertions),
    )
}

pub(super) fn resolve_db_path_from(
    flag: Option<PathBuf>,
    env_db: Option<PathBuf>,
    dev_db: Option<PathBuf>,
    config: &AppConfig,
    debug_build: bool,
) -> Result<PathBuf> {
    if let Some(path) = flag {
        return Ok(path);
    }
    if debug_build && let Some(path) = dev_db {
        return Ok(path);
    }
    if let Some(path) = env_db {
        return Ok(path);
    }
    if let Some(path) = &config.local.db_path {
        return expand_tilde(path);
    }
    if debug_build {
        bail!("error debug-database-required hint=\"set AVEN_DEV_DB, set AVEN_DB, or pass --db\"");
    }
    default_db_path()
}

pub fn debug_db_path_from_env() -> Option<PathBuf> {
    if cfg!(debug_assertions) {
        env::var_os("AVEN_DEV_DB").map(PathBuf::from)
    } else {
        None
    }
}

#[allow(dead_code)]
pub fn resolve_blob_dir(db_path: &Path, config: &AppConfig) -> Result<PathBuf> {
    let base = db_path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match &config.local.blob_dir {
        Some(path) if path.is_absolute() => Ok(path.clone()),
        Some(path) => Ok(base.join(path)),
        None => {
            let mut blob_dir = db_path.as_os_str().to_os_string();
            blob_dir.push(".blobs");
            Ok(PathBuf::from(blob_dir))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_data_prefers_systemd_state_directory() {
        let user = || Ok(PathBuf::from("/home/u/.local/state/aven"));
        assert_eq!(
            default_server_data_path_from(Some("/var/lib/aven:/var/lib/other".into()), user)
                .unwrap(),
            PathBuf::from("/var/lib/aven/sync-server.sqlite")
        );
        assert_eq!(
            default_server_data_path_from(Some("relative".into()), user).unwrap(),
            PathBuf::from("/home/u/.local/state/aven/server/sync-server.sqlite")
        );
        assert_eq!(
            default_server_data_path_from(None, user).unwrap(),
            PathBuf::from("/home/u/.local/state/aven/server/sync-server.sqlite")
        );
    }

    #[test]
    fn tilde_paths_expand_from_home() {
        let home = dirs::home_dir().expect("home directory");

        assert_eq!(
            expand_tilde(Path::new("~/work")).unwrap(),
            home.join("work")
        );
        assert_eq!(
            expand_tilde(Path::new("~someone/work")).unwrap(),
            PathBuf::from("~someone/work")
        );
        assert_eq!(
            expand_tilde(Path::new("relative/work")).unwrap(),
            PathBuf::from("relative/work")
        );
    }

    #[test]
    fn debug_database_resolution_requires_an_explicit_path() {
        let config = AppConfig::default();
        let error = resolve_db_path_from(None, None, None, &config, true).unwrap_err();

        assert!(format!("{error:#}").contains("debug-database-required"));
    }

    #[test]
    fn database_resolution_preserves_build_specific_precedence() {
        let dev = "/tmp/aven-dev.sqlite";
        let env = "/tmp/aven-env.sqlite";
        let flag = "/tmp/aven-flag.sqlite";
        for (flag, env, dev, debug, expected) in [
            (None, None, Some(dev), true, dev),
            (None, Some(env), Some(dev), true, dev),
            (Some(flag), None, Some(dev), true, flag),
            (None, None, Some(dev), false, "/tmp/configured.sqlite"),
        ] {
            let mut config = AppConfig::default();
            if !debug {
                config.local.db_path = Some(PathBuf::from("/tmp/configured.sqlite"));
            }
            assert_eq!(
                resolve_db_path_from(
                    flag.map(PathBuf::from),
                    env.map(PathBuf::from),
                    dev.map(PathBuf::from),
                    &config,
                    debug,
                )
                .unwrap(),
                PathBuf::from(expected),
                "flag={flag:?} env={env:?} dev={dev:?} debug={debug}"
            );
        }
    }

    #[test]
    fn resolves_blob_dir_from_db_path_and_config() {
        let db_path = PathBuf::from("/tmp/aven/db.sqlite");
        let config = AppConfig::default();
        assert_eq!(
            resolve_blob_dir(&db_path, &config).unwrap(),
            PathBuf::from("/tmp/aven/db.sqlite.blobs")
        );

        let mut config = AppConfig::default();
        config.local.blob_dir = Some(PathBuf::from("blobs"));
        assert_eq!(
            resolve_blob_dir(&db_path, &config).unwrap(),
            PathBuf::from("/tmp/aven/blobs")
        );

        config.local.blob_dir = Some(PathBuf::from("/var/aven/blobs"));
        assert_eq!(
            resolve_blob_dir(&db_path, &config).unwrap(),
            PathBuf::from("/var/aven/blobs")
        );
    }
}
