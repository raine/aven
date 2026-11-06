//! Moves the `aven` command and the daemon onto the Desktop app in
//! `/Applications` from a terminal installation: the install script's bundle,
//! an older bare binary it installed, or the Homebrew formula.
//!
//! Nothing here opens the task database. Tasks, sync setup and the daemon's
//! settings stay where they are; only the executable they run changes. Files
//! Setup cannot attribute to an Aven installation are never changed.
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::app_bundle::{
    AVEN_BUNDLE_ID, BUNDLE_EXECUTABLE, BUNDLE_NAME, direct_install_bundle, enclosing_bundle,
};
use crate::daemon::service::{self, AgentState};

/// What the user runs to remove the Homebrew formula. Setup never runs it.
pub const FORMULA_UNINSTALL: &str = "brew uninstall raine/aven/aven";
/// What links the cask's `aven` command once the formula no longer holds
/// Homebrew's bin entry. Homebrew skips a cask's link that a formula owns.
pub const CASK_RELINK: &str = "brew reinstall --cask raine/aven/aven";

const STATE_VERSION: u32 = 1;
const SHELL_PATH_TIMEOUT: Duration = Duration::from_secs(5);
const PATH_MARKER: &str = "__AVEN_PATH__";

/// The paths Setup reads and changes.
#[derive(Debug, Clone)]
pub struct Layout {
    pub home: PathBuf,
    /// Where Desktop belongs: `/Applications/Aven.app`.
    pub app: PathBuf,
    /// The executable of the running app.
    pub running: PathBuf,
    /// The login shell's `PATH` in order, when known.
    pub path: Option<Vec<PathBuf>>,
    pub brew_prefixes: Vec<PathBuf>,
    pub agent_plist: PathBuf,
    /// Records a switch in progress, so it resumes after an interruption.
    pub state_file: PathBuf,
}

/// Where Desktop is installed. A debug build can be pointed at a scratch
/// directory instead, to run the installed layout without the real app.
fn applications() -> PathBuf {
    #[cfg(debug_assertions)]
    if let Some(dir) = std::env::var_os("AVEN_DEV_APPLICATIONS").filter(|dir| !dir.is_empty()) {
        return PathBuf::from(dir);
    }
    PathBuf::from("/Applications")
}

impl Layout {
    pub fn current() -> Result<Self> {
        let home = dirs::home_dir().context("could not find home directory")?;
        Ok(Self {
            app: applications().join(BUNDLE_NAME),
            running: std::env::current_exe().context("resolve current executable")?,
            path: None,
            brew_prefixes: vec!["/opt/homebrew".into(), "/usr/local".into()],
            agent_plist: service::agent_plist_path(&home),
            // Not under XDG_STATE_HOME: the app and a terminal must agree on
            // it whatever their environment.
            state_file: home.join(".local/state/aven/desktop-setup.json"),
            home,
        })
    }

    /// Desktop's executable, which the command and the daemon run.
    pub fn executable(&self) -> PathBuf {
        self.app.join(BUNDLE_EXECUTABLE)
    }

    /// Where Desktop links the `aven` command when nothing else provides it.
    pub fn command_link(&self) -> PathBuf {
        self.home.join(".local/bin/aven")
    }

    pub fn script_bundle(&self) -> PathBuf {
        direct_install_bundle(&self.home)
    }

    /// Whether the running app is the one in `/Applications`, not a copy in
    /// Downloads or on a disk image.
    pub fn in_place(&self) -> bool {
        same_file(&self.running, &self.executable())
    }

    /// Where to look for `aven`: the login shell's `PATH` first, then the
    /// directories installers use, marked as not on `PATH`.
    fn search_dirs(&self) -> Vec<(PathBuf, bool)> {
        let mut dirs: Vec<(PathBuf, bool)> = Vec::new();
        let mut add = |dir: PathBuf, on_path: bool| {
            if dir.is_absolute() && !dirs.iter().any(|(seen, _)| same_dir(seen, &dir)) {
                dirs.push((dir, on_path));
            }
        };
        for dir in self.path.iter().flatten() {
            add(dir.clone(), true);
        }
        add(self.home.join(".local/bin"), false);
        add(self.home.join(".cargo/bin"), false);
        add(self.home.join("bin"), false);
        for prefix in &self.brew_prefixes {
            add(prefix.join("bin"), false);
        }
        dirs
    }

