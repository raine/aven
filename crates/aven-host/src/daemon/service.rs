#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::Path;
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(not(target_os = "linux"))]
use std::process::Output;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "macos")]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(target_os = "linux"))]
use anyhow::bail;

#[cfg(target_os = "macos")]
use crate::app_bundle::{
    AVEN_BUNDLE_ID, BUNDLE_EXECUTABLE, BUNDLE_NAME, direct_install_bundle, enclosing_bundle,
};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::config;

#[cfg(any(target_os = "linux", test))]
mod systemd;

/// The agent's launchd label, under Aven.app's bundle identifier.
#[cfg(target_os = "macos")]
const LABEL: &str = "fi.zendit.Aven.daemon";
/// The label agents had before they were named after Aven.app. Installing,
/// repairing or starting the agent retires a job under this label, so only one
/// daemon runs against the database.
#[cfg(target_os = "macos")]
const LEGACY_LABEL: &str = "com.raine.aven.daemon";
#[cfg(target_os = "macos")]
const DEFAULT_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";
#[cfg(target_os = "macos")]
const BOOTOUT_POLL_INTERVAL: Duration = Duration::from_millis(100);
#[cfg(target_os = "macos")]
const BOOTOUT_POLL_LIMIT: usize = 300;
#[cfg(target_os = "macos")]
const BOOTSTRAP_RETRY_DELAYS: [Duration; 4] = [
    Duration::from_millis(100),
    Duration::from_millis(200),
    Duration::from_millis(400),
    Duration::from_millis(800),
];

pub struct ServiceInstallArgs {
    pub db_path: PathBuf,
    pub program: Option<PathBuf>,
}

pub struct ServiceRepairArgs {
    pub db_path: PathBuf,
    pub program: Option<PathBuf>,
    pub if_installed: bool,
}

#[derive(Debug, Clone)]
pub struct ServiceStatus {
    pub platform_supported: bool,
    pub installed: bool,
    pub loaded: Option<bool>,
    pub running: Option<bool>,
    pub service_path: PathBuf,
    pub program: Option<PathBuf>,
    pub current_executable: PathBuf,
    pub program_matches_current: Option<bool>,
    /// The agent runs this executable through a path outside Aven.app, so
    /// Login Items does not attribute it to the app.
    pub bundle_migration_pending: Option<bool>,
    /// The user turned Aven off in System Settings > General > Login Items &
    /// Extensions, so launchd doesn't run the agent.
    pub turned_off_in_login_items: Option<bool>,
    pub stdout_path: Option<PathBuf>,
    pub stderr_path: Option<PathBuf>,
}

/// Where an installed service lives and where its output goes.
#[derive(Debug)]
pub struct InstalledService {
    pub path: PathBuf,
    pub logs: String,
}

impl InstalledService {
    pub fn print(&self) {
        println!("installed {}", self.path.display());
        println!("logs {}", self.logs);
    }
}

#[cfg(not(target_os = "linux"))]
pub fn install(args: ServiceInstallArgs) -> Result<InstalledService> {
    install_with_runner(args, &SystemRunner, &SystemSleeper)
}

#[cfg(target_os = "macos")]
pub fn uninstall() -> Result<()> {
    uninstall_with_runner(&SystemRunner, &ServiceSpec::current()?)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn uninstall() -> Result<()> {
    uninstall_with_runner(&SystemRunner)
}

#[cfg(target_os = "macos")]
pub fn restart() -> Result<()> {
    restart_with_runner(&SystemRunner, &ServiceSpec::current()?)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn restart() -> Result<()> {
    restart_with_runner(&SystemRunner)
}

#[cfg(not(target_os = "linux"))]
pub fn repair(args: ServiceRepairArgs) -> Result<()> {
    repair_with_runner(args, &SystemRunner, &SystemSleeper)
}

#[cfg(not(target_os = "linux"))]
pub fn status_snapshot() -> Result<ServiceStatus> {
    status_with_runner(&SystemRunner)
}

/// Whether the running daemon moved its LaunchAgent to the Aven.app executable.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleMigration {
    NotNeeded,
    Skipped(&'static str),
    /// The plist now names `program`; launchd still runs the old definition
    /// until the service is reloaded.
    Prepared {
        program: PathBuf,
    },
}

#[cfg(target_os = "macos")]
pub fn prepare_bundle_migration() -> Result<BundleMigration> {
    let running_as = std::env::var_os("XPC_SERVICE_NAME");
    let Some(running_as) = running_as.as_ref().and_then(|name| name.to_str()) else {
        return Ok(BundleMigration::NotNeeded);
    };
    let spec = ServiceSpec::current()?;
    let current_exe = std::env::current_exe().context("resolve current executable")?;
    prepare_bundle_migration_with_runner(
        &SystemRunner,
        &SystemSleeper,
        &spec,
        running_as,
        &stable_program_path(&current_exe),
    )
}

#[cfg(target_os = "macos")]
pub fn reload_service_definition() -> Result<()> {
    reload_definition_with_runner(&SystemRunner, &SystemSleeper, &ServiceSpec::current()?)
}

/// Whether launchd has the agent loaded, and whether it is disabled from
/// loading at login.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AgentState {
    pub loaded: bool,
    pub disabled: bool,
}

#[cfg(target_os = "macos")]
pub fn agent_state() -> Result<AgentState> {
    let spec = ServiceSpec::current()?.installed();
    Ok(AgentState {
        loaded: service_is_loaded(&SystemRunner, &spec)?,
        disabled: service_is_disabled(&SystemRunner, &spec)?,
    })
}

/// Unloads the agent and waits until its process is gone, leaving its plist
/// and enablement as they are.
#[cfg(target_os = "macos")]
pub fn stop_agent() -> Result<()> {
    let spec = ServiceSpec::current()?.installed();
    stop_service(&SystemRunner, &SystemSleeper, &spec).map(drop)
}

/// Loads the agent from its plist as it stands.
#[cfg(target_os = "macos")]
pub fn start_agent() -> Result<()> {
    let spec = ServiceSpec::current()?.installed();
    bootstrap_service(&SystemRunner, &SystemSleeper, &spec, true)
}

/// Where launchd reads the agent's definition.
#[cfg(target_os = "macos")]
pub fn agent_plist_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist"))
}

/// The plist of the installed agent: the one under the legacy label while
/// only that one exists.
#[cfg(target_os = "macos")]
pub fn installed_agent_plist_path(home: &Path) -> PathBuf {
    installed_plist_path(agent_plist_path(home))
}

#[cfg(target_os = "macos")]
fn installed_plist_path(plist: PathBuf) -> PathBuf {
    let legacy = legacy_plist_path(&plist);
    if !plist.exists() && legacy.exists() {
        legacy
    } else {
        plist
    }
}

#[cfg(target_os = "macos")]
fn legacy_plist_path(plist: &Path) -> PathBuf {
    plist.with_file_name(format!("{LEGACY_LABEL}.plist"))
}

/// Whether System Settings > General > Login Items & Extensions lets the
/// agent run. Turning Aven off there stops the agent and keeps launchd from
/// loading it at login, but leaves its plist and launchd's own enablement as
/// they were; a plain `launchctl bootstrap` would still start it.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginItem {
    Allowed,
    /// The user turned Aven off there. Only they can turn it on again.
    TurnedOff,
    /// The system can't say, as before macOS 13.
    Unknown,
}

/// Why the agent was not started: the user turned Aven off in Login Items.
#[cfg(target_os = "macos")]
#[derive(Debug)]
struct TurnedOffInLoginItems;

#[cfg(target_os = "macos")]
impl std::fmt::Display for TurnedOffInLoginItems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            "error daemon-turned-off-in-login-items hint=\"turn Aven on in System Settings > General > Login Items & Extensions\"",
        )
    }
}

#[cfg(target_os = "macos")]
impl std::error::Error for TurnedOffInLoginItems {}

/// The agent as Aven Desktop shows and manages it.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentStatus {
    /// The plist exists.
    pub installed: bool,
    /// The executable the plist runs, when it names one.
    pub program: Option<PathBuf>,
    pub loaded: bool,
    pub running: bool,
    pub login_item: LoginItem,
}

#[cfg(target_os = "macos")]
pub fn agent_status() -> Result<AgentStatus> {
    let spec = ServiceSpec::current()?;
    agent_status_with_runner(&SystemRunner, &spec)
}

/// What [`ensure_agent`] did.
#[cfg(target_os = "macos")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ensured {
    /// The agent was already installed as it should be and loaded.
    Unchanged,
    /// The agent was installed, rewritten or loaded.
    Started,
    /// The user turned Aven off in Login Items, so it was left off.
    TurnedOff,
}

/// Installs the agent for the database at `db_path`, or brings an installed
/// one up to date, leaving a current and loaded agent running. The
/// environment of an installed plist for the same program and database is
/// kept, so the app and `aven daemon install` from a shell don't rewrite each
/// other's. Never starts an agent the user turned off in Login Items.
#[cfg(target_os = "macos")]
pub fn ensure_agent(db_path: PathBuf) -> Result<Ensured> {
    let spec = ServiceSpec::from_install_args(db_path, None)?;
    ensure_with_runner(&SystemRunner, &SystemSleeper, &spec)
}

