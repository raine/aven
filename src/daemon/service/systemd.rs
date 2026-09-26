//! Manages the daemon as a systemd user service on Linux.
use std::path::{Path, PathBuf};
use std::process::Output;

use anyhow::{Context, Result, bail};

use super::{
    ServiceInstallArgs, ServiceRepairArgs, ServiceStatus, paths_resolve_to_same_file,
    validate_install_config,
};
#[cfg(target_os = "linux")]
use super::{absolute_path, stable_program_path_from_candidates};

const UNIT: &str = "aven-daemon.service";
const DEFAULT_PATH: &str = "/usr/local/bin:/usr/bin:/bin";

pub(super) trait SystemctlRunner {
    /// Runs `systemctl --user` with `args`.
    fn run(&self, args: &[&str]) -> Result<Output>;
}

#[cfg(target_os = "linux")]
pub(super) struct SystemSystemctl;

#[cfg(target_os = "linux")]
impl SystemctlRunner for SystemSystemctl {
    fn run(&self, args: &[&str]) -> Result<Output> {
        std::process::Command::new("systemctl")
            .arg("--user")
            .args(args)
            .output()
            .with_context(|| format!("run systemctl --user {}", args.join(" ")))
    }
}

#[derive(Debug, Clone)]
pub(super) struct UnitSpec {
    executable: PathBuf,
    db_path: PathBuf,
    config_dir: Option<PathBuf>,
    path_env: String,
    unit_path: PathBuf,
}

impl UnitSpec {
    #[cfg(target_os = "linux")]
    pub(super) fn from_install_args(db_path: PathBuf, program: Option<PathBuf>) -> Result<Self> {
        let config_home = dirs::config_dir().context("could not find config directory")?;
        let config_dir = std::env::var_os("AVEN_CONFIG_DIR")
            .map(PathBuf::from)
            .map(absolute_path)
            .transpose()?;
        let current_exe = std::env::current_exe().context("resolve current executable")?;
        let executable = match program {
            Some(program) => absolute_path(program)?,
            None => stable_program_path(&current_exe),
        };
        Ok(Self {
            executable,
            db_path: absolute_path(db_path)?,
            config_dir,
            path_env: std::env::var("PATH").unwrap_or_else(|_| DEFAULT_PATH.to_string()),
            unit_path: config_home.join("systemd/user").join(UNIT),
        })
    }
}

#[cfg(target_os = "linux")]
fn stable_program_path(current_exe: &Path) -> PathBuf {
    let mut candidates = vec![
        PathBuf::from("/home/linuxbrew/.linuxbrew/bin/aven"),
        PathBuf::from("/usr/local/bin/aven"),
        PathBuf::from("/usr/bin/aven"),
    ];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".cargo/bin/aven"));
        candidates.push(home.join(".local/bin/aven"));
    }
    stable_program_path_from_candidates(current_exe, candidates)
}

pub(super) fn install_with_runner(
    args: ServiceInstallArgs,
    spec: &UnitSpec,
    runner: &impl SystemctlRunner,
) -> Result<()> {
    validate_install_config(&args.config)?;
    reload_service(runner, spec)?;
    println!("installed {}", spec.unit_path.display());
    println!("logs journalctl --user -u {UNIT}");
    Ok(())
}

pub(super) fn uninstall_with_runner(spec: &UnitSpec, runner: &impl SystemctlRunner) -> Result<()> {
    if spec.unit_path.exists() {
        run_systemctl(runner, &["disable", "--now", UNIT])?;
        std::fs::remove_file(&spec.unit_path)
            .with_context(|| format!("remove {}", spec.unit_path.display()))?;
        run_systemctl(runner, &["daemon-reload"])?;
    }
    println!("uninstalled {}", spec.unit_path.display());
    Ok(())
}

pub(super) fn restart_with_runner(runner: &impl SystemctlRunner) -> Result<()> {
    run_systemctl(runner, &["restart", UNIT])?;
    println!("restarted {UNIT}");
    Ok(())
}

pub(super) fn repair_with_runner(
    args: ServiceRepairArgs,
    spec: &UnitSpec,
    runner: &impl SystemctlRunner,
) -> Result<()> {
    if !spec.unit_path.exists() {
        if args.if_installed {
            println!("daemon repair skipped installed=no");
            return Ok(());
        }
        bail!(
            "error daemon-not-installed path={}",
            spec.unit_path.display()
        );
    }
    validate_install_config(&args.config)?;
    reload_service(runner, spec)?;
    println!(
        "daemon repair installed=yes restarted=yes path={}",
        spec.executable.display()
    );
    Ok(())
}