    fn is_formula_path(&self, path: &Path) -> bool {
        self.brew_prefixes.iter().any(|prefix| {
            under(path, &prefix.join("Cellar/aven")) || under(path, &prefix.join("opt/aven"))
        })
    }

    fn in_brew_bin(&self, path: &Path) -> bool {
        path.parent().is_some_and(|dir| {
            self.brew_prefixes
                .iter()
                .any(|prefix| same_dir(dir, &prefix.join("bin")))
        })
    }

    fn formula_installed(&self) -> bool {
        self.brew_prefixes
            .iter()
            .any(|prefix| prefix.join("Cellar/aven").exists())
    }

    fn cask_installed(&self) -> bool {
        self.brew_prefixes
            .iter()
            .any(|prefix| prefix.join("Caskroom/aven").exists())
    }
}

/// The parts of the system Setup changes besides files.
pub trait System: Send + Sync {
    /// The code signing identifier of an executable, read without running it.
    fn signing_identifier(&self, path: &Path) -> Option<String>;
    fn agent(&self) -> Result<AgentState>;
    fn stop_agent(&self) -> Result<()>;
    fn start_agent(&self) -> Result<()>;
    /// Drops a bundle from Launch Services, so it no longer competes for the
    /// bundle identifier.
    fn unregister(&self, bundle: &Path) -> Result<()>;
}

/// The running Mac.
pub struct MacSystem;

impl System for MacSystem {
    fn signing_identifier(&self, path: &Path) -> Option<String> {
        let output = Command::new("/usr/bin/codesign")
            .arg("-dv")
            .arg(path)
            .stdin(Stdio::null())
            .output()
            .ok()?;
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .find_map(|line| line.strip_prefix("Identifier="))
            .map(str::to_string)
    }

    fn agent(&self) -> Result<AgentState> {
        service::agent_state()
    }

    fn stop_agent(&self) -> Result<()> {
        service::stop_agent()
    }

    fn start_agent(&self) -> Result<()> {
        service::start_agent()
    }

    fn unregister(&self, bundle: &Path) -> Result<()> {
        const LSREGISTER: &str = "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister";
        let status = Command::new(LSREGISTER)
            .arg("-u")
            .arg(bundle)
            .status()
            .context("run lsregister")?;
        if !status.success() {
            bail!("lsregister -u {} failed: {status}", bundle.display());
        }
        Ok(())
    }
}

/// Who put an `aven` executable where it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Owner {
    /// A link to Desktop's executable.
    Desktop,
    /// The cask's link to Desktop's executable in Homebrew's bin.
    Cask,
    /// The install script's bundle, or an older bare binary signed as Aven.
    Script,
    Formula,
    /// Signed as Aven, in a place no Aven installer uses.
    UnknownAven,
    /// Some other program named `aven`.
    Other,
}

impl Owner {
    pub fn is_desktop(self) -> bool {
        matches!(self, Owner::Desktop | Owner::Cask)
    }

    fn is_terminal_install(self) -> bool {
        matches!(self, Owner::Script | Owner::Formula)
    }
}

/// An `aven` found in a searched directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub owner: Owner,
    /// Whether the directory is on the login shell's `PATH`.
    pub on_path: bool,
}

/// The daemon's LaunchAgent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Agent {
    pub program: PathBuf,
    pub owner: Owner,
    pub loaded: bool,
    pub disabled: bool,
}

/// What is installed now.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Survey {
    /// In `PATH` order, then the other searched directories.
    pub commands: Vec<Found>,
    pub agent: Option<Agent>,
    /// The plist exists but names no program Setup can read.
    pub unreadable_agent: bool,
    pub script_bundle: Option<PathBuf>,
    pub formula: bool,
    pub cask: bool,
}

impl Survey {
    fn terminal_install(&self) -> bool {
        self.script_bundle.is_some()
            || self.formula
            || self
                .commands
                .iter()
                .any(|found| found.owner.is_terminal_install())
            || self
                .agent
                .as_ref()
                .is_some_and(|agent| agent.owner.is_terminal_install())
    }

    fn desktop_command(&self) -> Option<&Found> {
        self.commands.iter().find(|found| found.owner.is_desktop())
    }
}

/// What Setup does with what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// No terminal installation. Desktop may still need its command link.
    Ready,
    /// This copy is not `/Applications/Aven.app`, so nothing is changed.
    MoveToApplications {
        running: PathBuf,
    },
    Switch(Switch),
    Blocked(Blocked),
}