/// Stops the agent and removes its plist when it runs `program`, under either
/// label. Leaves launchd's enablement alone, so a later install starts
/// cleanly. Returns whether it removed one.
#[cfg(target_os = "macos")]
pub fn remove_agent(program: &Path) -> Result<bool> {
    let spec = ServiceSpec::current()?;
    remove_with_runner(&SystemRunner, &SystemSleeper, &spec, program)
}

#[cfg(not(target_os = "macos"))]
pub fn reload_service_definition() -> Result<()> {
    anyhow::bail!(
        "error unsupported-platform command=internal-reload-daemon platform={}",
        std::env::consts::OS
    )
}

#[cfg(target_os = "linux")]
pub fn install(args: ServiceInstallArgs) -> Result<InstalledService> {
    let spec = systemd::UnitSpec::from_install_args(args.db_path.clone(), args.program.clone())?;
    systemd::install_with_runner(&spec, &systemd::SystemSystemctl)
}

#[cfg(target_os = "linux")]
pub fn uninstall() -> Result<()> {
    let spec = systemd::UnitSpec::from_install_args(config::default_db_path()?, None)?;
    systemd::uninstall_with_runner(&spec, &systemd::SystemSystemctl)
}

#[cfg(target_os = "linux")]
pub fn restart() -> Result<()> {
    systemd::restart_with_runner(&systemd::SystemSystemctl)
}

#[cfg(target_os = "linux")]
pub fn repair(args: ServiceRepairArgs) -> Result<()> {
    let spec = systemd::UnitSpec::from_install_args(args.db_path.clone(), args.program.clone())?;
    systemd::repair_with_runner(args, &spec, &systemd::SystemSystemctl)
}

#[cfg(target_os = "linux")]
pub fn status_snapshot() -> Result<ServiceStatus> {
    let spec = systemd::UnitSpec::from_install_args(config::default_db_path()?, None)?;
    systemd::status_with_runner(&spec, &systemd::SystemSystemctl)
}

#[cfg(not(target_os = "linux"))]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
trait LaunchctlRunner {
    fn run(&self, args: &[&str]) -> Result<Output>;

    /// What Login Items says about the agent whose plist is at `plist`.
    #[cfg(target_os = "macos")]
    fn login_item(&self, plist: &Path) -> LoginItem;
}

#[cfg(target_os = "macos")]
trait Sleeper {
    fn sleep(&self, duration: Duration);
}

#[cfg(not(target_os = "linux"))]
struct SystemRunner;
#[cfg(not(target_os = "linux"))]
struct SystemSleeper;

#[cfg(target_os = "macos")]
impl Sleeper for SystemSleeper {
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(target_os = "macos")]
impl LaunchctlRunner for SystemRunner {
    fn run(&self, args: &[&str]) -> Result<Output> {
        Command::new("/bin/launchctl")
            .args(args)
            .output()
            .with_context(|| format!("run launchctl {}", args.join(" ")))
    }

