//! The development app's filesystem boundary.
//!
//! Every development launch names a disposable root, and all state resolves
//! below it. The guard runs before the app touches a window or a database, so a
//! launch without a valid root refuses to start. Owner directories come from the
//! password database rather than `$HOME`, which launchers point into the root.
use std::collections::BTreeMap;
use std::ffi::{CStr, OsString};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use aven_core::sync::client::keys::{FileProtectedStorage, ProtectedStorage};

pub const ROOT_VAR: &str = "AVEN_DESKTOP_ROOT";

/// Paths below the owner's home that hold real Aven data or daemon state.
const OWNER_PATHS: &[&str] = &[
    ".aven",
    ".cache/aven",
    ".config/aven",
    ".local/share/aven",
    ".local/state/aven",
    "Library/Application Support/Aven",
    "Library/Application Support/aven",
    "Library/Application Support/fi.zendit.Aven",
    "Library/Caches/fi.zendit.Aven",
    "Library/Containers/fi.zendit.Aven",
    "Library/Keychains",
    "Library/LaunchAgents",
    "Library/Preferences/fi.zendit.Aven.plist",
];

/// One launch's private paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Isolation {
    pub root: PathBuf,
    pub home: PathBuf,
    pub state_home: PathBuf,
    pub config_home: PathBuf,
    pub config_dir: PathBuf,
    pub cache_home: PathBuf,
    pub database: PathBuf,
    pub blobs: PathBuf,
    pub log_file: PathBuf,
    pub keys: PathBuf,
    pub server: PathBuf,
}

impl Isolation {
    /// The layout below `root`. Blobs follow the core's default next to the
    /// database, and the state layout matches `XDG_STATE_HOME/aven`.
    pub fn layout(root: &Path) -> Self {
        let state_home = root.join("state");
        let state = state_home.join("aven");
        let config_home = root.join("config");
        let database = state.join("db.sqlite");
        Self {
            root: root.to_path_buf(),
            home: root.join("home"),
            config_dir: config_home.join("aven"),
            cache_home: root.join("cache"),
            blobs: state.join("db.sqlite.blobs"),
            log_file: state.join("logs/desktop.log"),
            keys: state.join("protected-keys"),
            server: state.join("server"),
            database,
            state_home,
            config_home,
        }
    }

    /// The process environment that places every Aven path below the root.
    pub fn environment(&self) -> Vec<(&'static str, &Path)> {
        vec![
            (ROOT_VAR, self.root.as_path()),
            ("HOME", self.home.as_path()),
            ("XDG_STATE_HOME", self.state_home.as_path()),
            ("XDG_CONFIG_HOME", self.config_home.as_path()),
            ("XDG_CACHE_HOME", self.cache_home.as_path()),
            ("AVEN_CONFIG_DIR", self.config_dir.as_path()),
            ("AVEN_DB", self.database.as_path()),
            ("AVEN_LOG_FILE", self.log_file.as_path()),
        ]
    }

    /// Validates this process's environment against the owner's real home.
    pub fn from_process() -> Result<Self> {
        let environment = std::env::vars_os()
            .filter_map(|(key, value)| Some((key.into_string().ok()?, value)))
            .collect();
        Self::resolve(&environment, &owner_home()?)
    }