/// A switch from a terminal installation, which needs consent unless it is
/// resuming one already agreed to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Switch {
    pub resuming: bool,
    pub script: bool,
    pub formula: bool,
    pub cask: bool,
    /// Commands to point at Desktop's executable.
    pub repoint: Vec<PathBuf>,
    /// The daemon, when it still runs the old installation.
    pub agent: Option<Agent>,
    /// The script's bundle, removed once everything runs from Desktop.
    pub retire: Option<PathBuf>,
}

/// Why Setup stops without changing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocked {
    /// Aven executables Setup cannot attribute to an installer.
    UnknownAven { paths: Vec<PathBuf> },
    /// The daemon runs a program Setup does not recognize.
    UnknownAgent {
        plist: PathBuf,
        program: Option<PathBuf>,
    },
    /// Paths Setup would change but cannot write.
    Unwritable { paths: Vec<PathBuf> },
    /// The record of a switch in progress cannot be read.
    UnknownState { file: PathBuf, error: String },
}

/// Recorded once the user agrees, and removed when the switch completes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct State {
    version: u32,
    /// The daemon was running before the switch stopped it.
    restart_agent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The formula is still installed; the user removes it with
    /// `FORMULA_UNINSTALL`, then Setup checks again. `sync_paused` says the
    /// daemon was stopped and starts from Desktop once the switch completes.
    AwaitFormula {
        cask: bool,
        sync_paused: bool,
    },
    Done(Report),
}

/// What Setup leaves for the user to know or do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub switched: bool,
    /// The `aven` command that runs Desktop, first in `PATH` order.
    pub command: Option<PathBuf>,
    /// Setup created this link now.
    pub linked: bool,
    /// The command's directory is missing from the login shell's `PATH`.
    pub add_to_path: Option<PathBuf>,
    /// Other `aven` executables that run before Desktop's.
    pub shadowed_by: Vec<PathBuf>,
    /// Other `aven` executables left in place.
    pub others: Vec<PathBuf>,
    /// The cask is installed but Homebrew has not linked its command.
    pub cask_unlinked: bool,
    pub agent: Option<AgentReport>,
    /// A bundle at the script's location that is not Aven's, left in place.
    pub kept_bundle: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentReport {
    pub running: bool,
    pub disabled: bool,
}

impl Report {
    /// Whether anything here needs the user's attention.
    pub fn needs_attention(&self) -> bool {
        self.add_to_path.is_some()
            || !self.shadowed_by.is_empty()
            || self.cask_unlinked
            || self.command.is_none()
    }
}

/// Whether the app must show Setup before opening the database. Only reads a
/// few paths, so it is cheap enough for every launch.
pub fn needs_setup(layout: &Layout) -> bool {
    if layout.state_file.exists() {
        return true;
    }
    let exe = layout.executable();
    let agent_elsewhere = plist_program(&layout.agent_plist)
        .ok()
        .is_some_and(|program| !same_file(&program, &exe));
    let old = layout.script_bundle().exists() || layout.formula_installed() || agent_elsewhere;
    if !layout.in_place() {
        return old || layout.app.exists();
    }
    if old {
        return true;
    }
    let linked = layout
        .search_dirs()
        .iter()
        .any(|(dir, _)| same_file(&dir.join("aven"), &exe));
    !linked && !layout.cask_installed()
}

pub fn survey(layout: &Layout, system: &dyn System) -> Result<Survey> {
    let mut commands = Vec::new();
    for (dir, on_path) in layout.search_dirs() {
        let path = dir.join("aven");
        if path.symlink_metadata().is_err() {
            continue;
        }
        if let Some(owner) = classify(layout, system, &path) {
            commands.push(Found {
                path,
                owner,
                on_path,
            });
        }
    }
    let mut unreadable_agent = false;
    let agent = if layout.agent_plist.exists() {
        match plist_program(&layout.agent_plist) {
            Ok(program) => {
                let state = system.agent()?;
                let owner = classify(layout, system, &program).unwrap_or_else(|| {
                    // A program that is gone can still be recognized by path.
                    if layout.is_formula_path(&program) {
                        Owner::Formula
                    } else if under(&program, &layout.script_bundle()) {
                        Owner::Script
                    } else {
                        Owner::UnknownAven
                    }
                });
                Some(Agent {
                    program,
                    owner,
                    loaded: state.loaded,
                    disabled: state.disabled,
                })
            }
            Err(_) => {
                unreadable_agent = true;
                None
            }
        }
    } else {
        None
    };
    let script_bundle = Some(layout.script_bundle()).filter(|bundle| bundle.exists());
    Ok(Survey {
        formula: layout.formula_installed()
            || commands.iter().any(|found| found.owner == Owner::Formula),
        cask: layout.cask_installed(),
        commands,
        agent,
        unreadable_agent,
        script_bundle,
    })
}