    fn login_item(&self, plist: &Path) -> LoginItem {
        login_items::status(plist)
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
impl LaunchctlRunner for SystemRunner {
    fn run(&self, _args: &[&str]) -> Result<Output> {
        unreachable!("launchctl is only available on macOS")
    }
}

#[cfg(target_os = "macos")]
fn install_with_runner(
    args: ServiceInstallArgs,
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
) -> Result<InstalledService> {
    let spec = ServiceSpec::from_install_args(args.db_path, args.program)?;
    let plist = render_plist(&spec);
    reload_service(runner, sleeper, &spec, &plist)?;
    Ok(InstalledService {
        path: spec.plist_path,
        logs: spec.log_dir.display().to_string(),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn install_with_runner(
    args: ServiceInstallArgs,
    _runner: &impl LaunchctlRunner,
    _sleeper: &SystemSleeper,
) -> Result<InstalledService> {
    drop(args);
    bail!(
        "error unsupported-platform command=daemon-install platform={}",
        std::env::consts::OS
    )
}

#[cfg(target_os = "macos")]
fn uninstall_with_runner(runner: &impl LaunchctlRunner, spec: &ServiceSpec) -> Result<()> {
    let legacy = spec.legacy();
    for spec in [spec, &legacy] {
        if service_is_loaded(runner, spec)? {
            run_launchctl(runner, &["bootout", &spec.service_target])?;
        }
    }
    let _ = runner.run(&["disable", &spec.service_target]);
    if remove_plist(&legacy.plist_path)? {
        println!("uninstalled {}", legacy.plist_path.display());
    }
    remove_plist(&spec.plist_path)?;
    println!("uninstalled {}", spec.plist_path.display());
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn uninstall_with_runner(_runner: &impl LaunchctlRunner) -> Result<()> {
    bail!(
        "error unsupported-platform command=daemon-uninstall platform={}",
        std::env::consts::OS
    )
}

#[cfg(target_os = "macos")]
fn restart_with_runner(runner: &impl LaunchctlRunner, spec: &ServiceSpec) -> Result<()> {
    let spec = spec.installed();
    run_launchctl(runner, &["kickstart", "-k", &spec.service_target])?;
    println!("restarted {}", spec.service_target);
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn restart_with_runner(_runner: &impl LaunchctlRunner) -> Result<()> {
    bail!(
        "error unsupported-platform command=daemon-restart platform={}",
        std::env::consts::OS
    )
}

#[cfg(target_os = "macos")]
fn repair_with_runner(
    args: ServiceRepairArgs,
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
) -> Result<()> {
    let spec = ServiceSpec::from_install_args(args.db_path, args.program)?;
    if !spec.installed().plist_path.exists() {
        if args.if_installed {
            println!("daemon repair skipped installed=no");
            return Ok(());
        }
        bail!(
            "error daemon-not-installed path={}",
            spec.plist_path.display()
        );
    }
    let plist = render_plist(&spec);
    reload_service(runner, sleeper, &spec, &plist)?;
    println!(
        "daemon repair installed=yes restarted=yes path={}",
        spec.executable.display()
    );
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn repair_with_runner(
    args: ServiceRepairArgs,
    _runner: &impl LaunchctlRunner,
    _sleeper: &SystemSleeper,
) -> Result<()> {
    drop(args);
    bail!(
        "error unsupported-platform command=daemon-repair platform={}",
        std::env::consts::OS
    )
}

#[cfg(target_os = "macos")]
fn status_with_runner(runner: &impl LaunchctlRunner) -> Result<ServiceStatus> {
    let spec = ServiceSpec::current()?.installed();
    let program = read_plist_program(&spec.plist_path)?;
    let current_executable = std::env::current_exe().context("resolve current executable")?;
    let program_matches_current = program
        .as_ref()
        .map(|program| crate::paths::same_file(program, &current_executable));
    let bundled_target = enclosing_bundle(&stable_program_path(&current_executable)).is_some();
    let bundle_migration_pending = program.as_ref().map(|program| {
        bundled_target
            && enclosing_bundle(program).is_none()
            && crate::paths::same_file(program, &current_executable)
    });
    let runtime = service_runtime(runner, &spec)?;
    let turned_off_in_login_items = (runtime.loaded || spec.plist_path.exists())
        .then(|| runner.login_item(&spec.plist_path) == LoginItem::TurnedOff);
    Ok(ServiceStatus {
        platform_supported: true,
        installed: spec.plist_path.exists(),
        loaded: Some(runtime.loaded),
        running: runtime.running,
        service_path: spec.plist_path,
        program,
        current_executable,
        program_matches_current,
        bundle_migration_pending,
        turned_off_in_login_items,
        stdout_path: Some(spec.stdout_path),
        stderr_path: Some(spec.stderr_path),
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn status_with_runner(_runner: &impl LaunchctlRunner) -> Result<ServiceStatus> {
    Ok(ServiceStatus {
        platform_supported: false,
        installed: false,
        loaded: None,
        running: None,
        service_path: PathBuf::new(),
        program: None,
        current_executable: std::env::current_exe().unwrap_or_default(),
        program_matches_current: None,
        bundle_migration_pending: None,
        turned_off_in_login_items: None,
        stdout_path: None,
        stderr_path: None,
    })
}

/// Moves the agent that runs this daemon onto the current label, and onto
/// the bundle path when it runs Aven.app's executable through a symlink. Only
/// plists that match what aven renders are rewritten, so the database,
/// environment and log settings carry over unchanged. `running_as` is
/// launchd's name for this process's job.
#[cfg(target_os = "macos")]
fn prepare_bundle_migration_with_runner(
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
    base: &ServiceSpec,
    running_as: &str,
    program: &Path,
) -> Result<BundleMigration> {
    // launchd names its jobs this way; anything else is a manual `aven daemon`.
    let installed = if running_as == base.label {
        // A rename interrupted before its reload leaves the legacy plist,
        // which would start a second daemon at the next login.
        if legacy_plist_path(&base.plist_path).exists() {
            retire_legacy(runner, sleeper, base)?;
        }
        base.clone()
    } else if running_as == LEGACY_LABEL {
        base.legacy()
    } else {
        return Ok(BundleMigration::NotNeeded);
    };
    let existing = match std::fs::read_to_string(&installed.plist_path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(BundleMigration::NotNeeded);
        }
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", installed.plist_path.display()));
        }
    };
    let plist = match bundle_migration_plist(base, &installed.label, &existing, program) {
        Ok(Some(plist)) => plist,
        Ok(None) => return Ok(BundleMigration::NotNeeded),
        Err(reason) => return Ok(BundleMigration::Skipped(reason)),
    };
    if service_is_disabled(runner, &installed)? {
        return Ok(BundleMigration::Skipped("disabled"));
    }
    write_plist(base, &plist)?;
    Ok(BundleMigration::Prepared {
        program: program.to_path_buf(),
    })
}

/// Renders `existing`, the plist of the agent under `installed_label`, under
/// `base`'s label, with `program` as the executable when the agent runs it
/// through a symlink. `None` when the agent already has the label and
/// either runs from a bundle or `program` is not inside one.
#[cfg(target_os = "macos")]
fn bundle_migration_plist(
    base: &ServiceSpec,
    installed_label: &str,
    existing: &str,
    program: &Path,
) -> Result<Option<String>, &'static str> {
    let renamed = installed_label != base.label;
    let program_bundled = enclosing_bundle(program).is_some();
    if !renamed && !program_bundled {
        return Ok(None);
    }
    let mut spec = base
        .with_plist_settings(existing)
        .ok_or("unrecognized-plist")?;
    let into_bundle = program_bundled && enclosing_bundle(&spec.executable).is_none();
    if !renamed && !into_bundle {
        return Ok(None);
    }
    let as_installed = ServiceSpec {
        label: installed_label.to_string(),
        ..spec.clone()
    };
    if [KeepAlive::Always, KeepAlive::WhileProgramExists]
        .into_iter()
        .all(|keep_alive| render_plist_with(&as_installed, keep_alive) != existing)
    {
        return Err("customized-plist");
    }
    if into_bundle {
        if !crate::paths::same_file(&spec.executable, program) {
            return Err("different-program");
        }
        spec.executable = program.to_path_buf();
    }
    Ok(Some(render_plist(&spec)))
}

#[cfg(target_os = "macos")]
fn service_is_disabled(runner: &impl LaunchctlRunner, spec: &ServiceSpec) -> Result<bool> {
    let args = ["print-disabled", spec.domain.as_str()];
    let output = runner.run(&args)?;
    if !output.status.success() {
        return Err(launchctl_failure(&args, &output));
    }
    let entry = format!("\"{}\" =>", spec.label);
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(text
        .lines()
        .filter_map(|line| line.trim().strip_prefix(entry.as_str()))
        .any(|state| matches!(state.trim(), "disabled" | "true")))
}

/// Reloads the service from its plist without rewriting it. The caller must
/// not be part of the service's job, since bootout ends the job's processes.
#[cfg(target_os = "macos")]
fn reload_definition_with_runner(
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
    current: &ServiceSpec,
) -> Result<()> {
    let spec = current.installed();
    let performed_bootout = stop_service(runner, sleeper, &spec)?;
    if spec.label == current.label {
        retire_legacy(runner, sleeper, current)?;
    }
    bootstrap_service(runner, sleeper, &spec, performed_bootout)?;
    println!("reloaded {}", spec.service_target);
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
struct ServiceSpec {
    label: String,
    executable: PathBuf,
    db_path: PathBuf,
    config_dir: Option<PathBuf>,
    state_home: Option<PathBuf>,
    path_env: String,
    log_dir: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    plist_path: PathBuf,
    domain: String,
    service_target: String,
}

#[cfg(target_os = "macos")]
impl ServiceSpec {
    /// The spec for the default database and the running executable.
    fn current() -> Result<Self> {
        Self::from_install_args(config::default_db_path()?, None)
    }

    fn from_install_args(db_path: PathBuf, program: Option<PathBuf>) -> Result<Self> {
        let home = dirs::home_dir().context("could not find home directory")?;
        let log_dir = home.join("Library/Logs/aven");
        let label = LABEL.to_string();
        let uid = current_uid();
        let domain = format!("gui/{uid}");
        let service_target = format!("{domain}/{label}");
        let config_dir = std::env::var_os("AVEN_CONFIG_DIR")
            .map(PathBuf::from)
            .map(absolute_path)
            .transpose()?;
        let state_home = std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        let current_exe = std::env::current_exe().context("resolve current executable")?;
        let executable = match program {
            Some(program) => absolute_path(program)?,
            None => stable_program_path(&current_exe),
        };
        Ok(Self {
            label: label.clone(),
            executable,
            db_path: absolute_path(db_path)?,
            config_dir,
            state_home,
            path_env: std::env::var("PATH").unwrap_or_else(|_| DEFAULT_PATH.to_string()),
            stdout_path: log_dir.join("daemon.out.log"),
            stderr_path: log_dir.join("daemon.err.log"),
            log_dir,
            plist_path: agent_plist_path(&home),
            domain,
            service_target,
        })
    }

    /// This spec with the executable, database and environment of a plist
    /// rendered by `render_plist`.
    fn with_plist_settings(&self, text: &str) -> Option<Self> {
        let arguments = tag_values(
            plist_section(text, "ProgramArguments", "<array>", "</array>")?,
            "string",
        );
        let [program, db_flag, db_path, command] = arguments.as_slice() else {
            return None;
        };
        if db_flag != "--db" || command != "daemon" {
            return None;
        }
        let env_section = plist_section(text, "EnvironmentVariables", "<dict>", "</dict>")?;
        let env: std::collections::HashMap<String, String> = tag_values(env_section, "key")
            .into_iter()
            .zip(tag_values(env_section, "string"))
            .collect();
        let mut spec = self.clone();
        spec.executable = PathBuf::from(program);
        spec.db_path = PathBuf::from(db_path);
        spec.path_env = env.get("PATH")?.clone();
        spec.config_dir = env.get("AVEN_CONFIG_DIR").map(PathBuf::from);
        spec.state_home = env.get("XDG_STATE_HOME").map(PathBuf::from);
        Some(spec)
    }

    /// This spec under the legacy label.
    fn legacy(&self) -> Self {
        let mut spec = self.clone();
        spec.label = LEGACY_LABEL.to_string();
        spec.plist_path = legacy_plist_path(&self.plist_path);
        spec.service_target = format!("{}/{LEGACY_LABEL}", self.domain);
        spec
    }

    /// The spec of the installed agent: under the legacy label while only
    /// its plist exists.
    fn installed(&self) -> Self {
        if installed_plist_path(self.plist_path.clone()) == self.plist_path {
            self.clone()
        } else {
            self.legacy()
        }
    }

    fn plist_path_str(&self) -> Result<&str> {
        self.plist_path
            .to_str()
            .with_context(|| format!("non-utf8 plist path {}", self.plist_path.display()))
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(std::env::current_dir()?.join(path))
}

/// Bundle paths come first: Login Items attributes the agent to Aven.app only
/// when the program path itself is inside the bundle, not a symlink to it.
#[cfg(target_os = "macos")]
fn stable_program_path(current_exe: &Path) -> PathBuf {
    let home = dirs::home_dir();
    let mut candidates = vec![
        Path::new("/Applications")
            .join(BUNDLE_NAME)
            .join(BUNDLE_EXECUTABLE),
        Path::new("/opt/homebrew/opt/aven")
            .join(BUNDLE_NAME)
            .join(BUNDLE_EXECUTABLE),
        Path::new("/usr/local/opt/aven")
            .join(BUNDLE_NAME)
            .join(BUNDLE_EXECUTABLE),
    ];
    if let Some(home) = &home {
        candidates.push(direct_install_bundle(home).join(BUNDLE_EXECUTABLE));
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/opt/aven/bin/aven"),
        PathBuf::from("/usr/local/opt/aven/bin/aven"),
        PathBuf::from("/opt/homebrew/bin/aven"),
        PathBuf::from("/usr/local/bin/aven"),
    ]);
    if let Some(home) = &home {
        candidates.push(home.join(".cargo/bin/aven"));
        candidates.push(home.join(".local/bin/aven"));
    }
    stable_program_path_from_candidates(current_exe, candidates)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn stable_program_path_from_candidates(
    current_exe: &Path,
    candidates: impl IntoIterator<Item = PathBuf>,
) -> PathBuf {
    candidates
        .into_iter()
        .find(|candidate| crate::paths::same_file(candidate, current_exe))
        .unwrap_or_else(|| current_exe.to_path_buf())
}

#[cfg(target_os = "macos")]
fn read_plist_program(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Ok(program_from_plist_text(&text).map(PathBuf::from))
}

#[cfg(target_os = "macos")]
fn program_from_plist_text(text: &str) -> Option<String> {
    let program_args_index = text.find("<key>ProgramArguments</key>")?;
    let after_program_args = &text[program_args_index..];
    let string_start = after_program_args.find("<string>")? + "<string>".len();
    let after_string = &after_program_args[string_start..];
    let string_end = after_string.find("</string>")?;
    Some(unescape_xml(&after_string[..string_end]))
}

/// The text between `open` and `close` following `<key>{key}</key>`.
#[cfg(target_os = "macos")]
fn plist_section<'a>(text: &'a str, key: &str, open: &str, close: &str) -> Option<&'a str> {
    let after_key = &text[text.find(&format!("<key>{key}</key>"))?..];
    let start = after_key.find(open)? + open.len();
    let end = start + after_key[start..].find(close)?;
    Some(&after_key[start..end])
}

#[cfg(target_os = "macos")]
fn tag_values(text: &str, tag: &str) -> Vec<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let mut values = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(&open) {
        let after_open = &rest[start + open.len()..];
        let Some(end) = after_open.find(&close) else {
            break;
        };
        values.push(unescape_xml(&after_open[..end]));
        rest = &after_open[end + close.len()..];
    }
    values
}

#[cfg(target_os = "macos")]
fn unescape_xml(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

#[cfg(target_os = "macos")]
fn service_is_loaded(runner: &impl LaunchctlRunner, spec: &ServiceSpec) -> Result<bool> {
    Ok(service_runtime(runner, spec)?.loaded)
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ServiceRuntime {
    loaded: bool,
    running: Option<bool>,
}

#[cfg(target_os = "macos")]
fn service_runtime(runner: &impl LaunchctlRunner, spec: &ServiceSpec) -> Result<ServiceRuntime> {
    let args = ["print", spec.service_target.as_str()];
    let output = runner.run(&args)?;
    if output.status.success() {
        let text = String::from_utf8_lossy(&output.stdout);
        return Ok(ServiceRuntime {
            loaded: true,
            running: Some(text.lines().any(|line| line.trim() == "state = running")),
        });
    }
    if output.status.code() == Some(113) {
        return Ok(ServiceRuntime {
            loaded: false,
            running: Some(false),
        });
    }
    Err(launchctl_failure(&args, &output))
}

#[cfg(target_os = "macos")]
fn reload_service(
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
    spec: &ServiceSpec,
    plist: &str,
) -> Result<()> {
    let legacy = spec.legacy();
    if spec.plist_path.exists() || legacy.plist_path.exists() {
        ensure_login_item_allowed(runner, spec)?;
    }
    let performed_bootout = stop_service(runner, sleeper, spec)?;
    // The legacy job stops before the new plist exists and its plist goes
    // after, so an interruption leaves one of them to load at login.
    stop_service(runner, sleeper, &legacy)?;
    write_plist(spec, plist)?;
    remove_plist(&legacy.plist_path)?;
    run_launchctl(runner, &["enable", &spec.service_target])?;
    bootstrap_service(runner, sleeper, spec, performed_bootout)
}

/// Stops the agent under the legacy label and removes its plist.
#[cfg(target_os = "macos")]
fn retire_legacy(
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
    spec: &ServiceSpec,
) -> Result<()> {
    let legacy = spec.legacy();
    stop_service(runner, sleeper, &legacy)?;
    remove_plist(&legacy.plist_path)?;
    Ok(())
}

/// Removes `path`, returning whether it existed.
#[cfg(target_os = "macos")]
fn remove_plist(path: &Path) -> Result<bool> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

/// Fails with [`TurnedOffInLoginItems`] when the user turned Aven off in
/// Login Items, for this agent or for the one under the legacy label that it
/// replaces. Login Items remembers that for the label, also across a removed
/// and rewritten plist.
#[cfg(target_os = "macos")]
fn ensure_login_item_allowed(runner: &impl LaunchctlRunner, spec: &ServiceSpec) -> Result<()> {
    let legacy = legacy_plist_path(&spec.plist_path);
    let turned_off = |plist: &Path| runner.login_item(plist) == LoginItem::TurnedOff;
    if turned_off(&spec.plist_path) || (legacy.exists() && turned_off(&legacy)) {
        return Err(TurnedOffInLoginItems.into());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn agent_status_with_runner(
    runner: &impl LaunchctlRunner,
    spec: &ServiceSpec,
) -> Result<AgentStatus> {
    let spec = &spec.installed();
    let runtime = service_runtime(runner, spec)?;
    Ok(AgentStatus {
        installed: spec.plist_path.exists(),
        program: read_plist_program(&spec.plist_path)?,
        loaded: runtime.loaded,
        running: runtime.running == Some(true),
        login_item: runner.login_item(&spec.plist_path),
    })
}

#[cfg(target_os = "macos")]
fn ensure_with_runner(
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
    spec: &ServiceSpec,
) -> Result<Ensured> {
    // An agent under the legacy label keeps its settings as it moves over.
    let installed = spec.installed().plist_path;
    let existing = match std::fs::read_to_string(&installed) {
        Ok(text) => Some(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", installed.display()));
        }
    };
    let spec = existing
        .as_deref()
        .and_then(|text| spec.with_plist_settings(text))
        .filter(|kept| {
            crate::paths::same_file(&kept.executable, &spec.executable)
                && kept.db_path == spec.db_path
        })
        .map(|kept| ServiceSpec {
            executable: spec.executable.clone(),
            ..kept
        })
        .unwrap_or_else(|| spec.clone());
    let plist = render_plist(&spec);
    let result = if existing.as_deref() == Some(plist.as_str())
        && !legacy_plist_path(&spec.plist_path).exists()
    {
        if service_is_loaded(runner, &spec)? {
            return Ok(Ensured::Unchanged);
        }
        ensure_login_item_allowed(runner, &spec).and_then(|()| {
            run_launchctl(runner, &["enable", &spec.service_target])?;
            bootstrap_service(runner, sleeper, &spec, false)
        })
    } else {
        reload_service(runner, sleeper, &spec, &plist)
    };
    match result {
        Ok(()) => Ok(Ensured::Started),
        Err(error) if error.is::<TurnedOffInLoginItems>() => Ok(Ensured::TurnedOff),
        Err(error) => Err(error),
    }
}

#[cfg(target_os = "macos")]
fn remove_with_runner(
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
    spec: &ServiceSpec,
    program: &Path,
) -> Result<bool> {
    let mut removed = false;
    for spec in [spec.legacy(), spec.clone()] {
        let Some(installed) = read_plist_program(&spec.plist_path)? else {
            continue;
        };
        if !crate::paths::same_path(&installed, program) {
            continue;
        }
        stop_service(runner, sleeper, &spec)?;
        remove_plist(&spec.plist_path)?;
        removed = true;
    }
    Ok(removed)
}

/// Boots out a loaded service and waits until launchd releases it. Returns
/// whether a bootout happened.
#[cfg(target_os = "macos")]
fn stop_service(
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
    spec: &ServiceSpec,
) -> Result<bool> {
    let loaded = service_is_loaded(runner, spec)?;
    if loaded {
        run_launchctl(runner, &["bootout", &spec.service_target])?;
        wait_for_service_absence(runner, sleeper, spec)?;
    }
    Ok(loaded)
}

#[cfg(target_os = "macos")]
fn wait_for_service_absence(
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
    spec: &ServiceSpec,
) -> Result<()> {
    for poll in 0..=BOOTOUT_POLL_LIMIT {
        if !service_is_loaded(runner, spec)? {
            return Ok(());
        }
        if poll == BOOTOUT_POLL_LIMIT {
            break;
        }
        sleeper.sleep(BOOTOUT_POLL_INTERVAL);
    }
    bail!(
        "error launchd-teardown-timeout target={} waited_ms={}",
        spec.service_target,
        BOOTOUT_POLL_INTERVAL.as_millis() * BOOTOUT_POLL_LIMIT as u128
    )
}

#[cfg(target_os = "macos")]
fn bootstrap_service(
    runner: &impl LaunchctlRunner,
    sleeper: &impl Sleeper,
    spec: &ServiceSpec,
    performed_bootout: bool,
) -> Result<()> {
    ensure_login_item_allowed(runner, spec)?;
    let args = ["bootstrap", spec.domain.as_str(), spec.plist_path_str()?];
    for retry in 0..=BOOTSTRAP_RETRY_DELAYS.len() {
        let output = runner.run(&args)?;
        if output.status.success() {
            return Ok(());
        }
        if !performed_bootout || output.status.code() != Some(5) {
            return Err(launchctl_failure(&args, &output));
        }
        if service_is_loaded(runner, spec)? {
            return Ok(());
        }
        let Some(delay) = BOOTSTRAP_RETRY_DELAYS.get(retry) else {
            return Err(launchctl_failure(&args, &output));
        };
        sleeper.sleep(*delay);
    }
    unreachable!("bootstrap retry loop returns on every terminal state")
}

#[cfg(target_os = "macos")]
fn launchctl_failure(args: &[&str], output: &Output) -> anyhow::Error {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    anyhow::anyhow!(
        "error launchctl-failed command={} status={} stdout={} stderr={}",
        args.join(" "),
        output.status,
        stdout.trim(),
        stderr.trim()
    )
}

#[cfg(target_os = "macos")]
fn run_launchctl(runner: &impl LaunchctlRunner, args: &[&str]) -> Result<()> {
    let output = runner.run(args)?;
    if output.status.success() {
        return Ok(());
    }
    Err(launchctl_failure(args, &output))
}

#[cfg(target_os = "macos")]
fn write_plist(spec: &ServiceSpec, plist: &str) -> Result<()> {
    if let Some(parent) = spec.plist_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create launch agents directory {}", parent.display()))?;
    }
    std::fs::create_dir_all(&spec.log_dir)
        .with_context(|| format!("create log directory {}", spec.log_dir.display()))?;
    let tmp = spec.plist_path.with_extension("plist.tmp");
    std::fs::write(&tmp, plist).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &spec.plist_path).with_context(|| {
        format!(
            "replace {} with {}",
            spec.plist_path.display(),
            tmp.display()
        )
    })?;
    Ok(())
}

/// How launchd decides to keep the daemon running.
#[cfg(target_os = "macos")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeepAlive {
    /// Restart after every exit, as LaunchAgents without a `PathState`
    /// condition do. A removed executable then fails to spawn every throttle
    /// interval.
    Always,
    /// Restart after every exit while the executable exists, and start again
    /// when it reappears, so moving Aven.app to the Trash stops the daemon
    /// and replacing it starts the new one.
    WhileProgramExists,
}

#[cfg(target_os = "macos")]
fn render_plist(spec: &ServiceSpec) -> String {
    render_plist_with(spec, KeepAlive::WhileProgramExists)
}

#[cfg(target_os = "macos")]
fn render_plist_with(spec: &ServiceSpec, keep_alive: KeepAlive) -> String {
    let mut env = vec![("PATH", spec.path_env.as_str())];
    if let Some(path) = &spec.config_dir {
        env.push(("AVEN_CONFIG_DIR", path_str(path)));
    }
    if let Some(path) = &spec.state_home {
        env.push(("XDG_STATE_HOME", path_str(path)));
    }

    let env_xml = env
        .into_iter()
        .map(|(key, value)| {
            format!(
                "    <key>{}</key>\n    <string>{}</string>\n",
                escape_xml(key),
                escape_xml(value)
            )
        })
        .collect::<String>();
    // Login Items lists the agent under the app's name and icon.
    let associated_bundle = if enclosing_bundle(&spec.executable).is_some() {
        format!(
            "  <key>AssociatedBundleIdentifiers</key>\n  <array>\n    <string>{AVEN_BUNDLE_ID}</string>\n  </array>\n"
        )
    } else {
        String::new()
    };
    let keep_alive = match keep_alive {
        KeepAlive::Always => "  <true/>\n".to_string(),
        KeepAlive::WhileProgramExists => format!(
            "  <dict>\n    <key>PathState</key>\n    <dict>\n      <key>{}</key>\n      <true/>\n    </dict>\n  </dict>\n",
            escape_xml(path_str(&spec.executable))
        ),
    };

    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
<plist version=\"1.0\">\n\
<dict>\n\
  <key>Label</key>\n\
  <string>{}</string>\n\
  <key>ProgramArguments</key>\n\
  <array>\n\
    <string>{}</string>\n\
    <string>--db</string>\n\
    <string>{}</string>\n\
    <string>daemon</string>\n\
  </array>\n\
  <key>EnvironmentVariables</key>\n\
  <dict>\n\
{}\
  </dict>\n\
  <key>RunAtLoad</key>\n\
  <true/>\n\
  <key>KeepAlive</key>\n\
{}\
  <key>ThrottleInterval</key>\n\
  <integer>30</integer>\n\
  <key>StandardOutPath</key>\n\
  <string>{}</string>\n\
  <key>StandardErrorPath</key>\n\
  <string>{}</string>\n\
{}\
</dict>\n\
</plist>\n",
        escape_xml(&spec.label),
        escape_xml(path_str(&spec.executable)),
        escape_xml(path_str(&spec.db_path)),
        env_xml,
        keep_alive,
        escape_xml(path_str(&spec.stdout_path)),
        escape_xml(path_str(&spec.stderr_path)),
        associated_bundle
    )
}

#[cfg(target_os = "macos")]
fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or("")
}

/// `SMAppService`'s view of a hand-written LaunchAgent, which is what Login
/// Items shows. Readable without privileges, unlike the BTM records.
#[cfg(target_os = "macos")]
mod login_items {
    use std::ffi::{CString, c_char, c_void};
    use std::os::unix::ffi::OsStrExt as _;
    use std::path::Path;

    use super::LoginItem;

    #[link(name = "ServiceManagement", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "Foundation", kind = "framework")]
    unsafe extern "C" {}
    #[link(name = "objc")]
    unsafe extern "C" {
        fn objc_getClass(name: *const c_char) -> *mut c_void;
        fn sel_registerName(name: *const c_char) -> *mut c_void;
        fn objc_msgSend();
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }

    /// `SMAppServiceStatusRequiresApproval`. Login Items reports a legacy
    /// agent the user turned off this way; never-seen, enabled and missing
    /// plists report the other statuses.
    const REQUIRES_APPROVAL: isize = 2;

    pub(super) fn status(plist: &Path) -> LoginItem {
        let Ok(path) = CString::new(plist.as_os_str().as_bytes()) else {
            return LoginItem::Unknown;
        };
        // SAFETY: each class and selector exists from macOS 13, checked by
        // `SMAppService` being present, and each `objc_msgSend` is cast to
        // the signature of the method it sends. The pool releases the
        // autoreleased string and URL.
        unsafe {
            let service = objc_getClass(c"SMAppService".as_ptr());
            if service.is_null() {
                return LoginItem::Unknown;
            }
            let pool = objc_autoreleasePoolPush();
            let send_ptr: extern "C" fn(*mut c_void, *mut c_void, *const c_char) -> *mut c_void =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_object: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> *mut c_void =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_status: extern "C" fn(*mut c_void, *mut c_void, *mut c_void) -> isize =
                std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let string = send_ptr(
                objc_getClass(c"NSString".as_ptr()),
                sel_registerName(c"stringWithUTF8String:".as_ptr()),
                path.as_ptr(),
            );
            let url = send_object(
                objc_getClass(c"NSURL".as_ptr()),
                sel_registerName(c"fileURLWithPath:".as_ptr()),
                string,
            );
            let status = if url.is_null() {
                None
            } else {
                Some(send_status(
                    service,
                    sel_registerName(c"statusForLegacyURL:".as_ptr()),
                    url,
                ))
            };
            objc_autoreleasePoolPop(pool);
            match status {
                Some(REQUIRES_APPROVAL) => LoginItem::TurnedOff,
                Some(_) => LoginItem::Allowed,
                None => LoginItem::Unknown,
            }
        }
    }
}

#[cfg(target_os = "macos")]
fn escape_xml(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            ch => escaped.push(ch),
        }
    }
    escaped
}

#[cfg(target_os = "macos")]
fn current_uid() -> u32 {
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

#[cfg(test)]
#[cfg(target_os = "macos")]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    use super::*;

    #[test]
    fn escapes_xml_text() {
        assert_eq!(escape_xml("<&>\"'"), "&lt;&amp;&gt;&quot;&apos;");
    }

    #[test]
    fn renders_daemon_plist() {
        let spec = ServiceSpec {
            label: "fi.zendit.Aven.daemon".to_string(),
            executable: PathBuf::from("/bin/aven&test"),
            db_path: PathBuf::from("/tmp/db.sqlite"),
            config_dir: Some(PathBuf::from("/tmp/config")),
            state_home: Some(PathBuf::from("/tmp/state")),
            path_env: "/usr/bin:/bin".to_string(),
            log_dir: PathBuf::from("/tmp/logs"),
            stdout_path: PathBuf::from("/tmp/logs/out.log"),
            stderr_path: PathBuf::from("/tmp/logs/err.log"),
            plist_path: PathBuf::from("/tmp/fi.zendit.Aven.daemon.plist"),
            domain: "gui/501".to_string(),
            service_target: "gui/501/fi.zendit.Aven.daemon".to_string(),
        };
        let plist = render_plist(&spec);
        assert!(plist.contains("<string>fi.zendit.Aven.daemon</string>"));
        assert!(plist.contains("<string>/bin/aven&amp;test</string>"));
        assert!(plist.contains("<string>--db</string>"));
        assert!(plist.contains("<string>/tmp/db.sqlite</string>"));
        assert!(plist.contains("<string>daemon</string>"));
        assert!(plist.contains("<key>AVEN_CONFIG_DIR</key>"));
        assert!(plist.contains("<key>XDG_STATE_HOME</key>"));
        assert!(plist.contains("<string>/tmp/state</string>"));
        assert!(plist.contains("<key>StandardErrorPath</key>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains(
            "<key>KeepAlive</key>\n  <dict>\n    <key>PathState</key>\n    <dict>\n      <key>/bin/aven&amp;test</key>\n      <true/>\n    </dict>\n  </dict>\n"
        ));
        assert!(!plist.contains("<key>SuccessfulExit</key>"));
        assert!(plist.contains("<key>ThrottleInterval</key>"));
        assert!(!plist.contains("AssociatedBundleIdentifiers"));
    }

    #[test]
    fn bundled_program_associates_the_agent_with_aven_app() {
        let mut spec = test_spec();
        spec.executable = PathBuf::from("/opt/homebrew/opt/aven/Aven.app/Contents/MacOS/aven");
        let plist = render_plist(&spec);
        assert!(plist.contains(
            "  <key>AssociatedBundleIdentifiers</key>\n  <array>\n    <string>fi.zendit.Aven</string>\n  </array>\n</dict>"
        ));
        assert_eq!(
            program_from_plist_text(&plist).as_deref(),
            Some("/opt/homebrew/opt/aven/Aven.app/Contents/MacOS/aven")
        );

        for bare in [
            "/opt/homebrew/bin/aven",
            "/x/Aven.app/Contents/Resources/aven",
        ] {
            spec.executable = PathBuf::from(bare);
            assert!(
                !render_plist(&spec).contains("AssociatedBundleIdentifiers"),
                "{bare}"
            );
        }
    }

    #[test]
    fn plist_with_bundle_association_is_valid_xml_plist() {
        let mut spec = test_spec();
        spec.executable = PathBuf::from("/Users/me/.local/share/aven/Aven.app/Contents/MacOS/aven");
        let path = spec.plist_path.clone();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, render_plist(&spec)).unwrap();
        let output = Command::new("/usr/bin/plutil")
            .args([
                "-extract",
                "AssociatedBundleIdentifiers.0",
                "raw",
                "-o",
                "-",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            "fi.zendit.Aven"
        );
        let output = Command::new("/usr/bin/plutil")
            .args(["-extract", "KeepAlive", "json", "-o", "-"])
            .arg(&path)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        let keep_alive: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            keep_alive,
            serde_json::json!({ "PathState": { path_str(&spec.executable): true } })
        );
    }

    #[test]
    fn reload_bootstraps_an_absent_service_without_waiting() {
        let runner = FakeRunner::new(vec![absent(), absent(), success(), success()]);
        let sleeper = RecordingSleeper::default();
        let spec = test_spec();

        reload_service(&runner, &sleeper, &spec, &render_plist(&spec)).unwrap();

        assert_eq!(
            runner.commands(),
            vec![
                vec!["print", "gui/501/fi.zendit.Aven.daemon"],
                vec!["print", LEGACY_TARGET],
                vec!["enable", "gui/501/fi.zendit.Aven.daemon"],
                vec!["bootstrap", "gui/501", spec.plist_path_str().unwrap()],
            ]
        );
        assert!(sleeper.durations().is_empty());
    }

    #[test]
    fn reload_waits_for_bootout_to_release_the_service() {
        let runner = FakeRunner::new(vec![
            success(),
            success(),
            success(),
            absent(),
            absent(),
            success(),
            success(),
        ]);
        let sleeper = RecordingSleeper::default();
        let spec = test_spec();

        reload_service(&runner, &sleeper, &spec, &render_plist(&spec)).unwrap();

        assert_eq!(
            runner.commands(),
            vec![
                vec!["print", "gui/501/fi.zendit.Aven.daemon"],
                vec!["bootout", "gui/501/fi.zendit.Aven.daemon"],
                vec!["print", "gui/501/fi.zendit.Aven.daemon"],
                vec!["print", "gui/501/fi.zendit.Aven.daemon"],
                vec!["print", LEGACY_TARGET],
                vec!["enable", "gui/501/fi.zendit.Aven.daemon"],
                vec!["bootstrap", "gui/501", spec.plist_path_str().unwrap()],
            ]
        );
        assert_eq!(sleeper.durations(), vec![BOOTOUT_POLL_INTERVAL]);
    }

    #[test]
    fn reload_retries_status_five_after_bootout() {
        let runner = FakeRunner::new(vec![
            success(),
            success(),
            absent(),
            absent(),
            success(),
            failure_with_status(5),
            absent(),
            success(),
        ]);
        let sleeper = RecordingSleeper::default();
        let spec = test_spec();

        reload_service(&runner, &sleeper, &spec, &render_plist(&spec)).unwrap();

        assert_eq!(
            runner
                .commands()
                .iter()
                .filter(|args| args.first().map(String::as_str) == Some("bootstrap"))
                .count(),
            2
        );
        assert_eq!(sleeper.durations(), vec![BOOTSTRAP_RETRY_DELAYS[0]]);
    }

    #[test]
    fn fresh_bootstrap_status_five_fails_without_retrying() {
        let runner = FakeRunner::new(vec![absent(), absent(), success(), failure_with_status(5)]);
        let sleeper = RecordingSleeper::default();
        let spec = test_spec();

        let error = reload_service(&runner, &sleeper, &spec, &render_plist(&spec)).unwrap_err();

        assert!(error.to_string().contains("status=exit status: 5"));
        assert_eq!(
            runner
                .commands()
                .iter()
                .filter(|args| args.first().map(String::as_str) == Some("bootstrap"))
                .count(),
            1
        );
        assert!(sleeper.durations().is_empty());
    }

    #[test]
    fn reload_bounds_the_bootout_wait() {
        let mut outputs = vec![success(), success()];
        outputs.extend(std::iter::repeat_with(success).take(BOOTOUT_POLL_LIMIT + 1));
        let runner = FakeRunner::new(outputs);
        let sleeper = RecordingSleeper::default();
        let spec = test_spec();

        let error = reload_service(&runner, &sleeper, &spec, &render_plist(&spec)).unwrap_err();

        assert!(error.to_string().contains("launchd-teardown-timeout"));
        assert_eq!(sleeper.durations().len(), BOOTOUT_POLL_LIMIT);
        assert!(
            runner
                .commands()
                .iter()
                .all(|args| args.first().map(String::as_str) != Some("bootstrap"))
        );
    }

    #[test]
    fn reload_bounds_status_five_retries() {
        let mut outputs = vec![success(), success(), absent(), absent(), success()];
        for _ in 0..=BOOTSTRAP_RETRY_DELAYS.len() {
            outputs.push(failure_with_status(5));
            outputs.push(absent());
        }
        let runner = FakeRunner::new(outputs);
        let sleeper = RecordingSleeper::default();
        let spec = test_spec();

        let error = reload_service(&runner, &sleeper, &spec, &render_plist(&spec)).unwrap_err();

        assert!(error.to_string().contains("status=exit status: 5"));
        assert_eq!(sleeper.durations(), BOOTSTRAP_RETRY_DELAYS);
        assert_eq!(
            runner
                .commands()
                .iter()
                .filter(|args| args.first().map(String::as_str) == Some("bootstrap"))
                .count(),
            BOOTSTRAP_RETRY_DELAYS.len() + 1
        );
    }

    #[test]
    fn service_runtime_distinguishes_running_loaded_and_absent() {
        let spec = test_spec();
        let running = FakeRunner::new(vec![success_with_stdout("state = running\n")]);
        assert_eq!(
            service_runtime(&running, &spec).unwrap(),
            ServiceRuntime {
                loaded: true,
                running: Some(true),
            }
        );
        let loaded = FakeRunner::new(vec![success_with_stdout("state = waiting\n")]);
        assert_eq!(
            service_runtime(&loaded, &spec).unwrap(),
            ServiceRuntime {
                loaded: true,
                running: Some(false),
            }
        );
        let absent = FakeRunner::new(vec![absent()]);
        assert_eq!(
            service_runtime(&absent, &spec).unwrap(),
            ServiceRuntime {
                loaded: false,
                running: Some(false),
            }
        );
    }

    #[test]
    fn restart_uses_kickstart() {
        let runner = FakeRunner::new(vec![success()]);
        let spec = test_spec();
        run_launchctl(&runner, &["kickstart", "-k", &spec.service_target]).unwrap();
        assert_eq!(
            runner.commands(),
            vec![vec!["kickstart", "-k", "gui/501/fi.zendit.Aven.daemon"]]
        );
    }

    #[test]
    fn extracts_program_from_plist_text() {
        let spec = test_spec();
        let plist = render_plist(&spec);
        assert_eq!(
            program_from_plist_text(&plist).as_deref(),
            Some("/bin/aven")
        );
    }

    #[test]
    fn stable_program_path_prefers_matching_installed_path() {
        let dir = tempfile::tempdir().unwrap();
        let current = dir.path().join("cellar/aven/1.0.0/bin/aven");
        let stable = dir.path().join("opt/aven/bin/aven");
        std::fs::create_dir_all(current.parent().unwrap()).unwrap();
        std::fs::create_dir_all(stable.parent().unwrap()).unwrap();
        std::fs::write(&current, "aven").unwrap();
        std::os::unix::fs::symlink(&current, &stable).unwrap();
        assert_eq!(
            stable_program_path_from_candidates(&current, vec![stable.clone()]),
            stable
        );
    }

    #[test]
    fn stable_program_path_prefers_the_bundle_over_bin_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let cellar_bundle = dir.path().join("Cellar/aven/1.0.0/Aven.app");
        let executable = cellar_bundle.join(BUNDLE_EXECUTABLE);
        std::fs::create_dir_all(executable.parent().unwrap()).unwrap();
        std::fs::write(&executable, "aven").unwrap();
        let opt = dir.path().join("opt/aven");
        std::fs::create_dir_all(opt.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(cellar_bundle.parent().unwrap(), &opt).unwrap();
        let bin = dir.path().join("bin/aven");
        std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&executable, &bin).unwrap();
        let opt_bundle = opt.join(BUNDLE_NAME).join(BUNDLE_EXECUTABLE);

        // Launched through the PATH symlink, the program still points inside the bundle.
        assert_eq!(
            stable_program_path_from_candidates(&bin, vec![opt_bundle.clone(), bin.clone()]),
            opt_bundle
        );
        // A bare executable never matches a bundle left beside it.
        let bare = dir.path().join("bare/aven");
        std::fs::create_dir_all(bare.parent().unwrap()).unwrap();
        std::fs::write(&bare, "aven").unwrap();
        assert_eq!(
            stable_program_path_from_candidates(&bare, vec![opt_bundle, bin]),
            bare
        );
    }

    #[test]
    fn program_from_plist_text_unescapes_xml() {
        let plist = "<key>ProgramArguments</key><array><string>/bin/aven&amp;test</string></array>";
        assert_eq!(
            program_from_plist_text(plist).as_deref(),
            Some("/bin/aven&test")
        );
    }

    /// A Homebrew-style layout: `bin/aven` links to the bundle executable.
    fn linked_bundle() -> (PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap().keep();
        let program = dir.join("opt/aven/Aven.app/Contents/MacOS/aven");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, "aven").unwrap();
        let link = dir.join("opt/aven/bin/aven");
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&program, &link).unwrap();
        (link, program)
    }

