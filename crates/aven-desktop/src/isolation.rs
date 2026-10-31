//! The development app's filesystem boundary.
//!
//! Every development launch names a disposable root, and all state resolves
//! below it. The guard runs before the app touches a window or a database, so a
//! launch without a valid root refuses to start. Owner directories come from the
//! password database rather than `$HOME`, which launchers point into the root.
//!
//! Paths are compared by device and inode, not by text, because one directory
//! can have many spellings: symlinks, firmlinks such as `/System/Volumes/Data`,
//! and case on a case-insensitive volume.
use std::collections::{BTreeMap, VecDeque};
use std::ffi::{CStr, OsStr, OsString};
use std::net::SocketAddr;
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use aven_core::sync::client::keys::{FileProtectedStorage, ProtectedStorage};
use serde::Deserialize;

pub const ROOT_VAR: &str = "AVEN_DESKTOP_ROOT";

/// The wake port the shared config falls back to, where a real daemon listens.
const OWNER_WAKE_PORT: u16 = 47631;
/// The kernel's own limit on symlinks followed while resolving one path.
const MAX_LINKS: usize = 32;

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
    pub config_file: PathBuf,
    pub cache_home: PathBuf,
    pub database: PathBuf,
    pub blobs: PathBuf,
    pub log_file: PathBuf,
    pub keys: PathBuf,
    pub server: PathBuf,
    pub launch_record: PathBuf,
}

/// The settings that keep a launch away from a real daemon and sync server.
#[derive(Debug, Deserialize)]
struct IsolatedConfig {
    sync: SyncSettings,
    daemon: DaemonSettings,
}