/// Attributes an `aven` executable, or `None` for a link to nothing.
fn classify(layout: &Layout, system: &dyn System, path: &Path) -> Option<Owner> {
    let canonical = path.canonicalize().ok();
    if canonical.is_some() && canonical == layout.executable().canonicalize().ok() {
        return Some(if layout.in_brew_bin(path) {
            Owner::Cask
        } else {
            Owner::Desktop
        });
    }
    let mut seen = vec![path.to_path_buf()];
    if let Ok(target) = std::fs::read_link(path) {
        seen.push(match path.parent() {
            Some(dir) => dir.join(target),
            None => target,
        });
    }
    seen.extend(canonical.clone());
    if seen.iter().any(|seen| layout.is_formula_path(seen)) {
        return Some(Owner::Formula);
    }
    let bundle = layout.script_bundle();
    if seen.iter().any(|seen| under(seen, &bundle)) {
        return Some(Owner::Script);
    }
    let canonical = canonical?;
    if system.signing_identifier(&canonical).as_deref() != Some(AVEN_BUNDLE_ID) {
        return Some(Owner::Other);
    }
    // The install script once copied the signed executable straight into the
    // install directory.
    let bare = !path.is_symlink() && enclosing_bundle(&canonical).is_none();
    Some(if bare {
        Owner::Script
    } else {
        Owner::UnknownAven
    })
}

pub fn plan(layout: &Layout, survey: &Survey) -> Plan {
    let state = match load_state(layout) {
        Ok(state) => state,
        Err(error) => {
            return Plan::Blocked(Blocked::UnknownState {
                file: layout.state_file.clone(),
                error: format!("{error:#}"),
            });
        }
    };
    if !layout.in_place() {
        return Plan::MoveToApplications {
            running: layout.running.clone(),
        };
    }
    if survey.unreadable_agent {
        return Plan::Blocked(Blocked::UnknownAgent {
            plist: layout.agent_plist.clone(),
            program: None,
        });
    }
    if let Some(agent) = &survey.agent
        && matches!(agent.owner, Owner::UnknownAven | Owner::Other)
    {
        return Plan::Blocked(Blocked::UnknownAgent {
            plist: layout.agent_plist.clone(),
            program: Some(agent.program.clone()),
        });
    }
    let unknown: Vec<PathBuf> = survey
        .commands
        .iter()
        .filter(|found| found.owner == Owner::UnknownAven)
        .map(|found| found.path.clone())
        .collect();
    if !unknown.is_empty() {
        return Plan::Blocked(Blocked::UnknownAven { paths: unknown });
    }
    if !survey.terminal_install() && state.is_none() {
        return Plan::Ready;
    }
    let repoint: Vec<PathBuf> = survey
        .commands
        .iter()
        .filter(|found| found.owner == Owner::Script)
        .map(|found| found.path.clone())
        .collect();
    let agent = survey
        .agent
        .clone()
        .filter(|agent| agent.owner.is_terminal_install());
    let retire = survey
        .script_bundle
        .clone()
        .filter(|bundle| bundle_identifier(bundle).as_deref() == Some(AVEN_BUNDLE_ID));
    let mut unwritable: Vec<PathBuf> = repoint
        .iter()
        .filter(|path| !path.parent().is_some_and(writable))
        .cloned()
        .collect();
    if agent.is_some() && !layout.agent_plist.parent().is_some_and(writable) {
        unwritable.push(layout.agent_plist.clone());
    }
    if let Some(bundle) = &retire
        && !bundle.parent().is_some_and(writable)
    {
        unwritable.push(bundle.clone());
    }
    if !unwritable.is_empty() {
        return Plan::Blocked(Blocked::Unwritable { paths: unwritable });
    }
    Plan::Switch(Switch {
        resuming: state.is_some(),
        script: survey.script_bundle.is_some()
            || survey
                .commands
                .iter()
                .any(|found| found.owner == Owner::Script)
            || agent
                .as_ref()
                .is_some_and(|agent| agent.owner == Owner::Script),
        formula: survey.formula,
        cask: survey.cask,
        repoint,
        agent,
        retire,
    })
}