    #[test]
    fn bundle_migration_moves_the_program_and_keeps_settings() {
        let (link, program) = linked_bundle();
        let base = test_spec();
        let mut installed = base.clone();
        installed.executable = link;
        installed.db_path = PathBuf::from("/data/a&b.sqlite");
        installed.path_env = "/custom/bin:/usr/bin".to_string();
        installed.config_dir = Some(PathBuf::from("/config/aven"));
        installed.state_home = Some(PathBuf::from("/state"));
        let existing = render_plist(&installed);

        // The running process may see different settings; the plist wins.
        let migrated = bundle_migration_plist(&base, &base.label, &existing, &program)
            .unwrap()
            .unwrap();

        let mut expected = installed.clone();
        expected.executable = program.clone();
        assert_eq!(migrated, render_plist(&expected));
        assert!(migrated.contains("AssociatedBundleIdentifiers"));
        assert_eq!(
            bundle_migration_plist(&base, &base.label, &migrated, &program),
            Ok(None)
        );

        // An agent under the legacy label moves to the label as well, also
        // when it already runs from the bundle.
        let as_legacy = |spec: &ServiceSpec| {
            render_plist(&ServiceSpec {
                label: LEGACY_LABEL.to_string(),
                ..spec.clone()
            })
        };
        assert_eq!(
            bundle_migration_plist(&base, LEGACY_LABEL, &as_legacy(&installed), &program),
            Ok(Some(migrated.clone()))
        );
        assert_eq!(
            bundle_migration_plist(&base, LEGACY_LABEL, &as_legacy(&expected), &program),
            Ok(Some(migrated.clone()))
        );
        assert!(migrated.contains("<string>fi.zendit.Aven.daemon</string>"));

        // Agents installed before KeepAlive followed the program move too.
        let legacy = render_plist_with(&installed, KeepAlive::Always);
        assert!(legacy.contains("<key>KeepAlive</key>\n  <true/>\n"));
        assert_eq!(
            bundle_migration_plist(&base, &base.label, &legacy, &program),
            Ok(Some(migrated))
        );
    }