#[derive(Debug, Deserialize)]
struct SyncSettings {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
struct DaemonSettings {
    wake_addr: String,
}

impl Isolation {
    /// The layout below `root`. Blobs follow the core's default next to the
    /// database, and the state layout matches `XDG_STATE_HOME/aven`.
    pub fn layout(root: &Path) -> Self {
        let state_home = root.join("state");
        let state = state_home.join("aven");
        let config_home = root.join("config");
        let database = state.join("db.sqlite");
        let config_dir = config_home.join("aven");
        Self {
            root: root.to_path_buf(),
            home: root.join("home"),
            config_file: config_dir.join("config.yaml"),
            config_dir,
            cache_home: root.join("cache"),
            blobs: state.join("db.sqlite.blobs"),
            log_file: state.join("logs/desktop.log"),
            keys: state.join("protected-keys"),
            server: state.join("server"),
            launch_record: root.join("launch.json"),
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

    /// Validates this process's environment and the launch's config.
    pub fn from_process() -> Result<Self> {
        let isolation = Self::paths_from_process()?;
        isolation.require_isolated_config()?;
        Ok(isolation)
    }

    /// Validates this process's environment against the owner's real home.
    /// Launchers run this before writing the config into the root.
    pub fn paths_from_process() -> Result<Self> {
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
        // A `..` after a missing directory cannot be resolved until that
        // directory exists, which is after the guard has run.
        if root.components().any(|part| part == Component::ParentDir) {
            bail!("{ROOT_VAR} must not contain `..`: {}", root.display());
        }
        let owner = owner_paths(owner_home)?;
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
    pub fn resolved_paths(&self) -> [(&'static str, &Path); 13] {
        [
            ("root", &self.root),
            ("home", &self.home),
            ("state", &self.state_home),
            ("config", &self.config_home),
            ("config dir", &self.config_dir),
            ("config file", &self.config_file),
            ("cache", &self.cache_home),
            ("database", &self.database),
            ("blobs", &self.blobs),
            ("logs", &self.log_file),
            ("keys", &self.keys),
            ("server storage", &self.server),
            ("launch record", &self.launch_record),
        ]
    }

    /// Requires this launch's own config. Without one, the shared defaults
    /// reach the owner's daemon on its wake port.
    pub fn require_isolated_config(&self) -> Result<()> {
        let path = &self.config_file;
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("{} is required", path.display()))?;
        let config: IsolatedConfig =
            serde_saphyr::from_str(&text).with_context(|| format!("read {}", path.display()))?;
        if config.sync.enabled {
            bail!("sync.enabled must be false in {}", path.display());
        }
        let wake: SocketAddr = config
            .daemon
            .wake_addr
            .parse()
            .with_context(|| format!("daemon.wake_addr in {}", path.display()))?;
        if !wake.ip().is_loopback() || wake.port() == OWNER_WAKE_PORT {
            bail!(
                "daemon.wake_addr must be a private loopback port, found {wake} in {}",
                path.display()
            );
        }
        Ok(())
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

fn owner_paths(owner_home: &Path) -> Result<Vec<(PathBuf, Vec<Step>)>> {
    OWNER_PATHS
        .iter()
        .map(|path| {
            let path = owner_home.join(path);
            let steps = walk(&path)?;
            Ok((path, steps))
        })
        .collect()
}

/// Refuses a path inside owner data, and a path that contains owner data, such
/// as the home directory itself. Any `.aven` component is a worktree's copy of
/// the real database.
fn refuse_owner_path(name: &str, path: &Path, owner: &[(PathBuf, Vec<Step>)]) -> Result<()> {
    let steps = walk(path).with_context(|| format!("{name} cannot be resolved"))?;
    if steps
        .iter()
        .any(|step| step.name.eq_ignore_ascii_case(".aven"))
    {
        bail!("{name} is inside a worktree data copy: {}", path.display());
    }
    for (protected, protected_steps) in owner {
        if contains(protected_steps, &steps) || contains(&steps, protected_steps) {
            bail!(
                "{name} overlaps the owner's {}: {}",
                protected.display(),
                path.display()
            );
        }
    }
    Ok(())
}

/// One component of a fully resolved path.
#[derive(Debug, Clone)]
struct Step {
    name: OsString,
    /// Device and inode, or `None` for a component that does not exist yet.
    identity: Option<(u64, u64)>,
}

/// Resolves a path one component at a time, following every symlink, including
/// a dangling one, whose target is where a later write would land.
fn walk(path: &Path) -> Result<Vec<Step>> {
    let root = std::fs::metadata("/")?;
    let mut steps = vec![Step {
        name: "/".into(),
        identity: Some((root.dev(), root.ino())),
    }];
    let mut pending: VecDeque<OsString> = components(path).collect();
    let mut links = 0;
    while let Some(part) = pending.pop_front() {
        let last = steps.last().context("a path has a root")?;
        if part == ".." {
            if last.identity.is_none() {
                bail!("`..` follows a missing directory in {}", path.display());
            }
            if steps.len() > 1 {
                steps.pop();
            }
            continue;
        }
        if last.identity.is_none() {
            steps.push(Step {
                name: part,
                identity: None,
            });
            continue;
        }
        let here: PathBuf = steps
            .iter()
            .map(|step| step.name.as_os_str())
            .chain([part.as_os_str()])
            .collect();
        match std::fs::symlink_metadata(&here) {
            Ok(metadata) if metadata.is_symlink() => {
                links += 1;
                if links > MAX_LINKS {
                    bail!("too many symlinks in {}", path.display());
                }
                let target = std::fs::read_link(&here)?;
                if target.is_absolute() {
                    steps.truncate(1);
                }
                for component in components(&target).collect::<Vec<_>>().into_iter().rev() {
                    pending.push_front(component);
                }
            }
            Ok(metadata) => steps.push(Step {
                name: part,
                identity: Some((metadata.dev(), metadata.ino())),
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => steps.push(Step {
                name: part,
                identity: None,
            }),
            Err(error) => {
                return Err(error).with_context(|| format!("inspect {}", here.display()));
            }
        }
    }
    Ok(steps)
}

/// Named components, with `..` kept for `walk` to apply.
fn components(path: &Path) -> impl Iterator<Item = OsString> + '_ {
    path.components().filter_map(|part| match part {
        Component::Normal(name) => Some(name.to_os_string()),
        Component::ParentDir => Some("..".into()),
        _ => None,
    })
}

/// Whether `inner` is `outer` or below it. Existing components match by identity
/// and missing ones by name, ignoring case as the default macOS volume does.
fn contains(outer: &[Step], inner: &[Step]) -> bool {
    let Some(anchor) = outer.iter().rposition(|step| step.identity.is_some()) else {
        return false;
    };
    let tail = &outer[anchor + 1..];
    inner.iter().enumerate().any(|(index, step)| {
        let below = &inner[index + 1..];
        step.identity == outer[anchor].identity
            && below.len() >= tail.len()
            && tail
                .iter()
                .zip(below)
                .all(|(left, right)| same_name(&left.name, &right.name))
    })
}

fn same_name(left: &OsStr, right: &OsStr) -> bool {
    left.eq_ignore_ascii_case(right)
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
    fn refuses_parent_components_after_a_missing_directory() {
        let owner = tempfile::tempdir().unwrap();
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("owner/.local/state/aven")).unwrap();
        let root = base.path().join("nope/../owner/.local/state/aven/run");
        assert!(refusal(&valid(&root), base.path().join("owner").as_path()).contains("`..`"));
        assert!(!base.path().join("nope").exists());
        let root = base.path().join("exists/..");
        assert!(refusal(&valid(&root), owner.path()).contains("`..`"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn refuses_owner_data_through_the_data_volume_firmlink() {
        let owner = tempfile::tempdir().unwrap();
        let home = owner.path().canonicalize().unwrap();
        std::fs::create_dir_all(home.join(".local/state/aven")).unwrap();
        let through_firmlink = Path::new("/System/Volumes/Data")
            .join(home.strip_prefix("/").unwrap())
            .join(".local/state/aven/run");
        assert!(through_firmlink.parent().unwrap().is_dir());
        assert!(refusal(&valid(&through_firmlink), &home).contains("overlaps the owner's"));
    }

    #[test]
    fn refuses_owner_data_reached_through_a_symlink_in_the_owner_home() {
        let owner = tempfile::tempdir().unwrap();
        let dotfiles = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dotfiles.path().join(".config/aven")).unwrap();
        std::os::unix::fs::symlink(
            dotfiles.path().join(".config"),
            owner.path().join(".config"),
        )
        .unwrap();
        for root in [
            owner.path().join(".config/aven/run"),
            dotfiles.path().join(".config/aven/run"),
        ] {
            assert!(
                refusal(&valid(&root), owner.path()).contains("overlaps the owner's"),
                "{}",
                root.display()
            );
        }
    }

    #[test]
    fn refuses_dangling_symlinks_into_owner_data() {
        let owner = tempfile::tempdir().unwrap();
        let state = owner.path().join(".local/state/aven");
        std::fs::create_dir_all(&state).unwrap();

        let root = tempfile::tempdir().unwrap();
        let database = Isolation::layout(root.path()).database;
        std::fs::create_dir_all(database.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(state.join("missing.sqlite"), &database).unwrap();
        assert!(refusal(&valid(root.path()), owner.path()).contains("database overlaps"));

        let root = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(state.join("missing"), root.path().join("state")).unwrap();
        assert!(refusal(&valid(root.path()), owner.path()).contains("overlaps the owner's"));
    }

    #[test]
    fn refuses_owner_data_spelled_in_another_case() {
        let owner = tempfile::tempdir().unwrap();
        let root = owner.path().join(".LOCAL/State/Aven/run");
        assert!(refusal(&valid(&root), owner.path()).contains("overlaps the owner's"));
        let worktree = tempfile::tempdir().unwrap();
        let root = worktree.path().join(".AVEN/run");
        assert!(refusal(&valid(&root), owner.path()).contains("worktree data copy"));
    }

    #[test]
    fn accepts_a_root_beside_missing_owner_data() {
        let owner = tempfile::tempdir().unwrap();
        let root = owner.path().join("code/disposable");
        Isolation::resolve(&valid(&root), owner.path()).unwrap();
    }

    fn with_config(text: Option<&str>) -> Result<()> {
        let root = tempfile::tempdir().unwrap();
        let isolation = Isolation::layout(root.path());
        std::fs::create_dir_all(&isolation.config_dir).unwrap();
        if let Some(text) = text {
            std::fs::write(&isolation.config_file, text).unwrap();
        }
        isolation.require_isolated_config()
    }

    #[test]
    fn requires_a_private_wake_port_and_no_sync() {
        let config = |enabled, address| {
            format!("sync:\n  enabled: {enabled}\ndaemon:\n  wake_addr: \"{address}\"\n")
        };
        with_config(Some(&config(false, "127.0.0.1:50000"))).unwrap();
        for (text, reason) in [
            (None, "no config"),
            (
                Some("sync:\n  enabled: false\n".to_string()),
                "no wake address",
            ),
            (Some(config(true, "127.0.0.1:50000")), "sync on"),
            (Some(config(false, "127.0.0.1:47631")), "the owner's port"),
            (Some(config(false, "192.0.2.1:50000")), "not loopback"),
        ] {
            assert!(with_config(text.as_deref()).is_err(), "{reason}");
        }
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