/// Moves the command and the daemon onto Desktop. The user agreed to `switch`,
/// or agreed earlier and it is resuming. Each step is repeatable, so a switch
/// interrupted anywhere continues from what the next survey finds.
pub fn switch(layout: &Layout, system: &dyn System, switch: &Switch) -> Result<Outcome> {
    let state = match load_state(layout)? {
        Some(state) => state,
        None => State {
            version: STATE_VERSION,
            restart_agent: switch
                .agent
                .as_ref()
                .is_some_and(|agent| agent.loaded && !agent.disabled),
        },
    };
    save_state(layout, state)?;
    let executable = layout.executable();
    if let Some(agent) = &switch.agent {
        // The old executable must not be removed while the daemon runs it.
        if agent.loaded {
            system.stop_agent().context("stop the daemon")?;
        }
        retarget_agent(&layout.agent_plist, &executable)?;
    }
    for path in &switch.repoint {
        link(path, &executable)?;
    }
    if survey(layout, system)?.formula {
        return Ok(Outcome::AwaitFormula {
            cask: switch.cask,
            sync_paused: state.restart_agent,
        });
    }
    finish(layout, system, Some(state)).map(Outcome::Done)
}

/// Completes setup when there is nothing to switch: links the command if
/// nothing provides it.
pub fn complete(layout: &Layout, system: &dyn System) -> Result<Report> {
    finish(layout, system, None)
}

fn finish(layout: &Layout, system: &dyn System, state: Option<State>) -> Result<Report> {
    if state.is_some_and(|state| state.restart_agent) && !system.agent()?.loaded {
        system.start_agent().context("start the daemon")?;
    }
    let before = survey(layout, system)?;
    let linked = ensure_command(layout, &before)?;
    let after = survey(layout, system)?;
    verify(layout, &after, state)?;
    let mut kept_bundle = None;
    if let Some(bundle) = &after.script_bundle {
        if bundle_identifier(bundle).as_deref() == Some(AVEN_BUNDLE_ID) {
            // Two bundles with one identifier compete for Launch Services
            // opens, including clicked notifications.
            if let Err(error) = system.unregister(bundle) {
                tracing::warn!(%error, bundle = %bundle.display(), "could not unregister");
            }
            std::fs::remove_dir_all(bundle)
                .with_context(|| format!("remove {}", bundle.display()))?;
        } else {
            kept_bundle = Some(bundle.clone());
        }
    }
    clear_state(layout)?;
    let mut report = report(layout, &after);
    report.switched = state.is_some();
    report.linked = linked;
    report.kept_bundle = kept_bundle;
    Ok(report)
}

/// Checks that everything a switch moved now runs Desktop.
fn verify(layout: &Layout, survey: &Survey, state: Option<State>) -> Result<()> {
    if let Some(found) = survey
        .commands
        .iter()
        .find(|found| found.owner.is_terminal_install())
    {
        bail!(
            "{} still runs the terminal installation",
            found.path.display()
        );
    }
    if let Some(agent) = &survey.agent {
        if agent.owner.is_terminal_install() {
            bail!(
                "the daemon still runs {}, from the terminal installation",
                agent.program.display()
            );
        }
        if state.is_some_and(|state| state.restart_agent) && !agent.loaded {
            bail!("the daemon did not start again");
        }
    }
    if state.is_some() && survey.desktop_command().is_none() && !survey.cask {
        bail!("no aven command runs {}", layout.executable().display());
    }
    Ok(())
}