    #[test]
    fn bundle_migration_skips_bare_targets_and_foreign_plists() {
        let (link, program) = linked_bundle();
        let base = test_spec();
        let mut installed = base.clone();
        installed.executable = link;
        let existing = render_plist(&installed);

        assert_eq!(
            bundle_migration_plist(
                &base,
                &base.label,
                &existing,
                Path::new("/opt/homebrew/bin/aven")
            ),
            Ok(None)
        );
        let customized = existing.replacen(
            "<key>RunAtLoad</key>",
            "<key>Nice</key>\n<integer>5</integer>\n<key>RunAtLoad</key>",
            1,
        );
        assert_ne!(customized, existing);
        assert_eq!(
            bundle_migration_plist(&base, &base.label, &customized, &program),
            Err("customized-plist")
        );
        let extra_argument = existing.replace(
            "<string>daemon</string>",
            "<string>daemon</string>\n    <string>--verbose</string>",
        );
        assert_eq!(
            bundle_migration_plist(&base, &base.label, &extra_argument, &program),
            Err("unrecognized-plist")
        );
        let (_, other_program) = linked_bundle();
        assert_eq!(
            bundle_migration_plist(&base, &base.label, &existing, &other_program),
            Err("different-program")
        );
    }

    #[test]
    fn disabled_services_are_detected_in_both_launchctl_formats() {
        let spec = test_spec();
        for (listing, disabled) in [
            ("\t\"fi.zendit.Aven.daemon\" => disabled\n", true),
            ("\t\"fi.zendit.Aven.daemon\" => true\n", true),
            ("\t\"fi.zendit.Aven.daemon\" => enabled\n", false),
            ("\t\"fi.zendit.Aven.daemon.other\" => disabled\n", false),
            ("", false),
        ] {
            let runner = FakeRunner::new(vec![success_with_stdout(listing)]);
            assert_eq!(
                service_is_disabled(&runner, &spec).unwrap(),
                disabled,
                "{listing}"
            );
            assert_eq!(runner.commands(), [["print-disabled", "gui/501"]]);
        }
    }

