use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

use crate::config::{self, AppConfig};

const LABEL: &str = "com.raine.aven.daemon";
const DEFAULT_PATH: &str = "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin";

pub struct ServiceInstallArgs {
    pub db_path: PathBuf,
    pub config: AppConfig,
}

pub fn install(args: ServiceInstallArgs) -> Result<()> {
    install_with_runner(args, &SystemRunner)
}

pub fn uninstall() -> Result<()> {
    uninstall_with_runner(&SystemRunner)
}

trait LaunchctlRunner {
    fn run(&self, args: &[&str]) -> Result<Output>;
}

struct SystemRunner;

impl LaunchctlRunner for SystemRunner {
    fn run(&self, args: &[&str]) -> Result<Output> {
        Command::new("/bin/launchctl")
            .args(args)
            .output()
            .with_context(|| format!("run launchctl {}", args.join(" ")))
    }
}

#[cfg(target_os = "macos")]
fn install_with_runner(args: ServiceInstallArgs, runner: &impl LaunchctlRunner) -> Result<()> {
    validate_install_config(&args.config)?;
    let spec = ServiceSpec::from_current_process(args.db_path)?;
    let plist = render_plist(&spec);
    if service_is_loaded(runner, &spec)? {
        run_launchctl(runner, &["bootout", &spec.service_target])?;
    }
    write_plist(&spec, &plist)?;
    run_launchctl(runner, &["enable", &spec.service_target])?;
    run_launchctl(runner, &["bootstrap", &spec.domain, spec.plist_path_str()?])?;
    println!("installed {}", spec.plist_path.display());
    println!("logs {}", spec.log_dir.display());
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn install_with_runner(_args: ServiceInstallArgs, _runner: &impl LaunchctlRunner) -> Result<()> {
    bail!(
        "error unsupported-platform command=daemon-install platform={}",
        std::env::consts::OS
    )
}

#[cfg(target_os = "macos")]
fn uninstall_with_runner(runner: &impl LaunchctlRunner) -> Result<()> {
    let spec = ServiceSpec::from_current_process(config::default_db_path()?)?;
    if service_is_loaded(runner, &spec)? {
        run_launchctl(runner, &["bootout", &spec.service_target])?;
    }
    let _ = runner.run(&["disable", &spec.service_target]);
    if spec.plist_path.exists() {
        std::fs::remove_file(&spec.plist_path)
            .with_context(|| format!("remove {}", spec.plist_path.display()))?;
    }
    println!("uninstalled {}", spec.plist_path.display());
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn uninstall_with_runner(_runner: &impl LaunchctlRunner) -> Result<()> {
    bail!(
        "error unsupported-platform command=daemon-uninstall platform={}",
        std::env::consts::OS
    )
}

#[cfg(target_os = "macos")]
fn validate_install_config(config: &AppConfig) -> Result<()> {
    if !config.sync.enabled {
        bail!("error sync-disabled hint=\"set sync.enabled = true in config.yaml\"");
    }
    config
        .sync
        .server_url
        .as_deref()
        .filter(|server| !server.trim().is_empty())
        .context("error sync-server-required hint=\"set sync.server_url in config.yaml\"")?;
    config.wake_addr()?;
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
struct ServiceSpec {
    label: String,
    executable: PathBuf,
    db_path: PathBuf,
    config_dir: Option<PathBuf>,
    path_env: String,
    log_dir: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    log_file: PathBuf,
    plist_path: PathBuf,
    domain: String,
    service_target: String,
}

#[cfg(target_os = "macos")]
impl ServiceSpec {
    fn from_current_process(db_path: PathBuf) -> Result<Self> {
        let home = dirs::home_dir().context("could not find home directory")?;
        let launch_agents_dir = home.join("Library/LaunchAgents");
        let log_dir = home.join("Library/Logs/aven");
        let label = LABEL.to_string();
        let uid = current_uid();
        let domain = format!("gui/{uid}");
        let service_target = format!("{domain}/{label}");
        let config_dir = std::env::var_os("AVEN_CONFIG_DIR")
            .map(PathBuf::from)
            .map(absolute_path)
            .transpose()?;
        Ok(Self {
            label: label.clone(),
            executable: std::env::current_exe().context("resolve current executable")?,
            db_path: absolute_path(db_path)?,
            config_dir,
            path_env: std::env::var("PATH").unwrap_or_else(|_| DEFAULT_PATH.to_string()),
            stdout_path: log_dir.join("daemon.out.log"),
            stderr_path: log_dir.join("daemon.err.log"),
            log_file: log_dir.join("daemon.log"),
            log_dir,
            plist_path: launch_agents_dir.join(format!("{label}.plist")),
            domain,
            service_target,
        })
    }

    fn plist_path_str(&self) -> Result<&str> {
        self.plist_path
            .to_str()
            .with_context(|| format!("non-utf8 plist path {}", self.plist_path.display()))
    }
}

#[cfg(target_os = "macos")]
fn absolute_path(path: PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path);
    }
    Ok(std::env::current_dir()?.join(path))
}

#[cfg(target_os = "macos")]
fn service_is_loaded(runner: &impl LaunchctlRunner, spec: &ServiceSpec) -> Result<bool> {
    let output = runner.run(&["print", &spec.service_target])?;
    Ok(output.status.success())
}

#[cfg(target_os = "macos")]
fn run_launchctl(runner: &impl LaunchctlRunner, args: &[&str]) -> Result<()> {
    let output = runner.run(args)?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    bail!(
        "error launchctl-failed command={} status={} stdout={} stderr={}",
        args.join(" "),
        output.status,
        stdout.trim(),
        stderr.trim()
    )
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

#[cfg(target_os = "macos")]
fn render_plist(spec: &ServiceSpec) -> String {
    let mut env = vec![
        ("PATH", spec.path_env.as_str()),
        ("AVEN_LOG_FILE", path_str(&spec.log_file)),
    ];
    if let Some(path) = &spec.config_dir {
        env.push(("AVEN_CONFIG_DIR", path_str(path)));
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
  <dict>\n\
    <key>SuccessfulExit</key>\n\
    <false/>\n\
    <key>Crashed</key>\n\
    <true/>\n\
  </dict>\n\
  <key>ThrottleInterval</key>\n\
  <integer>30</integer>\n\
  <key>StandardOutPath</key>\n\
  <string>{}</string>\n\
  <key>StandardErrorPath</key>\n\
  <string>{}</string>\n\
</dict>\n\
</plist>\n",
        escape_xml(&spec.label),
        escape_xml(path_str(&spec.executable)),
        escape_xml(path_str(&spec.db_path)),
        env_xml,
        escape_xml(path_str(&spec.stdout_path)),
        escape_xml(path_str(&spec.stderr_path))
    )
}

#[cfg(target_os = "macos")]
fn path_str(path: &Path) -> &str {
    path.to_str().unwrap_or("")
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
            label: "com.raine.aven.daemon".to_string(),
            executable: PathBuf::from("/bin/aven&test"),
            db_path: PathBuf::from("/tmp/db.sqlite"),
            config_dir: Some(PathBuf::from("/tmp/config")),
            path_env: "/usr/bin:/bin".to_string(),
            log_dir: PathBuf::from("/tmp/logs"),
            stdout_path: PathBuf::from("/tmp/logs/out.log"),
            stderr_path: PathBuf::from("/tmp/logs/err.log"),
            log_file: PathBuf::from("/tmp/logs/daemon.log"),
            plist_path: PathBuf::from("/tmp/com.raine.aven.daemon.plist"),
            domain: "gui/501".to_string(),
            service_target: "gui/501/com.raine.aven.daemon".to_string(),
        };
        let plist = render_plist(&spec);
        assert!(plist.contains("<string>com.raine.aven.daemon</string>"));
        assert!(plist.contains("<string>/bin/aven&amp;test</string>"));
        assert!(plist.contains("<string>--db</string>"));
        assert!(plist.contains("<string>/tmp/db.sqlite</string>"));
        assert!(plist.contains("<string>daemon</string>"));
        assert!(plist.contains("<key>AVEN_CONFIG_DIR</key>"));
        assert!(plist.contains("<key>AVEN_LOG_FILE</key>"));
        assert!(plist.contains("<key>RunAtLoad</key>"));
        assert!(plist.contains("<key>KeepAlive</key>"));
        assert!(plist.contains("<key>ThrottleInterval</key>"));
    }

    #[test]
    fn install_uses_expected_launchctl_sequence_when_not_loaded() {
        let runner = FakeRunner::new(vec![failure(), success(), success()]);
        let spec = test_spec();
        write_plist(&spec, "old").unwrap();
        let plist = render_plist(&spec);
        assert!(!service_is_loaded(&runner, &spec).unwrap());
        write_plist(&spec, &plist).unwrap();
        run_launchctl(&runner, &["enable", &spec.service_target]).unwrap();
        run_launchctl(
            &runner,
            &["bootstrap", &spec.domain, spec.plist_path_str().unwrap()],
        )
        .unwrap();
        assert_eq!(
            runner.commands(),
            vec![
                vec!["print", "gui/501/com.raine.aven.daemon"],
                vec!["enable", "gui/501/com.raine.aven.daemon"],
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
            path_env: DEFAULT_PATH.to_string(),
            log_dir: dir.join("logs"),
            stdout_path: dir.join("logs/out.log"),
            stderr_path: dir.join("logs/err.log"),
            log_file: dir.join("logs/daemon.log"),
            plist_path: dir.join("LaunchAgents/com.raine.aven.daemon.plist"),
            domain: "gui/501".to_string(),
            service_target: "gui/501/com.raine.aven.daemon".to_string(),
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

    impl LaunchctlRunner for FakeRunner {
        fn run(&self, args: &[&str]) -> Result<Output> {
            self.commands
                .lock()
                .unwrap()
                .push(args.iter().map(|arg| arg.to_string()).collect());
            Ok(self.outputs.lock().unwrap().pop().unwrap_or_else(success))
        }
    }

    fn success() -> Output {
        Output {
            status: ExitStatus::from_raw(0),
            stdout: Vec::new(),
            stderr: Vec::new(),
        }
    }

    fn failure() -> Output {
        Output {
            status: ExitStatus::from_raw(1 << 8),
            stdout: Vec::new(),
            stderr: b"not loaded".to_vec(),
        }
    }
}
