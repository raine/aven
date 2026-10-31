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

/// Paths below the owner's home that Aven reads or writes, from every path
/// source in the root and core crates, listed in the test
/// `every_owner_path_source_is_audited`, which fails when a new one appears.
const OWNER_PATHS: &[&str] = &[
    ".aven",
    ".cache/aven",
    ".cargo/bin/aven",
    ".claude/skills/aven",
    ".codex/skills/aven",
    ".config/aven",
    ".config/opencode/skills/aven",
    ".config/systemd/user/aven-daemon.service",
    ".local/bin/aven",
    ".local/share/aven",
    ".local/state/aven",
    ".pi/agent/skills/aven",
    "Library/Keychains",
    "Library/LaunchAgents",
    "Library/Logs/aven",
];

/// Install locations outside the home, from `daemon/service.rs`, the release
/// archive's bundle name and the documented systemd state directory.
const OWNER_SYSTEM_PATHS: &[&str] = &[
    "/Applications/Aven.app",
    "/opt/homebrew/Cellar/aven",
    "/opt/homebrew/bin/aven",
    "/opt/homebrew/opt/aven",
    "/usr/local/Cellar/aven",
    "/usr/local/bin/aven",
    "/usr/local/opt/aven",
    "/var/lib/aven",
];

/// Directories, relative to the owner's home, where names alone mark Aven data:
/// macOS keys these by bundle or app name, and owners keep hand-made databases
/// and backups beside them.
const OWNER_NAME_PARENTS: &[&str] = &[
    "",
    "Library/Application Scripts",
    "Library/Application Support",
    "Library/Caches",
    "Library/Containers",
    "Library/Group Containers",
    "Library/HTTPStorages",
    "Library/Logs",
    "Library/Preferences",
    "Library/Saved Application State",
    "Library/WebKit",
];
const OWNER_NAME_PREFIXES: &[&str] = &["aven", "com.raine.aven", "fi.zendit.aven"];

const OWNER_CONFIG: &str = ".config/aven/config.yaml";
const OWNER_DAEMON_PLIST: &str = "Library/LaunchAgents/com.raine.aven.daemon.plist";