    #[test]
    fn ensure_installs_and_starts_a_missing_agent() {
        let spec = test_spec();
        let runner = FakeRunner::new(vec![absent(), absent(), success(), success()]);
        let sleeper = RecordingSleeper::default();
        assert_eq!(
            ensure_with_runner(&runner, &sleeper, &spec).unwrap(),
            Ensured::Started
        );
        assert_eq!(
            std::fs::read_to_string(&spec.plist_path).unwrap(),
            render_plist(&spec)
        );
        assert_eq!(
            runner.commands(),
            [
                vec!["print", "gui/501/fi.zendit.Aven.daemon"],
                vec!["print", LEGACY_TARGET],
                vec!["enable", "gui/501/fi.zendit.Aven.daemon"],
                vec!["bootstrap", "gui/501", spec.plist_path.to_str().unwrap()],
            ]
        );
    }

    #[test]
    fn ensure_leaves_a_current_loaded_agent_and_keeps_its_environment() {
        let mut spec = test_spec();
        spec.executable = PathBuf::from("/bin/sh");
        let mut installed = spec.clone();
        installed.path_env = "/custom/bin:/usr/bin".to_string();
        write_plist(&installed, &render_plist(&installed)).unwrap();
        let runner = FakeRunner::new(vec![success()]);
        assert_eq!(
            ensure_with_runner(&runner, &RecordingSleeper::default(), &spec).unwrap(),
            Ensured::Unchanged
        );
        assert_eq!(
            std::fs::read_to_string(&spec.plist_path).unwrap(),
            render_plist(&installed)
        );
        assert_eq!(
            runner.commands(),
            [["print", "gui/501/fi.zendit.Aven.daemon"]]
        );
    }