/// Links `~/.local/bin/aven` to Desktop when no command runs it. A cask
/// install relies on Homebrew's link instead, so it never gets a second one.
fn ensure_command(layout: &Layout, survey: &Survey) -> Result<bool> {
    if survey.desktop_command().is_some() || survey.cask {
        return Ok(false);
    }
    let path = layout.command_link();
    if path.symlink_metadata().is_ok() {
        // Something else holds the name; it is reported, not replaced.
        return Ok(false);
    }
    let dir = path.parent().context("command link has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    link(&path, &layout.executable())?;
    Ok(true)
}

fn report(layout: &Layout, survey: &Survey) -> Report {
    let command = survey.desktop_command();
    let path_known = layout.path.is_some();
    let add_to_path = command
        .filter(|found| path_known && !found.on_path)
        .and_then(|found| found.path.parent().map(Path::to_path_buf));
    let shadowed_by: Vec<PathBuf> = match command {
        Some(command) if command.on_path => survey
            .commands
            .iter()
            .take_while(|found| found.path != command.path)
            .filter(|found| found.on_path)
            .map(|found| found.path.clone())
            .collect(),
        _ => Vec::new(),
    };
    let others = survey
        .commands
        .iter()
        .filter(|found| !found.owner.is_desktop() && !shadowed_by.contains(&found.path))
        .map(|found| found.path.clone())
        .collect();
    Report {
        switched: false,
        command: command.map(|found| found.path.clone()),
        linked: false,
        add_to_path,
        shadowed_by,
        others,
        cask_unlinked: survey.cask
            && !survey
                .commands
                .iter()
                .any(|found| found.owner == Owner::Cask),
        agent: survey.agent.as_ref().map(|agent| AgentReport {
            running: agent.loaded,
            disabled: agent.disabled,
        }),
        kept_bundle: None,
    }
}

/// Fills in the login shell's `PATH`, which an app started by Launch Services
/// does not inherit.
pub fn with_shell_path(mut layout: Layout) -> Layout {
    layout.path = login_shell_path();
    layout
}

fn login_shell_path() -> Option<Vec<PathBuf>> {
    let shell = std::env::var_os("SHELL")
        .filter(|shell| Path::new(shell).is_absolute())
        .unwrap_or_else(|| "/bin/zsh".into());
    // Interactive, so PATH set in `.zshrc` or `.bashrc` counts too.
    let script = format!("printf '\\n{PATH_MARKER}%s{PATH_MARKER}\\n' \"$PATH\"");
    let mut child = Command::new(shell)
        .args(["-l", "-i", "-c", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if started.elapsed() > SHELL_PATH_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let mut output = String::new();
    child.stdout.take()?.read_to_string(&mut output).ok()?;
    parse_shell_path(&output)
}

fn parse_shell_path(output: &str) -> Option<Vec<PathBuf>> {
    let start = output.rfind(&format!("\n{PATH_MARKER}"))? + PATH_MARKER.len() + 1;
    let rest = &output[start..];
    let path = &rest[..rest.find(PATH_MARKER)?];
    Some(
        path.split(':')
            .filter(|dir| !dir.is_empty())
            .map(PathBuf::from)
            .collect(),
    )
}

/// Points the daemon's plist at `executable`, keeping every other setting.
/// launchd reads the change when the agent next loads.
fn retarget_agent(plist: &Path, executable: &Path) -> Result<()> {
    let dir = plist.parent().context("plist has no directory")?;
    let name = plist.file_name().context("plist has no name")?;
    let staged = dir.join(format!(
        ".{}.setup.{}",
        name.to_string_lossy(),
        std::process::id()
    ));
    std::fs::copy(plist, &staged).with_context(|| format!("copy {}", plist.display()))?;
    let edited = (|| {
        let previous = plist_program(&staged)?;
        let executable = executable.to_str().context("non-UTF-8 executable path")?;
        plutil(&["-remove", "ProgramArguments.0"], &staged)?;
        plutil(
            &["-insert", "ProgramArguments.0", "-string", executable],
            &staged,
        )?;
        retarget_keep_alive(&staged, &previous, executable)?;
        // Login Items names the agent after the bundle it runs from.
        if plutil(
            &["-extract", "AssociatedBundleIdentifiers", "xml1", "-o", "-"],
            &staged,
        )
        .is_err()
        {
            plutil(
                &["-insert", "AssociatedBundleIdentifiers", "-array"],
                &staged,
            )?;
            plutil(
                &[
                    "-insert",
                    "AssociatedBundleIdentifiers.0",
                    "-string",
                    AVEN_BUNDLE_ID,
                ],
                &staged,
            )?;
        }
        plutil(&["-lint"], &staged)?;
        std::fs::rename(&staged, plist).with_context(|| format!("replace {}", plist.display()))
    })();
    if edited.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    edited?;
    let program = plist_program(plist)?;
    if program != executable {
        bail!(
            "{} names {} after the update",
            plist.display(),
            program.display()
        );
    }
    Ok(())
}

/// Keeps the daemon alive while `executable` exists, as `aven daemon install`
/// writes it, when the plist kept it alive unconditionally or while
/// `previous` existed. Any other `KeepAlive` was set by hand and stays.
fn retarget_keep_alive(plist: &Path, previous: &Path, executable: &str) -> Result<()> {
    let path_state = |program: &str| serde_json::json!({ "PathState": { program: true } });
    let always = plutil(&["-extract", "KeepAlive", "raw", "-o", "-"], plist)
        .is_ok_and(|value| value.trim() == "true");
    let while_previous_exists = || {
        let previous = previous.to_str()?;
        let current = plutil(&["-extract", "KeepAlive", "json", "-o", "-"], plist).ok()?;
        let current: serde_json::Value = serde_json::from_str(&current).ok()?;
        Some(current == path_state(previous))
    };
    if !always && while_previous_exists() != Some(true) {
        return Ok(());
    }
    let keep_alive = path_state(executable).to_string();
    plutil(&["-replace", "KeepAlive", "-json", &keep_alive], plist).map(drop)
}

/// The executable a LaunchAgent plist runs.
pub fn plist_program(plist: &Path) -> Result<PathBuf> {
    plutil(&["-extract", "ProgramArguments.0", "raw", "-o", "-"], plist)
        .map(|program| PathBuf::from(program.trim_end_matches('\n')))
}

fn bundle_identifier(bundle: &Path) -> Option<String> {
    plutil(
        &["-extract", "CFBundleIdentifier", "raw", "-o", "-"],
        &bundle.join("Contents/Info.plist"),
    )
    .ok()
    .map(|id| id.trim().to_string())
}

fn plutil(args: &[&str], file: &Path) -> Result<String> {
    let output = Command::new("/usr/bin/plutil")
        .args(args)
        .arg(file)
        .stdin(Stdio::null())
        .output()
        .context("run plutil")?;
    if !output.status.success() {
        bail!(
            "plutil {} {}: {}",
            args.join(" "),
            file.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Replaces `path` with a link to `target` in one step.
fn link(path: &Path, target: &Path) -> Result<()> {
    let dir = path.parent().context("link has no directory")?;
    let staged = dir.join(format!(".aven.setup.{}", std::process::id()));
    let _ = std::fs::remove_file(&staged);
    std::os::unix::fs::symlink(target, &staged)
        .with_context(|| format!("create {}", staged.display()))?;
    if let Err(error) = std::fs::rename(&staged, path) {
        let _ = std::fs::remove_file(&staged);
        return Err(error).with_context(|| format!("replace {}", path.display()));
    }
    Ok(())
}

fn load_state(layout: &Layout) -> Result<Option<State>> {
    let text = match std::fs::read_to_string(&layout.state_file) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("read {}", layout.state_file.display()));
        }
    };
    let state: State = serde_json::from_str(&text)
        .with_context(|| format!("parse {}", layout.state_file.display()))?;
    if state.version != STATE_VERSION {
        bail!("unknown version {}", state.version);
    }
    Ok(Some(state))
}

fn save_state(layout: &Layout, state: State) -> Result<()> {
    let dir = layout
        .state_file
        .parent()
        .context("state file has no directory")?;
    std::fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    let staged = layout.state_file.with_extension("json.tmp");
    std::fs::write(&staged, serde_json::to_vec_pretty(&state)?)
        .with_context(|| format!("write {}", staged.display()))?;
    std::fs::rename(&staged, &layout.state_file)
        .with_context(|| format!("replace {}", layout.state_file.display()))
}

fn clear_state(layout: &Layout) -> Result<()> {
    match std::fs::remove_file(&layout.state_file) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("remove {}", layout.state_file.display()))
        }
        _ => Ok(()),
    }
}

fn writable(dir: &Path) -> bool {
    tempfile::Builder::new()
        .prefix(".aven-setup-")
        .tempfile_in(dir)
        .is_ok()
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn same_dir(left: &Path, right: &Path) -> bool {
    left == right || same_file(left, right)
}

/// Whether `path` is `root` or inside it, also through `root`'s real path.
fn under(path: &Path, root: &Path) -> bool {
    path.starts_with(root) || root.canonicalize().is_ok_and(|real| path.starts_with(real))
}

#[cfg(test)]
mod tests;