    /// Requires a root and an environment that cannot reach the owner's data.
    pub fn resolve(environment: &BTreeMap<String, OsString>, owner_home: &Path) -> Result<Self> {
        let root = environment.get(ROOT_VAR).with_context(|| {
            format!("{ROOT_VAR} is required; start with scripts/desktop/launch")
        })?;
        let root = PathBuf::from(root);
        if !root.is_absolute() {
            bail!("{ROOT_VAR} must be absolute: {}", root.display());
        }
        let owner = owner_paths(owner_home);
        refuse_owner_path(ROOT_VAR, &root, &owner)?;
        let isolation = Self::layout(&root);
        for (name, expected) in isolation.environment() {
            let value = environment
                .get(name)
                .with_context(|| format!("{name} must be set below {ROOT_VAR}"))?;
            if Path::new(value) != expected {
                bail!(
                    "{name} must be {}, found {}",
                    expected.display(),
                    Path::new(value).display()
                );
            }
        }
        if let Some(value) = environment.get("AVEN_DEV_DB") {
            bail!(
                "AVEN_DEV_DB must be unset, found {}; worktree copies hold real data",
                Path::new(value).display()
            );
        }
        for (name, path) in isolation.resolved_paths() {
            refuse_owner_path(name, path, &owner)?;
        }
        Ok(isolation)
    }

    /// Each resolved path, for the guard and for launch records.
    pub fn resolved_paths(&self) -> [(&'static str, &Path); 11] {
        [
            ("root", &self.root),
            ("home", &self.home),
            ("state", &self.state_home),
            ("config", &self.config_home),
            ("config dir", &self.config_dir),
            ("cache", &self.cache_home),
            ("database", &self.database),
            ("blobs", &self.blobs),
            ("logs", &self.log_file),
            ("keys", &self.keys),
            ("server storage", &self.server),
        ]
    }

    pub fn create_directories(&self) -> Result<()> {
        for directory in [
            &self.home,
            &self.state_home,
            &self.config_dir,
            &self.cache_home,
            &self.keys,
            &self.server,
        ] {
            std::fs::create_dir_all(directory)
                .with_context(|| format!("create {}", directory.display()))?;
        }
        if let Some(parent) = self.log_file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if let Some(parent) = self.database.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(())
    }

    /// File-backed protected keys. Development launches have no Keychain path.
    pub fn protected_storage(&self) -> Arc<dyn ProtectedStorage> {
        Arc::new(FileProtectedStorage::new(self.keys.clone()))
    }
}

fn owner_paths(owner_home: &Path) -> Vec<PathBuf> {
    let home = resolve_through_links(owner_home);
    OWNER_PATHS.iter().map(|path| home.join(path)).collect()
}

/// Refuses a path inside owner data, and a path that contains owner data, such
/// as the home directory itself. Any `.aven` component is a worktree's copy of
/// the real database.
fn refuse_owner_path(name: &str, path: &Path, owner: &[PathBuf]) -> Result<()> {
    let resolved = resolve_through_links(path);
    if resolved
        .components()
        .any(|component| component == Component::Normal(".aven".as_ref()))
    {
        bail!(
            "{name} is inside a worktree data copy: {}",
            resolved.display()
        );
    }
    for protected in owner {
        if resolved.starts_with(protected) || protected.starts_with(&resolved) {
            bail!(
                "{name} overlaps the owner's {}: {}",
                protected.display(),
                resolved.display()
            );
        }
    }
    Ok(())
}

/// Canonicalizes the longest existing prefix, so a symlink cannot hide owner
/// data and a path that does not exist yet still resolves.
fn resolve_through_links(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut missing = Vec::new();
    loop {
        if let Ok(resolved) = existing.canonicalize() {
            return missing
                .iter()
                .rev()
                .fold(resolved, |path, part| path.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                missing.push(name);
                existing = parent;
            }
            _ => return path.to_path_buf(),
        }
    }
}

/// The login user's home from the password database.
pub fn owner_home() -> Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;