    #[test]
    fn ensure_never_starts_an_agent_turned_off_in_login_items() {
        let spec = test_spec();
        write_plist(&spec, &render_plist(&spec)).unwrap();
        let runner = FakeRunner::new(vec![absent()]).turned_off();
        let sleeper = RecordingSleeper::default();
        assert_eq!(
            ensure_with_runner(&runner, &sleeper, &spec).unwrap(),
            Ensured::TurnedOff
        );
        assert_eq!(
            runner.commands(),
            [["print", "gui/501/fi.zendit.Aven.daemon"]]
        );

        let stale = "<plist/>";
        std::fs::write(&spec.plist_path, stale).unwrap();
        let runner = FakeRunner::new(vec![]).turned_off();
        assert_eq!(
            ensure_with_runner(&runner, &sleeper, &spec).unwrap(),
            Ensured::TurnedOff
        );
        assert!(runner.commands().is_empty());
        assert_eq!(std::fs::read_to_string(&spec.plist_path).unwrap(), stale);
    }

    #[test]
    fn bootstrap_refuses_an_agent_turned_off_in_login_items() {
        let spec = test_spec();
        let runner = FakeRunner::new(vec![]).turned_off();
        let error =
            bootstrap_service(&runner, &RecordingSleeper::default(), &spec, true).unwrap_err();
        assert!(error.is::<TurnedOffInLoginItems>(), "{error:#}");
        assert!(error.to_string().contains("Login Items"));
        assert!(runner.commands().is_empty());
    }

    #[test]
    fn remove_takes_only_the_given_programs_agent_and_keeps_launchds_enablement() {
        let spec = test_spec();
        write_plist(&spec, &render_plist(&spec)).unwrap();
        let sleeper = RecordingSleeper::default();
        let runner = FakeRunner::new(vec![]);
        assert!(!remove_with_runner(&runner, &sleeper, &spec, Path::new("/other/aven")).unwrap());
        assert!(spec.plist_path.exists());
        assert!(runner.commands().is_empty());

        let runner = FakeRunner::new(vec![success(), success(), absent()]);
        assert!(remove_with_runner(&runner, &sleeper, &spec, &spec.executable).unwrap());
        assert!(!spec.plist_path.exists());
        assert_eq!(
            runner.commands(),
            [
                ["print", "gui/501/fi.zendit.Aven.daemon"],
                ["bootout", "gui/501/fi.zendit.Aven.daemon"],
                ["print", "gui/501/fi.zendit.Aven.daemon"],
            ]
        );
        assert!(!remove_with_runner(&runner, &sleeper, &spec, &spec.executable).unwrap());
    }