/// Variables that move Aven's storage and that a development launch never sets.
const REFUSED_ENVIRONMENT: &[&str] = &[
    "AVEN_DEV_DB",
    "AVEN_DEV_MIGRATION_BACKUPS",
    "AVEN_SERVER_CONFIG",
    "AVEN_TEST_PROTECTED_KEYS",
    "STATE_DIRECTORY",
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
/// Anything else in the file is refused, because other sections can run
/// commands or redirect storage.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IsolatedConfig {
    sync: SyncSettings,
    daemon: DaemonSettings,
    #[serde(default)]
    local: LocalSettings,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SyncSettings {
    enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DaemonSettings {
    wake_addr: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalSettings {
    db_path: Option<PathBuf>,
    blob_dir: Option<PathBuf>,
}

/// The parts of the owner's own config that place data or claim a port.
#[derive(Debug, Default, Deserialize)]
struct OwnerConfig {
    #[serde(default)]
    local: OwnerLocal,
    #[serde(default)]
    daemon: OwnerDaemon,
}

#[derive(Debug, Default, Deserialize)]
struct OwnerLocal {
    db_path: Option<PathBuf>,
    blob_dir: Option<PathBuf>,
}

#[derive(Debug, Default, Deserialize)]
struct OwnerDaemon {
    wake_addr: Option<String>,
}

/// What the owner's installation occupies, including what its config and
/// installed daemon point at. Read-only.
#[derive(Debug)]
struct Owner {
    paths: Vec<(PathBuf, Vec<Step>)>,
    /// A directory and a lowercase name prefix within it.
    names: Vec<(Vec<Step>, String)>,
    wake_ports: Vec<u16>,
}

impl Owner {
    fn read(home: &Path) -> Result<Self> {
        let mut paths: Vec<PathBuf> = OWNER_PATHS.iter().map(|path| home.join(path)).collect();
        paths.extend(OWNER_SYSTEM_PATHS.iter().map(PathBuf::from));
        let mut names = Vec::new();
        for parent in OWNER_NAME_PARENTS {
            let steps = walk(&home.join(parent))?;
            for prefix in OWNER_NAME_PREFIXES {
                names.push((steps.clone(), prefix.to_string()));
            }
        }
        let mut wake_ports = vec![OWNER_WAKE_PORT];

        let mut config_files = vec![home.join(OWNER_CONFIG)];
        let plist = home.join(OWNER_DAEMON_PLIST);
        if let Some(text) = read_if_present(&plist)? {
            let daemon = daemon_paths(&text);
            config_files.extend(daemon.config_dirs.iter().map(|dir| dir.join("config.yaml")));
            paths.extend(daemon.config_dirs);
            for database in daemon.databases {
                add_database(&database, &mut paths, &mut names)?;
            }
            paths.extend(daemon.other);
        }
        for file in config_files {
            let Some(text) = read_if_present(&file)? else {
                continue;
            };
            let config: OwnerConfig = serde_saphyr::from_str(&text)
                .with_context(|| format!("read {}", file.display()))?;
            let database = config.local.db_path.map(|path| expand_owner(&path, home));
            if let Some(database) = &database {
                add_database(database, &mut paths, &mut names)?;
            }
            if let Some(blobs) = config.local.blob_dir {
                let base = database.as_deref().and_then(Path::parent);
                let blobs = expand_owner(&blobs, home);
                paths.push(base.map_or(blobs.clone(), |base| base.join(&blobs)));
            }
            if let Some(address) = config.daemon.wake_addr
                && let Ok(address) = address.parse::<SocketAddr>()
            {
                wake_ports.push(address.port());
            }
        }

        let paths = paths
            .into_iter()
            .map(|path| {
                let steps = walk(&path)?;
                Ok((path, steps))
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            paths,
            names,
            wake_ports,
        })
    }
}

/// A database's blobs and sidecars share its name, so the name protects them.
fn add_database(
    database: &Path,
    paths: &mut Vec<PathBuf>,
    names: &mut Vec<(Vec<Step>, String)>,
) -> Result<()> {
    paths.push(database.to_path_buf());
    if let (Some(parent), Some(name)) = (database.parent(), database.file_name()) {
        names.push((walk(parent)?, name.to_string_lossy().to_lowercase()));
    }
    Ok(())
}

fn expand_owner(path: &Path, home: &Path) -> PathBuf {
    path.strip_prefix("~")
        .map_or_else(|_| path.to_path_buf(), |rest| home.join(rest))
}

fn read_if_present(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("read {}", path.display())),
    }
}

/// Paths an installed daemon's plist names.
#[derive(Debug, Default, PartialEq, Eq)]
struct DaemonPaths {
    databases: Vec<PathBuf>,
    config_dirs: Vec<PathBuf>,
    other: Vec<PathBuf>,
}

/// Reads the flat key and string sequence that `daemon/service.rs` renders.
fn daemon_paths(plist: &str) -> DaemonPaths {
    let mut paths = DaemonPaths::default();
    let mut previous = String::new();
    for (tag, close) in [("<key>", "</key>"), ("<string>", "</string>")]
        .iter()
        .flat_map(|(open, close)| {
            plist
                .match_indices(open)
                .map(move |(index, _)| (index + open.len(), *close))
        })
        .collect::<BTreeMap<_, _>>()
    {
        let Some(length) = plist[tag..].find(close) else {
            continue;
        };
        let value = plist[tag..tag + length]
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&apos;", "'")
            .replace("&quot;", "\"")
            .replace("&amp;", "&");
        let path = PathBuf::from(&value);
        match previous.as_str() {
            "--db" | "AVEN_DB" | "AVEN_DEV_DB" => paths.databases.push(path),
            "AVEN_CONFIG_DIR" => paths.config_dirs.push(path),
            "XDG_STATE_HOME" => paths.other.push(path.join("aven")),
            "StandardOutPath" | "StandardErrorPath" => paths.other.push(path),
            _ => {}
        }
        previous = value;
    }
    paths
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
        isolation.require_isolated_config(&Owner::read(&owner_home()?)?)?;
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
        let owner = Owner::read(owner_home)?;
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
        for name in REFUSED_ENVIRONMENT {
            if let Some(value) = environment.get(*name) {
                bail!(
                    "{name} must be unset, found {}; it moves storage outside the root",
                    Path::new(value).display()
                );
            }
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
    fn require_isolated_config(&self, owner: &Owner) -> Result<()> {
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
        if !wake.ip().is_loopback() || owner.wake_ports.contains(&wake.port()) {
            bail!(
                "daemon.wake_addr must be a private loopback port, found {wake} in {}",
                path.display()
            );
        }
        if let Some(database) = &config.local.db_path
            && database != &self.database
        {
            bail!(
                "local.db_path must be {}, found {}",
                self.database.display(),
                database.display()
            );
        }
        if let Some(blobs) = &config.local.blob_dir {
            let parent = self
                .database
                .parent()
                .context("the database has a parent")?;
            if parent.join(blobs) != self.blobs {
                bail!(
                    "local.blob_dir must be {}, found {}",
                    self.blobs.display(),
                    blobs.display()
                );
            }
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

/// Refuses a path inside owner data, a path that contains owner data, such as
/// the home directory itself, and a name reserved in one of the owner's
/// directories. Any `.aven` component is a worktree's copy of the real database.
fn refuse_owner_path(name: &str, path: &Path, owner: &Owner) -> Result<()> {
    let steps = walk(path).with_context(|| format!("{name} cannot be resolved"))?;
    if steps
        .iter()
        .any(|step| step.name.eq_ignore_ascii_case(".aven"))
    {
        bail!("{name} is inside a worktree data copy: {}", path.display());
    }
    for (protected, protected_steps) in &owner.paths {
        if below(protected_steps, &steps).is_some() || below(&steps, protected_steps).is_some() {
            bail!(
                "{name} overlaps the owner's {}: {}",
                protected.display(),
                path.display()
            );
        }
    }
    for (parent, prefix) in &owner.names {
        let Some(child) = below(parent, &steps).and_then(<[Step]>::first) else {
            continue;
        };
        if child
            .name
            .to_string_lossy()
            .to_lowercase()
            .starts_with(prefix)
        {
            bail!(
                "{name} overlaps the owner's data named {prefix}*: {}",
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

/// The part of `inner` below `outer`, empty when they are the same place.
/// Existing components match by identity and missing ones by name, ignoring
/// case as the default macOS volume does.
fn below<'a>(outer: &[Step], inner: &'a [Step]) -> Option<&'a [Step]> {
    let anchor = outer.iter().rposition(|step| step.identity.is_some())?;
    let tail = &outer[anchor + 1..];
    inner.iter().enumerate().find_map(|(index, step)| {
        let rest = &inner[index + 1..];
        (step.identity == outer[anchor].identity
            && rest.len() >= tail.len()
            && tail
                .iter()
                .zip(rest)
                .all(|(left, right)| same_name(&left.name, &right.name)))
        .then(|| &rest[tail.len()..])
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

    fn with_config(text: Option<&str>, owner_home: &Path) -> Result<()> {
        let root = tempfile::tempdir().unwrap();
        let isolation = Isolation::layout(root.path());
        std::fs::create_dir_all(&isolation.config_dir).unwrap();
        if let Some(text) = text {
            let text = text.replace("ROOT", &root.path().display().to_string());
            std::fs::write(&isolation.config_file, text).unwrap();
        }
        isolation.require_isolated_config(&Owner::read(owner_home)?)
    }

    fn config(enabled: bool, address: &str) -> String {
        format!("sync:\n  enabled: {enabled}\ndaemon:\n  wake_addr: \"{address}\"\n")
    }

    #[test]
    fn requires_a_private_wake_port_and_no_sync() {
        let owner = tempfile::tempdir().unwrap();
        with_config(Some(&config(false, "127.0.0.1:50000")), owner.path()).unwrap();
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
            assert!(
                with_config(text.as_deref(), owner.path()).is_err(),
                "{reason}"
            );
        }
    }

    #[test]
    fn refuses_storage_settings_outside_the_layout() {
        let owner = tempfile::tempdir().unwrap();
        let base = config(false, "127.0.0.1:50000");
        for allowed in [
            "local:\n  db_path: ROOT/state/aven/db.sqlite\n",
            "local:\n  blob_dir: ROOT/state/aven/db.sqlite.blobs\n",
            "local:\n  blob_dir: db.sqlite.blobs\n",
        ] {
            with_config(Some(&format!("{base}{allowed}")), owner.path()).unwrap();
        }
        for refused in [
            "local:\n  db_path: /elsewhere/db.sqlite\n",
            "local:\n  blob_dir: /elsewhere/blobs\n",
            "local:\n  blob_dir: ../blobs\n",
            "local:\n  image_optimization: {}\n",
            "agent:\n  task_intake:\n    command: /bin/sh\n",
            "reminders:\n  enabled: true\n",
        ] {
            let error = with_config(Some(&format!("{base}{refused}")), owner.path());
            assert!(error.is_err(), "{refused}");
        }
    }

    fn refuses_in_owner(relative: &str) {
        let owner = tempfile::tempdir().unwrap();
        let root = owner.path().join(relative);
        assert!(
            refusal(&valid(&root), owner.path()).contains("overlaps the owner's"),
            "{relative}"
        );
    }

    #[test]
    fn refuses_every_listed_owner_path() {
        for path in OWNER_PATHS.iter().filter(|path| **path != ".aven") {
            refuses_in_owner(&format!("{path}/run"));
        }
    }

    #[test]
    fn refuses_the_daemon_log_directory() {
        refuses_in_owner("Library/Logs/aven/run");
    }

    #[test]
    fn refuses_names_reserved_in_owner_directories() {
        for path in [
            "Library/Application Support/aven-real-install-backup-20261014/run",
            "Library/Application Support/Aven/run",
            "Library/Application Support/fi.zendit.Aven/run",
            "Library/Caches/fi.zendit.Aven/run",
            "Library/Containers/fi.zendit.Aven/run",
            "Library/Preferences/fi.zendit.Aven.plist",
            "Library/Saved Application State/fi.zendit.Aven.savedState/run",
            "aven-e2ee.sqlite",
            "aven-e2ee-test/run",
        ] {
            refuses_in_owner(path);
        }
    }

    #[test]
    fn refuses_install_locations_outside_the_home() {
        let owner = tempfile::tempdir().unwrap();
        for path in OWNER_SYSTEM_PATHS {
            let root = Path::new(path).join("run");
            assert!(
                refusal(&valid(&root), owner.path()).contains("overlaps the owner's"),
                "{path}"
            );
        }
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn refuses_storage_the_owner_config_points_at() {
        let owner = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let database = data.path().join("real.sqlite");
        let blobs = data.path().join("elsewhere");
        write(
            &owner.path().join(OWNER_CONFIG),
            &format!(
                "local:\n  db_path: {}\n  blob_dir: {}\ndaemon:\n  wake_addr: \"127.0.0.1:51234\"\nagent:\n  other: 1\n",
                database.display(),
                blobs.display()
            ),
        );
        for root in [
            database.clone(),
            data.path().join("real.sqlite.blobs/run"),
            blobs.join("run"),
        ] {
            assert!(
                refusal(&valid(&root), owner.path()).contains("overlaps the owner's"),
                "{}",
                root.display()
            );
        }
        let error = with_config(Some(&config(false, "127.0.0.1:51234")), owner.path());
        assert!(error.is_err(), "the owner's configured wake port");
    }

    #[test]
    fn refuses_storage_the_installed_daemon_points_at() {
        let owner = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let (database, logs, config_dir, state) = (
            data.path().join("daemon.sqlite"),
            data.path().join("logs/out.log"),
            data.path().join("config"),
            data.path().join("state"),
        );
        write(
            &owner.path().join(OWNER_DAEMON_PLIST),
            &format!(
                "<plist><dict><key>ProgramArguments</key><array>\
                 <string>/bin/aven</string><string>--db</string><string>{}</string>\
                 <string>daemon</string></array>\
                 <key>EnvironmentVariables</key><dict>\
                 <key>AVEN_CONFIG_DIR</key><string>{}</string>\
                 <key>XDG_STATE_HOME</key><string>{}</string></dict>\
                 <key>StandardOutPath</key><string>{}</string></dict></plist>",
                database.display(),
                config_dir.display(),
                state.display(),
                logs.display()
            ),
        );
        write(
            &config_dir.join("config.yaml"),
            "daemon:\n  wake_addr: \"127.0.0.1:51235\"\n",
        );
        for root in [
            database.join("run"),
            logs.clone(),
            config_dir.join("run"),
            state.join("aven/run"),
        ] {
            assert!(
                refusal(&valid(&root), owner.path()).contains("overlaps the owner's"),
                "{}",
                root.display()
            );
        }
        let error = with_config(Some(&config(false, "127.0.0.1:51235")), owner.path());
        assert!(error.is_err(), "the daemon's configured wake port");
    }

    fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }

    /// A new home or XDG lookup can add an owner path. When this fails, read the
    /// new source, extend the lists above, then add the file here.
    #[test]
    fn every_owner_path_source_is_audited() {
        const PATTERNS: &[&str] = &[
            "home_dir()",
            "dirs::",
            "\"HOME\"",
            "XDG_STATE_HOME",
            "XDG_CONFIG_HOME",
            "XDG_CACHE_HOME",
            "XDG_DATA_HOME",
            "STATE_DIRECTORY",
        ];
        const AUDITED: &[&str] = &[
            "src/cli/sync.rs",
            "src/commands/doctor/bootstrap.rs",
            "src/commands/skill.rs",
            "src/config/paths.rs",
            "src/daemon/service.rs",
            "src/daemon/service/systemd.rs",
            "src/logging.rs",
            "src/protected_local_keys.rs",
            "src/sync/encrypted/tests.rs",
            "src/tui/app.rs",
            "src/tui/changelog.rs",
            "src/tui/custom_command.rs",
            "src/tui/natural_add_runtime.rs",
            "src/update/cache.rs",
            "src/update/install.rs",
        ];
        let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut files = Vec::new();
        for directory in ["src", "crates/aven-core/src", "crates/aven-protocol/src"] {
            sources(&repository.join(directory), &mut files);
        }
        let mut found: Vec<String> = files
            .iter()
            .filter(|file| {
                let text = std::fs::read_to_string(file).unwrap();
                PATTERNS.iter().any(|pattern| text.contains(pattern))
            })
            .map(|file| {
                file.strip_prefix(&repository)
                    .unwrap()
                    .display()
                    .to_string()
            })
            .collect();
        found.sort();
        assert_eq!(found, AUDITED);
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
        for name in REFUSED_ENVIRONMENT {
            let mut environment = valid(root.path());
            environment.insert(name.to_string(), OsString::from("/x/elsewhere"));
            assert!(refusal(&environment, owner.path()).contains(name), "{name}");
        }
    }
}