    let mut buffer = vec![0u8; 16 * 1024];
    let mut entry = unsafe { std::mem::zeroed::<libc::passwd>() };
    let mut found = std::ptr::null_mut();
    // SAFETY: every pointer is valid for the call, and `buffer` outlives the
    // `CStr` read from the entry.
    let status = unsafe {
        libc::getpwuid_r(
            libc::getuid(),
            &mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() || entry.pw_dir.is_null() {
        bail!("could not read the owner's home directory");
    }
    // SAFETY: `pw_dir` is a NUL-terminated string inside `buffer`.
    let home = unsafe { CStr::from_ptr(entry.pw_dir) };
    Ok(Path::new(std::ffi::OsStr::from_bytes(home.to_bytes())).to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid(root: &Path) -> BTreeMap<String, OsString> {
        Isolation::layout(root)
            .environment()
            .into_iter()
            .map(|(key, value)| (key.to_string(), value.as_os_str().to_os_string()))
            .collect()
    }

    fn refusal(environment: &BTreeMap<String, OsString>, owner: &Path) -> String {
        format!(
            "{:#}",
            Isolation::resolve(environment, owner).expect_err("the guard refuses")
        )
    }

    #[test]
    fn accepts_a_disposable_root() {
        let owner = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let isolation = Isolation::resolve(&valid(root.path()), owner.path()).unwrap();
        for (name, path) in isolation.resolved_paths() {
            assert!(path.starts_with(root.path()), "{name}: {}", path.display());
        }
    }

    #[test]
    fn requires_a_root() {
        let owner = tempfile::tempdir().unwrap();
        assert!(refusal(&BTreeMap::new(), owner.path()).contains("scripts/desktop/launch"));
        let mut relative = BTreeMap::new();
        relative.insert(ROOT_VAR.to_string(), OsString::from("relative"));
        assert!(refusal(&relative, owner.path()).contains("absolute"));
    }

    #[test]
    fn refuses_roots_inside_or_containing_owner_data() {
        let owner = tempfile::tempdir().unwrap();
        for root in [
            owner.path().join(".local/state/aven/run"),
            owner.path().join(".config/aven"),
            owner.path().join("Library/Keychains/run"),
            owner.path().to_path_buf(),
            owner.path().join("Library"),
        ] {
            assert!(
                refusal(&valid(&root), owner.path()).contains("overlaps the owner's"),
                "{}",
                root.display()
            );
        }
    }

    #[test]
    fn refuses_a_symlink_into_owner_data() {
        let owner = tempfile::tempdir().unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        let state = owner.path().join(".local/state/aven");
        std::fs::create_dir_all(&state).unwrap();
        let link = elsewhere.path().join("innocent");
        std::os::unix::fs::symlink(&state, &link).unwrap();
        assert!(refusal(&valid(&link), owner.path()).contains("overlaps the owner's"));
    }

    #[test]
    fn refuses_a_symlinked_subdirectory_into_owner_data() {
        let owner = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let state = owner.path().join(".local/state/aven");
        std::fs::create_dir_all(&state).unwrap();
        std::os::unix::fs::symlink(owner.path().join(".local/state"), root.path().join("state"))
            .unwrap();
        assert!(refusal(&valid(root.path()), owner.path()).contains("overlaps the owner's"));
    }

    #[test]
    fn refuses_a_worktree_data_copy() {
        let owner = tempfile::tempdir().unwrap();
        let worktree = tempfile::tempdir().unwrap();
        let root = worktree.path().join(".aven/run");
        assert!(refusal(&valid(&root), owner.path()).contains("worktree data copy"));
    }

    #[test]
    fn refuses_environment_that_escapes_the_root() {
        let owner = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        for name in ["AVEN_DB", "XDG_STATE_HOME", "AVEN_CONFIG_DIR", "HOME"] {
            let mut environment = valid(root.path());
            environment.insert(name.to_string(), OsString::from("/elsewhere/db.sqlite"));
            assert!(
                refusal(&environment, owner.path()).contains(name),
                "{name} escaped"
            );
            environment.remove(name);
            assert!(
                refusal(&environment, owner.path()).contains(name),
                "{name} missing"
            );
        }
        let mut environment = valid(root.path());
        environment.insert("AVEN_DEV_DB".to_string(), OsString::from("/x/db.sqlite"));
        assert!(refusal(&environment, owner.path()).contains("AVEN_DEV_DB"));
    }
}