    const LEGACY_TARGET: &str = "gui/501/com.raine.aven.daemon";
    const TARGET: &str = "gui/501/fi.zendit.Aven.daemon";

    /// A spec whose only installed agent is under the legacy label.
    fn legacy_install() -> (ServiceSpec, ServiceSpec) {
        let spec = test_spec();
        let legacy = spec.legacy();
        write_plist(&legacy, &render_plist(&legacy)).unwrap();
        (spec, legacy)
    }

    #[test]
    fn ensure_moves_a_legacy_agent_to_the_label_and_keeps_its_settings() {
        let mut spec = test_spec();
        spec.executable = PathBuf::from("/bin/sh");
        let mut legacy = spec.legacy();
        legacy.path_env = "/custom/bin:/usr/bin".to_string();
        legacy.config_dir = Some(PathBuf::from("/config/aven"));
        write_plist(&legacy, &render_plist_with(&legacy, KeepAlive::Always)).unwrap();
        let runner = FakeRunner::new(vec![absent(), success(), success(), absent()]);
        let sleeper = RecordingSleeper::default();

        assert_eq!(
            ensure_with_runner(&runner, &sleeper, &spec).unwrap(),
            Ensured::Started
        );

        let expected = ServiceSpec {
            path_env: legacy.path_env.clone(),
            config_dir: legacy.config_dir.clone(),
            ..spec.clone()
        };
        assert_eq!(
            std::fs::read_to_string(&spec.plist_path).unwrap(),
            render_plist(&expected)
        );
        assert!(!legacy.plist_path.exists());
        assert_eq!(
            runner.commands(),
            [
                vec!["print", TARGET],
                vec!["print", LEGACY_TARGET],
                vec!["bootout", LEGACY_TARGET],
                vec!["print", LEGACY_TARGET],
                vec!["enable", TARGET],
                vec!["bootstrap", "gui/501", spec.plist_path_str().unwrap()],
            ]
        );
    }

    #[test]
    fn a_legacy_agent_turned_off_in_login_items_stays_off() {
        let (spec, legacy) = legacy_install();
        let runner = FakeRunner::new(vec![]).turned_off_at(&legacy.plist_path);

        assert_eq!(
            ensure_with_runner(&runner, &RecordingSleeper::default(), &spec).unwrap(),
            Ensured::TurnedOff
        );

        assert!(runner.commands().is_empty());
        assert!(legacy.plist_path.exists());
        assert!(!spec.plist_path.exists());
    }

    #[test]
    fn status_and_restart_address_the_legacy_agent_until_it_moves() {
        let (spec, legacy) = legacy_install();

        let runner = FakeRunner::new(vec![success_with_stdout("state = running\n")]);
        assert_eq!(
            agent_status_with_runner(&runner, &spec).unwrap(),
            AgentStatus {
                installed: true,
                program: Some(legacy.executable.clone()),
                loaded: true,
                running: true,
                login_item: LoginItem::Allowed,
            }
        );
        assert_eq!(runner.commands(), [["print", LEGACY_TARGET]]);

        let runner = FakeRunner::new(vec![success()]);
        restart_with_runner(&runner, &spec).unwrap();
        assert_eq!(runner.commands(), [["kickstart", "-k", LEGACY_TARGET]]);
        assert!(legacy.plist_path.exists());
        assert!(!spec.plist_path.exists());
    }

    #[test]
    fn uninstall_removes_the_agent_under_both_labels() {
        let spec = test_spec();
        write_plist(&spec, &render_plist(&spec)).unwrap();
        let legacy = spec.legacy();
        write_plist(&legacy, &render_plist(&legacy)).unwrap();
        let runner = FakeRunner::new(vec![success(), success(), success(), success()]);

        uninstall_with_runner(&runner, &spec).unwrap();

        assert!(!spec.plist_path.exists());
        assert!(!legacy.plist_path.exists());
        assert_eq!(
            runner.commands(),
            [
                vec!["print", TARGET],
                vec!["bootout", TARGET],
                vec!["print", LEGACY_TARGET],
                vec!["bootout", LEGACY_TARGET],
                vec!["disable", TARGET],
            ]
        );
    }

    #[test]
    fn a_legacy_daemon_moves_itself_to_the_label() {
        let (spec, legacy) = legacy_install();
        let sleeper = RecordingSleeper::default();

        // The daemon writes the renamed plist; launchd still runs the legacy job.
        let runner = FakeRunner::new(vec![success()]);
        assert_eq!(
            prepare_bundle_migration_with_runner(
                &runner,
                &sleeper,
                &spec,
                LEGACY_LABEL,
                &legacy.executable,
            )
            .unwrap(),
            BundleMigration::Prepared {
                program: legacy.executable.clone(),
            }
        );
        assert_eq!(runner.commands(), [["print-disabled", "gui/501"]]);
        assert_eq!(
            std::fs::read_to_string(&spec.plist_path).unwrap(),
            render_plist(&spec)
        );
        assert!(legacy.plist_path.exists());

        // The reload it starts swaps the legacy job for the renamed one.
        let runner = FakeRunner::new(vec![absent(), success(), success(), absent(), success()]);
        reload_definition_with_runner(&runner, &sleeper, &spec).unwrap();
        assert!(!legacy.plist_path.exists());
        assert_eq!(
            runner.commands(),
            [
                vec!["print", TARGET],
                vec!["print", LEGACY_TARGET],
                vec!["bootout", LEGACY_TARGET],
                vec!["print", LEGACY_TARGET],
                vec!["bootstrap", "gui/501", spec.plist_path_str().unwrap()],
            ]
        );
    }

    fn test_spec() -> ServiceSpec {
        let dir = tempfile::tempdir().unwrap().keep();
        ServiceSpec {
            label: LABEL.to_string(),
            executable: PathBuf::from("/bin/aven"),
            db_path: PathBuf::from("/tmp/db.sqlite"),
            config_dir: None,
            state_home: None,
            path_env: DEFAULT_PATH.to_string(),
            log_dir: dir.join("logs"),
            stdout_path: dir.join("logs/out.log"),
            stderr_path: dir.join("logs/err.log"),
            plist_path: dir.join(format!("LaunchAgents/{LABEL}.plist")),
            domain: "gui/501".to_string(),
            service_target: format!("gui/501/{LABEL}"),
        }
    }

    struct FakeRunner {
        outputs: Mutex<Vec<Output>>,
        commands: Mutex<Vec<Vec<String>>>,
        login_item: LoginItem,
        turned_off_plist: Option<PathBuf>,
    }

    #[derive(Default)]
    struct RecordingSleeper {
        durations: Mutex<Vec<Duration>>,
    }

    impl RecordingSleeper {
        fn durations(&self) -> Vec<Duration> {
            self.durations.lock().unwrap().clone()
        }
    }

    impl Sleeper for RecordingSleeper {
        fn sleep(&self, duration: Duration) {
            self.durations.lock().unwrap().push(duration);
        }
    }

    impl FakeRunner {
        fn new(mut outputs: Vec<Output>) -> Self {
            outputs.reverse();
            Self {
                outputs: Mutex::new(outputs),
                commands: Mutex::new(Vec::new()),
                login_item: LoginItem::Allowed,
                turned_off_plist: None,
            }
        }

        fn turned_off(mut self) -> Self {
            self.login_item = LoginItem::TurnedOff;
            self
        }

        /// Login Items has only the agent at `plist` turned off.
        fn turned_off_at(mut self, plist: &Path) -> Self {
            self.turned_off_plist = Some(plist.to_path_buf());
            self
        }

        fn commands(&self) -> Vec<Vec<String>> {
            self.commands.lock().unwrap().clone()
        }
    }

    impl LaunchctlRunner for FakeRunner {
        fn run(&self, args: &[&str]) -> Result<Output> {
            self.commands
                .lock()
                .unwrap()
                .push(args.iter().map(|arg| arg.to_string()).collect());
            Ok(self.outputs.lock().unwrap().pop().unwrap_or_else(success))
        }

        fn login_item(&self, plist: &Path) -> LoginItem {
            if self.turned_off_plist.as_deref() == Some(plist) {
                LoginItem::TurnedOff
            } else {
                self.login_item
            }
        }
    }

    fn success() -> Output {
        success_with_stdout("")
    }

    fn success_with_stdout(stdout: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(0),
            stdout: stdout.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    fn absent() -> Output {
        failure_with_status(113)
    }

    fn failure_with_status(status: i32) -> Output {
        Output {
            status: ExitStatus::from_raw(status << 8),
            stdout: Vec::new(),
            stderr: b"launchctl failure".to_vec(),
        }
    }
}