pub(super) fn status_with_runner(
    spec: &UnitSpec,
    runner: &impl SystemctlRunner,
) -> Result<ServiceStatus> {
    let program = read_unit_program(&spec.unit_path)?;
    let current_executable = std::env::current_exe().context("resolve current executable")?;
    let program_matches_current = program
        .as_ref()
        .map(|program| paths_resolve_to_same_file(program, &current_executable));
    let args = [
        "show",
        UNIT,
        "--property=LoadState",
        "--property=ActiveState",
    ];
    let output = runner.run(&args)?;
    if !output.status.success() {
        return Err(systemctl_failure(&args, &output));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let property = |name: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
            .unwrap_or_default()
            .to_string()
    };
    Ok(ServiceStatus {
        platform_supported: true,
        installed: spec.unit_path.exists(),
        loaded: Some(property("LoadState") == "loaded"),
        running: Some(property("ActiveState") == "active"),
        service_path: spec.unit_path.clone(),
        program,
        current_executable,
        program_matches_current,
        stdout_path: None,
        stderr_path: None,
    })
}

fn reload_service(runner: &impl SystemctlRunner, spec: &UnitSpec) -> Result<()> {
    write_unit(spec, &render_unit(spec))?;
    run_systemctl(runner, &["daemon-reload"])?;
    run_systemctl(runner, &["enable", UNIT])?;
    run_systemctl(runner, &["restart", UNIT])
}

fn write_unit(spec: &UnitSpec, unit: &str) -> Result<()> {
    if let Some(parent) = spec.unit_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create systemd user directory {}", parent.display()))?;
    }
    let tmp = spec.unit_path.with_extension("service.tmp");
    std::fs::write(&tmp, unit).with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &spec.unit_path).with_context(|| {
        format!(
            "replace {} with {}",
            spec.unit_path.display(),
            tmp.display()
        )
    })
}

/// The daemon exits when its executable changes, so systemd restarts it
/// without a start limit.
fn render_unit(spec: &UnitSpec) -> String {
    let mut environment = format!(
        "Environment={}\n",
        quote(&format!("PATH={}", spec.path_env))
    );
    if let Some(path) = &spec.config_dir {
        environment.push_str(&format!(
            "Environment={}\n",
            quote(&format!("AVEN_CONFIG_DIR={}", path.display()))
        ));
    }
    format!(
        "[Unit]\n\
Description=Aven sync daemon\n\
StartLimitIntervalSec=0\n\
\n\
[Service]\n\
ExecStart={} --db {} daemon\n\
{environment}\
Restart=always\n\
RestartSec=30\n\
\n\
[Install]\n\
WantedBy=default.target\n",
        quote(&spec.executable.display().to_string()),
        quote(&spec.db_path.display().to_string()),
    )
}

/// Quotes a value for a unit file, escaping specifiers and variable expansion.
fn quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '%' => quoted.push_str("%%"),
            '$' => quoted.push_str("$$"),
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

fn read_unit_program(path: &Path) -> Result<Option<PathBuf>> {
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    Ok(program_from_unit_text(&text).map(PathBuf::from))
}

fn program_from_unit_text(text: &str) -> Option<String> {
    let exec = text
        .lines()
        .find_map(|line| line.strip_prefix("ExecStart="))?;
    let mut chars = exec.strip_prefix('"')?.chars();
    let mut program = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(program),
            '\\' => program.push(chars.next()?),
            '%' | '$' => {
                chars.next();
                program.push(ch);
            }
            ch => program.push(ch),
        }
    }
    None
}

fn run_systemctl(runner: &impl SystemctlRunner, args: &[&str]) -> Result<()> {
    let output = runner.run(args)?;
    if output.status.success() {
        return Ok(());
    }
    Err(systemctl_failure(args, &output))
}

fn systemctl_failure(args: &[&str], output: &Output) -> anyhow::Error {
    anyhow::anyhow!(
        "error systemctl-failed command={} status={} stdout={} stderr={}",
        args.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    )
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;
    use std::sync::Mutex;

    use super::*;
    use crate::config::AppConfig;

    #[test]
    fn renders_escaped_unit() {
        let spec = UnitSpec {
            executable: PathBuf::from("/opt/my aven/aven"),
            db_path: PathBuf::from("/tmp/100%\"db$.sqlite"),
            config_dir: Some(PathBuf::from("/tmp/config")),
            path_env: "/usr/bin:/bin".to_string(),
            unit_path: PathBuf::from("/tmp/aven-daemon.service"),
        };
        let unit = render_unit(&spec);
        assert!(
            unit.contains(
                "ExecStart=\"/opt/my aven/aven\" --db \"/tmp/100%%\\\"db$$.sqlite\" daemon\n"
            ),
            "{unit}"
        );
        assert!(unit.contains("Environment=\"PATH=/usr/bin:/bin\"\n"));
        assert!(unit.contains("Environment=\"AVEN_CONFIG_DIR=/tmp/config\"\n"));
        assert!(unit.contains("Restart=always\n"));
        assert!(unit.contains("WantedBy=default.target\n"));
        assert_eq!(
            program_from_unit_text(&unit).as_deref(),
            Some("/opt/my aven/aven")
        );
    }

    #[test]
    fn program_from_unit_text_unescapes() {
        let unit = format!("ExecStart={} daemon\n", quote("/a\"b\\c%d$e"));
        assert_eq!(
            program_from_unit_text(&unit).as_deref(),
            Some("/a\"b\\c%d$e")
        );
    }

    #[test]
    fn install_writes_unit_then_enables_and_restarts() {
        let spec = test_spec();
        let runner = FakeRunner::new(vec![]);

        install_with_runner(install_args(), &spec, &runner).unwrap();

        assert!(spec.unit_path.exists());
        assert_eq!(
            runner.commands(),
            vec![
                vec!["daemon-reload"],
                vec!["enable", UNIT],
                vec!["restart", UNIT],
            ]
        );
    }

    #[test]
    fn install_requires_enabled_sync() {
        let spec = test_spec();
        let runner = FakeRunner::new(vec![]);
        let mut args = install_args();
        args.config.sync.enabled = false;

        assert!(install_with_runner(args, &spec, &runner).is_err());
        assert!(!spec.unit_path.exists());
        assert!(runner.commands().is_empty());
    }

    #[test]
    fn repair_skips_missing_unit_when_asked() {
        let spec = test_spec();
        let runner = FakeRunner::new(vec![]);
        let args = ServiceRepairArgs {
            db_path: spec.db_path.clone(),
            config: enabled_config(),
            program: None,
            if_installed: true,
        };

        repair_with_runner(args, &spec, &runner).unwrap();

        assert!(runner.commands().is_empty());
    }

    #[test]
    fn uninstall_disables_and_removes_unit() {
        let spec = test_spec();
        install_with_runner(install_args(), &spec, &FakeRunner::new(vec![])).unwrap();
        let runner = FakeRunner::new(vec![]);

        uninstall_with_runner(&spec, &runner).unwrap();

        assert!(!spec.unit_path.exists());
        assert_eq!(
            runner.commands(),
            vec![vec!["disable", "--now", UNIT], vec!["daemon-reload"]]
        );
    }

    #[test]
    fn status_reads_load_and_active_state() {
        let spec = test_spec();
        install_with_runner(install_args(), &spec, &FakeRunner::new(vec![])).unwrap();
        let runner = FakeRunner::new(vec![success_with_stdout(
            "LoadState=loaded\nActiveState=active\n",
        )]);

        let status = status_with_runner(&spec, &runner).unwrap();

        assert!(status.installed);
        assert_eq!(status.loaded, Some(true));
        assert_eq!(status.running, Some(true));
        assert_eq!(status.program, Some(spec.executable.clone()));

        let runner = FakeRunner::new(vec![success_with_stdout(
            "LoadState=loaded\nActiveState=activating\n",
        )]);
        let status = status_with_runner(&spec, &runner).unwrap();
        assert_eq!(status.running, Some(false));
    }

    #[test]
    fn restart_restarts_unit() {
        let runner = FakeRunner::new(vec![]);

        restart_with_runner(&runner).unwrap();

        assert_eq!(runner.commands(), vec![vec!["restart", UNIT]]);
    }

    #[test]
    fn systemctl_failure_is_reported() {
        let spec = test_spec();
        let runner = FakeRunner::new(vec![success(), failure()]);

        let error = install_with_runner(install_args(), &spec, &runner).unwrap_err();

        assert!(
            error
                .to_string()
                .contains("systemctl-failed command=enable"),
            "{error}"
        );
    }

    fn test_spec() -> UnitSpec {
        let dir = tempfile::tempdir().unwrap().keep();
        UnitSpec {
            executable: PathBuf::from("/usr/bin/aven"),
            db_path: dir.join("db.sqlite"),
            config_dir: None,
            path_env: DEFAULT_PATH.to_string(),
            unit_path: dir.join("systemd/user").join(UNIT),
        }
    }

    fn enabled_config() -> AppConfig {
        let mut config = AppConfig::default();
        config.sync.enabled = true;
        config
    }

    fn install_args() -> ServiceInstallArgs {
        ServiceInstallArgs {
            db_path: PathBuf::from("/tmp/db.sqlite"),
            config: enabled_config(),
            program: None,
        }
    }

    struct FakeRunner {
        outputs: Mutex<Vec<Output>>,
        commands: Mutex<Vec<Vec<String>>>,
    }

    impl FakeRunner {
        fn new(mut outputs: Vec<Output>) -> Self {
            outputs.reverse();
            Self {
                outputs: Mutex::new(outputs),
                commands: Mutex::new(Vec::new()),
            }
        }

        fn commands(&self) -> Vec<Vec<String>> {
            self.commands.lock().unwrap().clone()
        }
    }

    impl SystemctlRunner for FakeRunner {
        fn run(&self, args: &[&str]) -> Result<Output> {
            self.commands
                .lock()
                .unwrap()
                .push(args.iter().map(|arg| arg.to_string()).collect());
            Ok(self.outputs.lock().unwrap().pop().unwrap_or_else(success))
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

    fn failure() -> Output {
        Output {
            status: ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"systemctl failure".to_vec(),
        }
    }
}
